//! "React to any music on this PC": capture what the speakers play and run
//! the bass detector on it. Linux: the default sink's monitor source
//! (PulseAudio / PipeWire via pipewire-pulse). Windows: WASAPI loopback on
//! the default output. Audio is only analysed in memory, never stored.

use super::tap::{Analyzer, Levels};
use rodio::cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use rodio::cpal::{self, SampleFormat};
use std::sync::Arc;
use std::sync::atomic::Ordering;

pub struct Capture {
    _stream: cpal::Stream,
    levels: Arc<Levels>,
}

impl Drop for Capture {
    fn drop(&mut self) {
        self.levels.external.store(false, Ordering::Relaxed);
    }
}

fn name(d: &cpal::Device) -> String {
    d.description()
        .map(|d| d.name().to_string())
        .unwrap_or_default()
}

/// The device whose input is "what you hear".
fn pick_device(host: &cpal::Host) -> Result<cpal::Device, String> {
    let out = host
        .default_output_device()
        .ok_or("no audio output device")?;
    #[cfg(windows)]
    {
        // WASAPI: an output device opened for input records in loopback.
        Ok(out)
    }
    #[cfg(not(windows))]
    {
        let out_name = name(&out);
        let monitors: Vec<cpal::Device> = host
            .input_devices()
            .map_err(|e| e.to_string())?
            .filter(|d| name(d).to_ascii_lowercase().starts_with("monitor of"))
            .collect();
        monitors
            .iter()
            .position(|d| name(d).ends_with(&out_name))
            .or(if monitors.is_empty() { None } else { Some(0) })
            .map(|i| monitors[i].clone())
            .ok_or_else(|| {
                "no monitor source for the speakers (needs PulseAudio or PipeWire with pipewire-pulse)"
                    .to_string()
            })
    }
}

pub fn start(levels: Arc<Levels>) -> Result<Capture, String> {
    let host = cpal::default_host();
    let dev = pick_device(&host)?;
    #[cfg(windows)]
    let supported = dev.default_output_config().map_err(|e| e.to_string())?;
    #[cfg(not(windows))]
    let supported = dev.default_input_config().map_err(|e| e.to_string())?;
    let native = supported.sample_format();
    let config: cpal::StreamConfig = supported.into();
    let (ch, sr) = (config.channels, config.sample_rate);
    let err = |e| tracing::warn!("system audio capture: {e}");
    // Ask for f32 first: PulseAudio/PipeWire convert from whatever the
    // monitor source uses (often 24-bit). WASAPI loopback must use the mix
    // format, so fall back to the device's own format.
    let mut an = Analyzer::new(levels.clone());
    let stream = match dev.build_input_stream(
        config,
        move |data: &[f32], _: &_| {
            for &s in data {
                an.push(s, ch, sr);
            }
        },
        err,
        None,
    ) {
        Ok(s) => Ok(s),
        Err(first) => {
            let mut an = Analyzer::new(levels.clone());
            match native {
                SampleFormat::I16 => dev.build_input_stream(
                    config,
                    move |data: &[i16], _: &_| {
                        for &s in data {
                            an.push(s as f32 / 32768.0, ch, sr);
                        }
                    },
                    err,
                    None,
                ),
                SampleFormat::I32 => dev.build_input_stream(
                    config,
                    move |data: &[i32], _: &_| {
                        for &s in data {
                            an.push(s as f32 / 2_147_483_648.0, ch, sr);
                        }
                    },
                    err,
                    None,
                ),
                _ => Err(first),
            }
        }
    }
    .map_err(|e| e.to_string())?;
    stream.play().map_err(|e| e.to_string())?;
    levels.external.store(true, Ordering::Relaxed);
    tracing::info!("reacting to system audio from {}", name(&dev));
    Ok(Capture {
        _stream: stream,
        levels,
    })
}
