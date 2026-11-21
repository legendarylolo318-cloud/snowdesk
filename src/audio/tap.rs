//! Source wrapper that taps samples, runs a small FFT on the audio thread and
//! publishes bass energy through atomics. The render loop only reads atomics.

use rodio::Source;
use rodio::source::SeekError;
use rodio::{ChannelCount, Sample, SampleRate};
use rustfft::{Fft, FftPlanner, num_complex::Complex};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::time::Duration;

pub const FFT_SIZE: usize = 512;

#[derive(Default)]
pub struct Levels {
    /// Smoothed bass energy, f32 bits.
    bass: AtomicU32,
    /// Incremented on every detected bass hit.
    beats: AtomicU32,
    /// System audio capture is running: it hears everything, including our
    /// own player, so the player's tap stays quiet to avoid double beats.
    pub external: AtomicBool,
}

impl Levels {
    pub fn bass(&self) -> f32 {
        f32::from_bits(self.bass.load(Ordering::Relaxed))
    }
    pub fn beats(&self) -> u32 {
        self.beats.load(Ordering::Relaxed)
    }
}

/// Bass-hit detector: feed it interleaved samples.
pub struct Analyzer {
    levels: Arc<Levels>,
    fft: Arc<dyn Fft<f32>>,
    buf: Vec<Complex<f32>>,
    scratch: Vec<Complex<f32>>,
    n: usize,
    acc: f32,
    ch_i: u16,
    avg: f32,
    cooldown: u32,
}

impl Analyzer {
    pub fn new(levels: Arc<Levels>) -> Self {
        let fft = FftPlanner::new().plan_fft_forward(FFT_SIZE);
        let scratch = vec![Complex::default(); fft.get_inplace_scratch_len()];
        Self {
            levels,
            fft,
            buf: vec![Complex::default(); FFT_SIZE],
            scratch,
            n: 0,
            acc: 0.0,
            ch_i: 0,
            avg: 0.0,
            cooldown: 0,
        }
    }

    #[inline]
    pub fn push(&mut self, s: f32, channels: u16, sample_rate: u32) {
        self.acc += s;
        self.ch_i += 1;
        if self.ch_i >= channels.max(1) {
            self.buf[self.n] = Complex::new(self.acc / channels.max(1) as f32, 0.0);
            self.acc = 0.0;
            self.ch_i = 0;
            self.n += 1;
            if self.n == FFT_SIZE {
                self.n = 0;
                self.analyze(sample_rate as f32);
            }
        }
    }

    fn analyze(&mut self, sr: f32) {
        // Hann window.
        for (i, c) in self.buf.iter_mut().enumerate() {
            let w = 0.5 - 0.5 * (std::f32::consts::TAU * i as f32 / FFT_SIZE as f32).cos();
            c.re *= w;
        }
        self.fft
            .process_with_scratch(&mut self.buf, &mut self.scratch);
        let bin_hz = sr / FFT_SIZE as f32;
        let hi = ((150.0 / bin_hz) as usize).clamp(2, FFT_SIZE / 2);
        let e: f32 = self.buf[1..hi].iter().map(|c| c.norm()).sum::<f32>() / hi as f32;
        let e = e / (FFT_SIZE as f32 * 0.25);
        self.avg = self.avg * 0.92 + e * 0.08;
        let prev = self.levels.bass();
        let smooth = prev * 0.7 + e * 0.3;
        self.levels.bass.store(smooth.to_bits(), Ordering::Relaxed);
        if self.cooldown > 0 {
            self.cooldown -= 1;
        } else if e > self.avg * 1.5 + 0.02 {
            self.levels.beats.fetch_add(1, Ordering::Relaxed);
            self.cooldown = 8;
        }
    }
}

pub struct Tap<S: Source> {
    inner: S,
    levels: Arc<Levels>,
    analyzer: Analyzer,
}

impl<S: Source> Tap<S> {
    pub fn new(inner: S, levels: Arc<Levels>) -> Self {
        Self {
            inner,
            analyzer: Analyzer::new(levels.clone()),
            levels,
        }
    }
}

impl<S: Source> Iterator for Tap<S> {
    type Item = Sample;
    fn next(&mut self) -> Option<Sample> {
        let s = self.inner.next()?;
        if !self.levels.external.load(Ordering::Relaxed) {
            let (ch, sr) = (self.inner.channels().get(), self.inner.sample_rate().get());
            self.analyzer.push(s, ch, sr);
        }
        Some(s)
    }
}

impl<S: Source> Source for Tap<S> {
    fn current_span_len(&self) -> Option<usize> {
        self.inner.current_span_len()
    }
    fn channels(&self) -> ChannelCount {
        self.inner.channels()
    }
    fn sample_rate(&self) -> SampleRate {
        self.inner.sample_rate()
    }
    fn total_duration(&self) -> Option<Duration> {
        self.inner.total_duration()
    }
    fn try_seek(&mut self, pos: Duration) -> Result<(), SeekError> {
        self.inner.try_seek(pos)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rodio::buffer::SamplesBuffer;

    fn kicks(sr: u32, seconds: f32, bass: bool) -> Vec<f32> {
        (0..(sr as f32 * seconds) as usize * 2)
            .map(|i| {
                let t = (i / 2) as f32 / sr as f32;
                let ph = t % 0.5;
                let env = (-ph * 14.0).exp();
                let f = if bass { 60.0 } else { 6000.0 };
                0.8 * env * (std::f32::consts::TAU * f * ph).sin()
            })
            .collect()
    }

    fn run(samples: Vec<f32>) -> Arc<Levels> {
        let levels = Arc::new(Levels::default());
        let src = SamplesBuffer::new(
            std::num::NonZero::new(2).unwrap(),
            std::num::NonZero::new(44100).unwrap(),
            samples,
        );
        Tap::new(src, levels.clone()).for_each(drop);
        levels
    }

    #[test]
    fn bass_kicks_trigger_beats() {
        let l = run(kicks(44100, 3.0, true));
        assert!(l.beats() >= 3, "beats {}", l.beats());
    }

    #[test]
    fn treble_does_not_trigger_beats() {
        let l = run(kicks(44100, 3.0, false));
        assert_eq!(l.beats(), 0);
    }
}
