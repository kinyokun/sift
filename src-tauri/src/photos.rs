//! Saving photos to the system photo library (iOS). A Live Photo goes in as
//! one asset made of its still and its movie; Photos then shows it as a Live
//! Photo, which no file copied to the phone on its own becomes.

use std::path::Path;

/// Why a photo was not saved.
pub enum Failure {
    /// The user has not allowed Sift to add photos.
    Denied,
    Other(String),
}

#[cfg(target_os = "ios")]
pub const SUPPORTED: bool = true;
#[cfg(not(target_os = "ios"))]
pub const SUPPORTED: bool = false;

#[cfg(target_os = "ios")]
pub fn save(still: &Path, movie: Option<&Path>) -> Result<(), Failure> {
    use std::ffi::{c_char, c_int, CStr, CString};
    extern "C" {
        fn sift_photos_save(still: *const c_char, movie: *const c_char, err: *mut c_char, cap: c_int) -> c_int;
    }
    let c = |p: &Path| CString::new(p.to_string_lossy().as_bytes()).map_err(|_| Failure::Other("bad file name".into()));
    let still = c(still)?;
    let movie = movie.map(c).transpose()?;
    let mut message = [0 as c_char; 512];
    // SAFETY: the strings outlive the call and the buffer's size is passed with it.
    let saved = unsafe { sift_photos_save(still.as_ptr(), movie.as_ref().map_or(std::ptr::null(), |m| m.as_ptr()), message.as_mut_ptr(), 512) };
    match saved {
        1 => Ok(()),
        -1 => Err(Failure::Denied),
        // SAFETY: the buffer starts zeroed and the writer keeps it terminated.
        _ => Err(Failure::Other(unsafe { CStr::from_ptr(message.as_ptr()) }.to_string_lossy().into_owned())),
    }
}

#[cfg(not(target_os = "ios"))]
pub fn save(_: &Path, _: Option<&Path>) -> Result<(), Failure> {
    Err(Failure::Other("saving to the photo library is only available on iOS".into()))
}
