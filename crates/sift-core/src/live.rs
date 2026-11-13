//! A burst written out as an Apple Live Photo: a JPEG still and a QuickTime
//! movie of the frames, tied together by a shared content identifier (in the
//! still's Apple maker note and in the movie's metadata; the movie also marks
//! the moment the still shows).
//!
//! The movie is encoded by the operating system (AVFoundation, `live.m`), so
//! this exists on Apple platforms only.

use crate::{preview, Error, Result};
use image::{imageops, RgbImage};
use std::ffi::{c_char, c_int, c_long, c_void, CStr, CString};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

/// Long edge of the movie; Live Photos from phones are about this size.
pub const MOVIE_EDGE: u32 = 1920;
/// A burst is at least this many frames before it is worth a movie.
pub const MIN_FRAMES: usize = 3;
/// More frames than this are not written (about half a minute at the slowest pace).
pub const MAX_FRAMES: usize = 600;
/// How long a frame stays up when the files do not say when they were taken.
const DEFAULT_STEP: f64 = 0.1;
/// Frames are shown at the pace they were shot, within these bounds, so a
/// slow burst does not turn into a slide show and a fast one stays watchable.
const STEP_RANGE: (f64, f64) = (1.0 / 30.0, 0.25);

extern "C" {
    fn sift_live_begin(path: *const c_char, width: c_int, height: c_int, identifier: *const c_char, err: *mut c_char, cap: c_int) -> *mut c_void;
    fn sift_live_frame(handle: *mut c_void, bgra: *const u8, seconds: f64, err: *mut c_char, cap: c_int) -> c_int;
    fn sift_live_finish(handle: *mut c_void, still: f64, duration: f64, err: *mut c_char, cap: c_int) -> c_int;
    fn sift_live_abort(handle: *mut c_void);
    fn sift_live_still(jpeg: *const u8, length: c_long, path: *const c_char, identifier: *const c_char, err: *mut c_char, cap: c_int) -> c_int;
}

/// Room for the system's error message.
struct Message([c_char; 512]);

impl Message {
    fn new() -> Message {
        Message([0; 512])
    }
    fn ptr(&mut self) -> *mut c_char {
        self.0.as_mut_ptr()
    }
    fn error(&self) -> Error {
        // SAFETY: the buffer starts zeroed and the writer keeps it terminated.
        let text = unsafe { CStr::from_ptr(self.0.as_ptr()) }.to_string_lossy().into_owned();
        Error::Decode(if text.is_empty() { "the system could not write the Live Photo".into() } else { text })
    }
}

fn c_path(path: &Path) -> Result<CString> {
    CString::new(path.to_string_lossy().as_bytes()).map_err(|_| Error::Malformed)
}

/// An identifier in the form Photos writes: an upper-case version 4 UUID.
/// It only has to be the same in both files and unlikely to occur twice.
fn identifier(seed: &Path) -> String {
    use std::hash::{BuildHasher, Hasher};
    let mut bytes = [0u8; 16];
    for (i, half) in bytes.chunks_mut(8).enumerate() {
        // `RandomState` is seeded by the system for every instance.
        let mut h = std::collections::hash_map::RandomState::new().build_hasher();
        h.write(seed.to_string_lossy().as_bytes());
        h.write_usize(i);
        h.write_u128(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos()));
        half.copy_from_slice(&h.finish().to_le_bytes());
    }
    bytes[6] = (bytes[6] & 0x0F) | 0x40;
    bytes[8] = (bytes[8] & 0x3F) | 0x80;
    let hex: String = bytes.iter().map(|b| format!("{b:02X}")).collect();
    format!("{}-{}-{}-{}-{}", &hex[..8], &hex[8..12], &hex[12..16], &hex[16..20], &hex[20..])
}

/// When each frame appears, in seconds from the first, and when the movie ends.
/// `taken` holds the capture times in milliseconds where the files record them.
pub fn timeline(taken: &[Option<i64>]) -> (Vec<f64>, f64) {
    let mut gaps: Vec<f64> = taken.windows(2).filter_map(|w| Some((w[1]? - w[0]?) as f64 / 1000.0)).filter(|g| *g > 0.0).collect();
    gaps.sort_by(f64::total_cmp);
    // One pace for the whole movie: the usual gap, so a hiccup does not freeze it.
    let step = gaps.get(gaps.len() / 2).copied().unwrap_or(DEFAULT_STEP).clamp(STEP_RANGE.0, STEP_RANGE.1);
    ((0..taken.len()).map(|i| i as f64 * step).collect(), taken.len() as f64 * step)
}

/// The frame as the movie shows it: upright, `width` x `height`, BGRA.
fn movie_frame(path: &Path, size: Option<(u32, u32)>) -> Result<(RgbImage, Vec<u8>)> {
    let img = preview::pixels(path, MOVIE_EDGE)?;
    // The encoder wants even sides; every frame takes the size of the first.
    let (w, h) = size.unwrap_or((img.width() & !1, img.height() & !1));
    if w < 2 || h < 2 {
        return Err(Error::Malformed);
    }
    let img = if (img.width(), img.height()) == (w, h) { img } else { imageops::resize(&img, w, h, imageops::FilterType::Triangle) };
    let mut bgra = Vec::with_capacity((w * h * 4) as usize);
    for p in img.pixels() {
        bgra.extend_from_slice(&[p[2], p[1], p[0], 255]);
    }
    Ok((img, bgra))
}

/// Writes `frames` (one displayable file per shot, in shooting order) as
/// `<stem>.JPG` and `<stem>.MOV` in `dest`. `key` is the frame that becomes
/// the still. Returns the two paths; nothing is left behind on failure.
pub fn export(frames: &[PathBuf], taken: &[Option<i64>], key: usize, dest: &Path, stem: &str, cancel: &AtomicBool, mut on_frame: impl FnMut(usize)) -> Result<(PathBuf, PathBuf)> {
    if frames.len() < MIN_FRAMES {
        return Err(Error::Unsupported("a Live Photo needs a burst of at least three frames"));
    }
    if frames.len() > MAX_FRAMES {
        return Err(Error::Unsupported("too many frames for one Live Photo"));
    }
    let key = key.min(frames.len() - 1);
    let (still_path, movie_path) = (dest.join(format!("{stem}.JPG")), dest.join(format!("{stem}.MOV")));
    let id = CString::new(identifier(&frames[key])).map_err(|_| Error::Malformed)?;
    let (times, duration) = timeline(taken);
    let mut message = Message::new();

    let made = (|| -> Result<()> {
        let (first, mut bgra) = movie_frame(&frames[0], None)?;
        let size = (first.width(), first.height());
        // SAFETY: the strings outlive the calls, `bgra` holds width x height x 4
        // bytes as the writer was told, and the handle is finished or aborted once.
        unsafe {
            let handle = sift_live_begin(c_path(&movie_path)?.as_ptr(), size.0 as c_int, size.1 as c_int, id.as_ptr(), message.ptr(), 512);
            if handle.is_null() {
                return Err(message.error());
            }
            for (i, frame) in frames.iter().enumerate() {
                if cancel.load(Ordering::Relaxed) {
                    sift_live_abort(handle);
                    return Err(Error::Cancelled);
                }
                on_frame(i);
                if i > 0 {
                    bgra = match movie_frame(frame, Some(size)) {
                        Ok((_, pixels)) => pixels,
                        Err(e) => {
                            sift_live_abort(handle);
                            return Err(e);
                        }
                    };
                }
                if sift_live_frame(handle, bgra.as_ptr(), times[i], message.ptr(), 512) == 0 {
                    sift_live_abort(handle);
                    return Err(message.error());
                }
            }
            // The still's moment is the middle of the time its frame is shown.
            let still = times[key] + (duration / frames.len() as f64) / 2.0;
            if sift_live_finish(handle, still, duration, message.ptr(), 512) == 0 {
                return Err(message.error());
            }
            let jpeg = preview::full(&frames[key])?;
            if sift_live_still(jpeg.as_ptr(), jpeg.len() as c_long, c_path(&still_path)?.as_ptr(), id.as_ptr(), message.ptr(), 512) == 0 {
                return Err(message.error());
            }
        }
        Ok(())
    })();
    match made {
        Ok(()) => Ok((still_path, movie_path)),
        Err(e) => {
            let _ = std::fs::remove_file(&still_path);
            let _ = std::fs::remove_file(&movie_path);
            Err(e)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identifiers_look_like_uuids_and_differ() {
        let (a, b) = (identifier(Path::new("a.jpg")), identifier(Path::new("a.jpg")));
        assert_eq!(a.len(), 36);
        assert_eq!(a.as_bytes()[14], b'4');
        assert!(a.chars().all(|c| c == '-' || c.is_ascii_digit() || c.is_ascii_uppercase()));
        assert_ne!(a, b);
    }

    #[test]
    fn the_movie_keeps_the_pace_of_the_burst_within_bounds() {
        // Seven frames a second, with one gap twice as long.
        let (times, duration) = timeline(&[Some(0), Some(140), Some(280), Some(560), Some(700)]);
        assert!((times[1] - 0.14).abs() < 1e-9 && (times[4] - 0.56).abs() < 1e-9);
        assert!((duration - 0.7).abs() < 1e-9);
        // One frame a second would be a slide show; 120 a second too fast to see.
        assert!((timeline(&[Some(0), Some(1000), Some(2000)]).1 - 0.75).abs() < 1e-9);
        assert!((timeline(&[Some(0), Some(8), Some(16)]).1 - 0.1).abs() < 1e-9);
        // No capture times at all.
        assert!((timeline(&[None, None, None]).1 - 0.3).abs() < 1e-9);
    }
}
