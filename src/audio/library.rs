//! Music folder discovery, scanning and tag reading.

use lofty::file::{AudioFile, TaggedFileExt};
use lofty::tag::Accessor;
use std::path::{Path, PathBuf};
use std::time::Duration;

pub const EXTENSIONS: &[&str] = &["mp3", "ogg", "flac", "wav", "m4a", "aac"];

#[derive(Debug, Clone, PartialEq)]
pub struct Track {
    pub path: PathBuf,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub duration: Duration,
}

/// Primary (created) music dir.
pub fn primary_dir() -> PathBuf {
    #[cfg(windows)]
    {
        let base = std::env::var_os("APPDATA")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("."));
        base.join("snowdesk").join("music")
    }
    #[cfg(not(windows))]
    {
        let base = std::env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")))
            .unwrap_or_else(|| PathBuf::from("."));
        base.join("snowdesk").join("music")
    }
}

/// The user's own music folder (config `music_dir`), if set.
static CUSTOM: std::sync::RwLock<Option<PathBuf>> = std::sync::RwLock::new(None);

pub fn set_custom_dir(d: Option<PathBuf>) {
    *CUSTOM.write().unwrap_or_else(|e| e.into_inner()) = d.filter(|p| p.is_absolute());
}

/// Where new music goes and what the panel shows: the user's folder if
/// they picked one, else ours.
pub fn active_dir() -> PathBuf {
    CUSTOM
        .read()
        .unwrap_or_else(|e| e.into_inner())
        .clone()
        .unwrap_or_else(primary_dir)
}

/// All folders to scan: the user's folder, primary, `music/` next to
/// $APPIMAGE, next to the exe.
pub fn music_dirs() -> Vec<PathBuf> {
    let mut v = vec![active_dir(), primary_dir()];
    if let Some(img) = std::env::var_os("APPIMAGE")
        && let Some(p) = Path::new(&img).parent()
    {
        v.push(p.join("music"));
    }
    if let Ok(exe) = std::env::current_exe()
        && let Some(p) = exe.parent()
    {
        v.push(p.join("music"));
    }
    let mut seen = std::collections::HashSet::new();
    v.retain(|p| seen.insert(p.clone()));
    v
}

/// Copy audio files (or the audio inside dropped folders) into
/// `dest`. Existing files with the same name are kept: the copy gets a
/// " (2)" suffix. Returns how many files were copied.
pub fn import_files(paths: &[PathBuf], dest: &Path) -> std::io::Result<usize> {
    std::fs::create_dir_all(dest)?;
    let mut files = Vec::new();
    for p in paths {
        if p.is_dir() {
            walk(p, &mut files, 0);
        } else if is_audio(p) {
            files.push(p.clone());
        }
    }
    let dest_c = dest.canonicalize().unwrap_or_else(|_| dest.to_path_buf());
    let mut n = 0;
    for f in files {
        // Already in the music folder: nothing to do.
        if f.parent()
            .and_then(|d| d.canonicalize().ok())
            .is_some_and(|d| d.starts_with(&dest_c))
        {
            continue;
        }
        let Some(name) = f.file_name() else { continue };
        let mut target = dest.join(name);
        let stem = f.file_stem().unwrap_or(name).to_string_lossy().into_owned();
        let ext = f.extension().map(|e| e.to_string_lossy().into_owned());
        let mut k = 2;
        while target.exists() {
            let n = match &ext {
                Some(e) => format!("{stem} ({k}).{e}"),
                None => format!("{stem} ({k})"),
            };
            target = dest.join(n);
            k += 1;
        }
        std::fs::copy(&f, &target)?;
        n += 1;
    }
    Ok(n)
}

pub fn ensure_primary_dir() {
    let d = primary_dir();
    if let Err(e) = std::fs::create_dir_all(&d) {
        tracing::warn!("cannot create music dir {}: {e}", d.display());
    }
}

pub fn is_audio(p: &Path) -> bool {
    p.extension()
        .and_then(|e| e.to_str())
        .map(|e| EXTENSIONS.contains(&e.to_ascii_lowercase().as_str()))
        .unwrap_or(false)
}

fn walk(dir: &Path, out: &mut Vec<PathBuf>, depth: u32) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() && depth < 4 {
            walk(&p, out, depth + 1);
        } else if is_audio(&p) {
            out.push(p);
        }
    }
}

/// Find audio files (sorted, de-duplicated) without reading tags.
pub fn find_files(dirs: &[PathBuf]) -> Vec<PathBuf> {
    let mut v = Vec::new();
    for d in dirs {
        walk(d, &mut v, 0);
    }
    v.sort();
    v.dedup();
    v
}

pub fn read_track(path: &Path) -> Track {
    // Lossy: non-UTF-8 file names still get a readable title.
    let fallback = path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "Unknown".into());
    let mut t = Track {
        path: path.to_path_buf(),
        title: fallback,
        artist: String::new(),
        album: String::new(),
        duration: Duration::ZERO,
    };
    if let Ok(f) = lofty::read_from_path(path) {
        t.duration = f.properties().duration();
        if let Some(tag) = f.primary_tag().or_else(|| f.first_tag()) {
            if let Some(s) = tag.title()
                && !s.is_empty()
            {
                t.title = s.to_string();
            }
            t.artist = tag.artist().map(|s| s.to_string()).unwrap_or_default();
            t.album = tag.album().map(|s| s.to_string()).unwrap_or_default();
        }
    }
    t
}

#[cfg(test)]
pub fn scan(dirs: &[PathBuf]) -> Vec<Track> {
    let mut l = Library::default();
    l.rescan(dirs);
    l.tracks
}

fn mtime(p: &Path) -> Option<std::time::SystemTime> {
    std::fs::metadata(p).and_then(|m| m.modified()).ok()
}

/// The track list plus a tag cache, so rescans after a folder change only
/// read tags of new or modified files (matters for large libraries).
#[derive(Default)]
pub struct Library {
    pub tracks: Vec<Track>,
    cache: std::collections::HashMap<PathBuf, (Option<std::time::SystemTime>, Track)>,
}

impl Library {
    pub fn rescan(&mut self, dirs: &[PathBuf]) {
        let files = find_files(dirs);
        let mut cache = std::collections::HashMap::with_capacity(files.len());
        let mut tracks = Vec::with_capacity(files.len());
        for p in files {
            let m = mtime(&p);
            let t = match self.cache.remove(&p) {
                Some((old, t)) if old == m && m.is_some() => t,
                _ => read_track(&p),
            };
            tracks.push(t.clone());
            cache.insert(p, (m, t));
        }
        self.cache = cache;
        self.tracks = tracks;
    }
}

/// Decoded, downscaled cover art (RGBA8).
#[derive(Debug, Clone)]
pub struct Art {
    pub rgba: Vec<u8>,
    pub width: u32,
    pub height: u32,
}

/// Cover side in px: the panel shows 88 logical px, doubled for HiDPI.
const ART_PX: u32 = 176;

/// Embedded cover image, decoded on the audio thread and shrunk so a 3000 px
/// cover does not sit in memory at full size.
pub fn read_art(path: &Path) -> Option<Art> {
    let f = lofty::read_from_path(path).ok()?;
    let tag = f.primary_tag().or_else(|| f.first_tag())?;
    let data = tag.pictures().first()?.data();
    decode_art(data)
}

pub fn decode_art(data: &[u8]) -> Option<Art> {
    let mut r = image::ImageReader::new(std::io::Cursor::new(data))
        .with_guessed_format()
        .ok()?;
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(8192);
    limits.max_image_height = Some(8192);
    limits.max_alloc = Some(256 * 1024 * 1024);
    r.limits(limits);
    let img = r.decode().ok()?.thumbnail(ART_PX, ART_PX).to_rgba8();
    let (width, height) = img.dimensions();
    Some(Art {
        rgba: img.into_raw(),
        width,
        height,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scans_only_audio_files() {
        let d = std::env::temp_dir().join(format!("snowdesk-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("sub")).unwrap();
        for n in ["a.mp3", "b.FLAC", "sub/c.ogg", "notes.txt", "d.jpg"] {
            std::fs::write(d.join(n), b"x").unwrap();
        }
        let f = find_files(std::slice::from_ref(&d));
        let names: Vec<_> = f
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().to_string())
            .collect();
        assert_eq!(names, ["a.mp3", "b.FLAC", "c.ogg"]);
        // Unreadable tags fall back to the file name.
        let tr = scan(std::slice::from_ref(&d));
        assert_eq!(tr[0].title, "a");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn non_utf8_and_unicode_names() {
        let d = std::env::temp_dir().join(format!("snowdesk-names-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(d.join("Пісня ❄ 雪.mp3"), b"x").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::ffi::OsStrExt;
            let raw = std::ffi::OsStr::from_bytes(b"bad\xffname.mp3");
            std::fs::write(d.join(raw), b"x").unwrap();
        }
        let t = scan(std::slice::from_ref(&d));
        assert!(t.iter().any(|t| t.title == "Пісня ❄ 雪"));
        #[cfg(unix)]
        assert!(
            t.iter()
                .any(|t| t.title.starts_with("bad") && t.title.ends_with("name"))
        );
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn rescan_reuses_cache_and_sees_changes() {
        let d = std::env::temp_dir().join(format!("snowdesk-cache-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        for i in 0..1200 {
            std::fs::write(d.join(format!("{i:04}.mp3")), b"not really mp3").unwrap();
        }
        let mut l = Library::default();
        l.rescan(std::slice::from_ref(&d));
        assert_eq!(l.tracks.len(), 1200);
        std::fs::remove_file(d.join("0005.mp3")).unwrap();
        std::fs::write(d.join("new.flac"), b"x").unwrap();
        l.rescan(std::slice::from_ref(&d));
        assert_eq!(l.tracks.len(), 1200);
        assert!(l.tracks.iter().any(|t| t.title == "new"));
        assert!(!l.tracks.iter().any(|t| t.title == "0005"));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn corrupt_files_and_art_do_not_panic() {
        let d = std::env::temp_dir().join(format!("snowdesk-corrupt-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        let p = d.join("broken.mp3");
        let mut junk = b"ID3\x04\x00\x00\x7f\x7f\x7f\x7f".to_vec();
        junk.extend((0..4096).map(|i| (i * 7 % 251) as u8));
        std::fs::write(&p, &junk).unwrap();
        let t = read_track(&p);
        assert_eq!(t.title, "broken");
        assert!(read_art(&p).is_none());
        assert!(decode_art(b"\x89PNG\r\n\x1a\ngarbage").is_none());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn art_is_downscaled() {
        let img = image::RgbaImage::from_pixel(1200, 800, image::Rgba([10, 20, 30, 255]));
        let mut png = Vec::new();
        image::DynamicImage::ImageRgba8(img)
            .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
            .unwrap();
        let a = decode_art(&png).unwrap();
        assert!(a.width <= ART_PX && a.height <= ART_PX);
        assert_eq!(a.rgba.len(), (a.width * a.height * 4) as usize);
    }

    #[test]
    fn import_copies_audio_and_renames_clashes() {
        let base = std::env::temp_dir().join(format!("snowdesk-import-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let (src, dst) = (base.join("src"), base.join("dst"));
        std::fs::create_dir_all(src.join("album")).unwrap();
        std::fs::create_dir_all(&dst).unwrap();
        std::fs::write(src.join("a.mp3"), b"new").unwrap();
        std::fs::write(src.join("cover.jpg"), b"x").unwrap();
        std::fs::write(src.join("album/b.flac"), b"x").unwrap();
        std::fs::write(dst.join("a.mp3"), b"old").unwrap();
        let n = import_files(
            &[src.join("a.mp3"), src.join("cover.jpg"), src.join("album")],
            &dst,
        )
        .unwrap();
        assert_eq!(n, 2);
        assert_eq!(std::fs::read(dst.join("a.mp3")).unwrap(), b"old");
        assert_eq!(std::fs::read(dst.join("a (2).mp3")).unwrap(), b"new");
        assert!(dst.join("b.flac").exists());
        assert!(!dst.join("cover.jpg").exists());
        // Files already inside the folder are not duplicated.
        assert_eq!(import_files(&[dst.join("b.flac")], &dst).unwrap(), 0);
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn empty_folder_is_fine() {
        let d = std::env::temp_dir().join("snowdesk-nonexistent-dir");
        assert!(scan(&[d]).is_empty());
    }
}
