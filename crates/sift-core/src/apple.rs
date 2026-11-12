//! Image decoding through the operating system (ImageIO) on Apple platforms.
//!
//! The system decoder handles formats the bundled Rust decoders cannot
//! (Nikon High Efficiency NEF, HEIF, ...) and runs on Apple's accelerated
//! JPEG and RAW pipelines. RAW files come out with Apple's default rendering,
//! not the in-camera look.

use crate::{Error, Result};
use core_foundation::base::TCFType;
use core_foundation::dictionary::CFDictionary;
use core_foundation::number::CFNumber;
use core_foundation::string::CFString;
use core_foundation::url::CFURL;
use core_foundation_sys::base::{kCFAllocatorDefault, CFRelease, CFTypeRef};
use core_foundation_sys::data::{CFDataCreateMutable, CFDataGetBytePtr, CFDataGetLength, CFMutableDataRef};
use core_foundation_sys::dictionary::CFDictionaryRef;
use core_foundation_sys::string::CFStringRef;
use core_foundation_sys::url::CFURLRef;
use std::path::Path;
use std::ptr;

#[link(name = "ImageIO", kind = "framework")]
extern "C" {
    static kCGImageDestinationLossyCompressionQuality: CFStringRef;
    static kCGImagePropertyOrientation: CFStringRef;
    fn CGImageSourceCreateWithURL(url: CFURLRef, options: CFDictionaryRef) -> CFTypeRef;
    fn CGImageSourceCreateImageAtIndex(source: CFTypeRef, index: usize, options: CFDictionaryRef) -> CFTypeRef;
    fn CGImageDestinationCreateWithData(data: CFMutableDataRef, kind: CFStringRef, count: usize, options: CFDictionaryRef) -> CFTypeRef;
    fn CGImageDestinationAddImage(destination: CFTypeRef, image: CFTypeRef, properties: CFDictionaryRef);
    fn CGImageDestinationFinalize(destination: CFTypeRef) -> bool;
}

/// Releases a Core Foundation object created by a `Create` function.
struct Owned(CFTypeRef);

impl Owned {
    fn new(object: CFTypeRef, what: &str) -> Result<Owned> {
        if object.is_null() {
            return Err(Error::Decode(format!("system decoder: {what}")));
        }
        Ok(Owned(object))
    }
}

impl Drop for Owned {
    fn drop(&mut self) {
        unsafe { CFRelease(self.0) }
    }
}

/// Decodes the image data of `path` (for RAW: the sensor data, not an
/// embedded preview) with the system decoder and returns a full-size JPEG.
/// The pixels stay in sensor orientation; `orientation` is recorded in the
/// JPEG's EXIF so viewers show it upright.
pub fn render_jpeg(path: &Path, orientation: u8, quality: f32) -> Result<Vec<u8>> {
    let url = CFURL::from_path(path, false).ok_or(Error::Malformed)?;
    // SAFETY: every pointer passed below is a live Core Foundation object owned
    // by this function, and each created object is released by `Owned`.
    unsafe {
        let source = Owned::new(CGImageSourceCreateWithURL(url.as_concrete_TypeRef(), ptr::null()), "cannot open file")?;
        let image = Owned::new(CGImageSourceCreateImageAtIndex(source.0, 0, ptr::null()), "format not supported")?;

        let data = CFDataCreateMutable(kCFAllocatorDefault, 0);
        let _data_owner = Owned::new(data as CFTypeRef, "out of memory")?;
        let jpeg = CFString::from_static_string("public.jpeg");
        let destination = Owned::new(CGImageDestinationCreateWithData(data, jpeg.as_concrete_TypeRef(), 1, ptr::null()), "cannot encode")?;
        let key = |k: CFStringRef| CFString::wrap_under_get_rule(k);
        let properties = CFDictionary::from_CFType_pairs(&[
            (key(kCGImageDestinationLossyCompressionQuality), CFNumber::from(quality as f64).as_CFType()),
            (key(kCGImagePropertyOrientation), CFNumber::from(orientation.clamp(1, 8) as i32).as_CFType()),
        ]);
        CGImageDestinationAddImage(destination.0, image.0, properties.as_concrete_TypeRef());
        if !CGImageDestinationFinalize(destination.0) {
            return Err(Error::Decode("system decoder: cannot encode".into()));
        }
        let bytes = std::slice::from_raw_parts(CFDataGetBytePtr(data as _), CFDataGetLength(data as _) as usize).to_vec();
        Ok(bytes)
    }
}
