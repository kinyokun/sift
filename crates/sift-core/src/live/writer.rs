//! A burst written out as a Live Photo: a still (JPEG or HEIC) and a
//! QuickTime movie of the frames, with the same content identifier in both;
//! the movie also marks the moment the still shows. The two are saved side by
//! side under one name, the form every way of moving files keeps and Photos
//! imports as one picture.
//!
//! Both files are written by the operating system (AVFoundation and ImageIO,
//! `live.m`), so this exists on Apple platforms only.

use super::{enough, frame, identifier, picture, staged, timeline, Still};
use crate::{formats, Error, Result};
use image::RgbImage;
use std::ffi::{c_char, c_int, c_long, c_void, CStr, CString};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

extern "C" {
    fn sift_live_begin(path: *const c_char, width: c_int, height: c_int, identifier: *const c_char, err: *mut c_char, cap: c_int) -> *mut c_void;
    fn sift_live_frame(handle: *mut c_void, bgra: *const u8, seconds: f64, err: *mut c_char, cap: c_int) -> c_int;
    fn sift_live_finish(handle: *mut c_void, still: f64, duration: f64, err: *mut c_char, cap: c_int) -> c_int;
    fn sift_live_abort(handle: *mut c_void);
    fn sift_live_still(picture: *const u8, length: c_long, heic: c_int, path: *const c_char, identifier: *const c_char, err: *mut c_char, cap: c_int) -> c_int;
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

/// The pixels as the writer takes them: BGRA.
fn bgra(img: &RgbImage) -> Vec<u8> {
    let mut out = Vec::with_capacity((img.width() * img.height() * 4) as usize);
    for p in img.pixels() {
        out.extend_from_slice(&[p[2], p[1], p[0], 255]);
    }
    out
}

/// Writes `frames` (one displayable file per shot, in shooting order) as
/// `<stem>.JPG` (or `.HEIC`, as `still` says) and `<stem>.MOV` in `dest`.
/// `key` is the frame that becomes the still. Returns the paths of the still
/// and the movie; nothing is left behind on failure, and existing files of
/// those names are never replaced.
#[allow(clippy::too_many_arguments)]
pub fn export(frames: &[PathBuf], taken: &[Option<i64>], key: usize, format: Still, dest: &Path, stem: &str, cancel: &AtomicBool, mut on_frame: impl FnMut(usize)) -> Result<(PathBuf, PathBuf)> {
    enough(frames)?;
    let key = key.min(frames.len() - 1);
    staged(dest, stem, format, |still_path, movie_path| {
        let id = CString::new(identifier(&frames[key])).map_err(|_| Error::Malformed)?;
        let (times, duration) = timeline(taken);
        let mut message = Message::new();
        let first = frame(&frames[0], None)?;
        let size = first.dimensions();
        let mut pixels = bgra(&first);
        // SAFETY: the strings outlive the calls, `pixels` holds width x height x 4
        // bytes as the writer was told, and the handle is finished or aborted once.
        unsafe {
            let handle = sift_live_begin(c_path(movie_path)?.as_ptr(), size.0 as c_int, size.1 as c_int, id.as_ptr(), message.ptr(), 512);
            if handle.is_null() {
                return Err(message.error());
            }
            for (i, path) in frames.iter().enumerate() {
                if cancel.load(Ordering::Relaxed) {
                    sift_live_abort(handle);
                    return Err(Error::Cancelled);
                }
                on_frame(i);
                if i > 0 {
                    pixels = match frame(path, Some(size)) {
                        Ok(img) => bgra(&img),
                        Err(e) => {
                            sift_live_abort(handle);
                            return Err(e);
                        }
                    };
                }
                if sift_live_frame(handle, pixels.as_ptr(), times[i], message.ptr(), 512) == 0 {
                    sift_live_abort(handle);
                    return Err(message.error());
                }
            }
            // The still's moment is the middle of the time its frame is shown.
            let moment = times[key] + (duration / frames.len() as f64) / 2.0;
            if sift_live_finish(handle, moment, duration, message.ptr(), 512) == 0 {
                return Err(message.error());
            }
            // A HEIF goes to the system as it is, which reads it with everything it
            // records; of other files, the JPEG that is shown.
            let picture = if formats::is_heif(&frames[key]) { std::fs::read(&frames[key])? } else { picture(&frames[key])? };
            let heic = (format == Still::Heic) as c_int;
            if sift_live_still(picture.as_ptr(), picture.len() as c_long, heic, c_path(still_path)?.as_ptr(), id.as_ptr(), message.ptr(), 512) == 0 {
                return Err(message.error());
            }
        }
        Ok(())
    })
}
