//! Image decoding through the operating system (ImageIO) on Apple platforms.
//!
//! The system decoder handles formats the bundled Rust decoders cannot
//! (Nikon High Efficiency NEF, HEIF, ...) and runs on Apple's accelerated
//! JPEG and RAW pipelines. RAW files come out with Apple's default rendering,
//! not the in-camera look.

use crate::{Error, Result};
use core_foundation::base::TCFType;
use core_foundation::boolean::CFBoolean;
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
    static kCGImageSourceCreateThumbnailFromImageAlways: CFStringRef;
    static kCGImageSourceCreateThumbnailWithTransform: CFStringRef;
    static kCGImageSourceDecodeRequest: CFStringRef;
    static kCGImageSourceDecodeToHDR: CFStringRef;
    static kCGImageSourceThumbnailMaxPixelSize: CFStringRef;
    fn CGImageSourceCreateThumbnailAtIndex(source: CFTypeRef, index: usize, options: CFDictionaryRef) -> CFTypeRef;
    fn CGImageSourceCreateWithURL(url: CFURLRef, options: CFDictionaryRef) -> CFTypeRef;
    fn CGImageSourceCreateImageAtIndex(source: CFTypeRef, index: usize, options: CFDictionaryRef) -> CFTypeRef;
    fn CGImageDestinationCreateWithData(data: CFMutableDataRef, kind: CFStringRef, count: usize, options: CFDictionaryRef) -> CFTypeRef;
    fn CGImageDestinationAddImage(destination: CFTypeRef, image: CFTypeRef, properties: CFDictionaryRef);
    fn CGImageDestinationFinalize(destination: CFTypeRef) -> bool;
}

#[repr(C)]
struct CGRect {
    x: f64,
    y: f64,
    width: f64,
    height: f64,
}

/// `kCGImageAlphaNoneSkipLast`: RGBX, the plainest layout CoreGraphics draws into.
const RGBX: u32 = 5;
/// Four 32-bit floats a pixel, little-endian, alpha last and premultiplied.
const FLOAT_RGBA: u32 = 1 | 1 << 8 | 2 << 12;

#[link(name = "CoreGraphics", kind = "framework")]
extern "C" {
    static kCGColorSpaceSRGB: CFStringRef;
    static kCGColorSpaceExtendedLinearSRGB: CFStringRef;
    fn CGColorSpaceCreateWithName(name: CFStringRef) -> CFTypeRef;
    fn CGImageGetWidth(image: CFTypeRef) -> usize;
    fn CGImageGetHeight(image: CFTypeRef) -> usize;
    fn CGImageCreateWithImageInRect(image: CFTypeRef, rect: CGRect) -> CFTypeRef;
    fn CGBitmapContextCreate(data: *mut std::ffi::c_void, width: usize, height: usize, bits: usize, row_bytes: usize, space: CFTypeRef, info: u32) -> CFTypeRef;
    fn CGContextDrawImage(context: CFTypeRef, rect: CGRect, image: CFTypeRef);
    fn CGBitmapContextCreateImage(context: CFTypeRef) -> CFTypeRef;
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
/// embedded preview) with the system decoder and returns a full-size sRGB
/// JPEG. The pixels stay in sensor orientation; `orientation` is recorded in
/// the JPEG's EXIF so viewers show it upright. `crop` is the picture the file
/// records, for an RW2 the system develops whole (some it cuts itself).
///
/// The system develops RAW files into Display P3. They are drawn into sRGB,
/// the colours everything here works on (`color`).
pub fn render_jpeg(path: &Path, orientation: u8, crop: Option<crate::container::RecordedCrop>, quality: f32) -> Result<Vec<u8>> {
    let url = CFURL::from_path(path, false).ok_or(Error::Malformed)?;
    // SAFETY: every pointer passed below is a live Core Foundation object owned
    // by this function, and each created object is released by `Owned`.
    unsafe {
        let source = Owned::new(CGImageSourceCreateWithURL(url.as_concrete_TypeRef(), ptr::null()), "cannot open file")?;
        let mut decoded = Owned::new(CGImageSourceCreateImageAtIndex(source.0, 0, ptr::null()), "format not supported")?;
        if let Some(c) = crop.filter(|c| c.fits(CGImageGetWidth(decoded.0) as u32, CGImageGetHeight(decoded.0) as u32)) {
            let (x, y, w, h) = c.rect(CGImageGetWidth(decoded.0) as u32, CGImageGetHeight(decoded.0) as u32);
            let rect = CGRect { x: x as f64, y: y as f64, width: w as f64, height: h as f64 };
            decoded = Owned::new(CGImageCreateWithImageInRect(decoded.0, rect), "cannot crop")?;
        }
        let srgb = Owned::new(CGColorSpaceCreateWithName(kCGColorSpaceSRGB), "no sRGB colour space")?;
        let (width, height) = (CGImageGetWidth(decoded.0), CGImageGetHeight(decoded.0));
        let canvas = Owned::new(CGBitmapContextCreate(ptr::null_mut(), width, height, 8, 0, srgb.0, RGBX), "out of memory")?;
        CGContextDrawImage(canvas.0, CGRect { x: 0.0, y: 0.0, width: width as f64, height: height as f64 }, decoded.0);
        let image = Owned::new(CGBitmapContextCreateImage(canvas.0), "cannot convert to sRGB")?;

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

/// Upright sRGB pixels of a picture the system can decode (HEIF), with a
/// long edge of at most `edge`. The system turns the picture as the file
/// asks, converts its colours (phones record Display P3) and brings HDR into
/// the range of an ordinary screen.
pub fn pixels(path: &Path, edge: u32) -> Result<image::RgbImage> {
    let url = CFURL::from_path(path, false).ok_or(Error::Malformed)?;
    // SAFETY: every pointer passed below is a live Core Foundation object owned
    // by this function, each created object is released by `Owned`, and the
    // canvas draws into `rgbx`, which holds width x height x 4 bytes and outlives it.
    unsafe {
        let source = Owned::new(CGImageSourceCreateWithURL(url.as_concrete_TypeRef(), ptr::null()), "cannot open file")?;
        let key = |k: CFStringRef| CFString::wrap_under_get_rule(k);
        let yes = || CFBoolean::true_value().as_CFType();
        let mut options = vec![(key(kCGImageSourceCreateThumbnailFromImageAlways), yes()), (key(kCGImageSourceCreateThumbnailWithTransform), yes())];
        if let Ok(edge) = i32::try_from(edge) {
            options.push((key(kCGImageSourceThumbnailMaxPixelSize), CFNumber::from(edge).as_CFType()));
        }
        let options = CFDictionary::from_CFType_pairs(&options);
        let decoded = Owned::new(CGImageSourceCreateThumbnailAtIndex(source.0, 0, options.as_concrete_TypeRef()), "format not supported")?;
        let (width, height) = (CGImageGetWidth(decoded.0), CGImageGetHeight(decoded.0));
        let mut rgbx = vec![0u8; width * height * 4];
        let srgb = Owned::new(CGColorSpaceCreateWithName(kCGColorSpaceSRGB), "no sRGB colour space")?;
        let canvas = Owned::new(CGBitmapContextCreate(rgbx.as_mut_ptr().cast(), width, height, 8, width * 4, srgb.0, RGBX), "out of memory")?;
        CGContextDrawImage(canvas.0, CGRect { x: 0.0, y: 0.0, width: width as f64, height: height as f64 }, decoded.0);
        drop(canvas);
        let rgb: Vec<u8> = rgbx.as_chunks::<4>().0.iter().flat_map(|p| [p[0], p[1], p[2]]).collect();
        image::RgbImage::from_raw(width as u32, height as u32, rgb).ok_or(Error::Malformed)
    }
}

/// The light of an HDR picture (HEIF coded for HDR screens, or one with a
/// gain map) upright, with a long edge of at most `edge`: linear sRGB, three
/// values a pixel, in which 1 is the white of an ordinary picture.
pub fn light(path: &Path, edge: u32) -> Result<(Vec<f32>, u32, u32)> {
    let url = CFURL::from_path(path, false).ok_or(Error::Malformed)?;
    // SAFETY: as in `pixels`; the canvas draws into `rgba`, which holds
    // width x height x 4 floats and outlives it.
    unsafe {
        let source = Owned::new(CGImageSourceCreateWithURL(url.as_concrete_TypeRef(), ptr::null()), "cannot open file")?;
        let key = |k: CFStringRef| CFString::wrap_under_get_rule(k);
        let yes = || CFBoolean::true_value().as_CFType();
        let mut options = vec![
            (key(kCGImageSourceCreateThumbnailFromImageAlways), yes()),
            (key(kCGImageSourceCreateThumbnailWithTransform), yes()),
            (key(kCGImageSourceDecodeRequest), key(kCGImageSourceDecodeToHDR).as_CFType()),
        ];
        if let Ok(edge) = i32::try_from(edge) {
            options.push((key(kCGImageSourceThumbnailMaxPixelSize), CFNumber::from(edge).as_CFType()));
        }
        let options = CFDictionary::from_CFType_pairs(&options);
        let decoded = Owned::new(CGImageSourceCreateThumbnailAtIndex(source.0, 0, options.as_concrete_TypeRef()), "format not supported")?;
        let (width, height) = (CGImageGetWidth(decoded.0), CGImageGetHeight(decoded.0));
        let mut rgba = vec![0f32; width * height * 4];
        let space = Owned::new(CGColorSpaceCreateWithName(kCGColorSpaceExtendedLinearSRGB), "no linear sRGB colour space")?;
        let canvas = Owned::new(CGBitmapContextCreate(rgba.as_mut_ptr().cast(), width, height, 32, width * 16, space.0, FLOAT_RGBA), "out of memory")?;
        CGContextDrawImage(canvas.0, CGRect { x: 0.0, y: 0.0, width: width as f64, height: height as f64 }, decoded.0);
        drop(canvas);
        let rgb = rgba.as_chunks::<4>().0.iter().flat_map(|p| [p[0], p[1], p[2]]).collect();
        Ok((rgb, width as u32, height as u32))
    }
}
