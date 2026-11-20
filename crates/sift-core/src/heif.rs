//! HEIF pictures (`.heic` from phones, `.hif` from cameras) as upright sRGB
//! pixels, the form everything else here works on.
//!
//! Apple platforms decode them with the system (`apple::pixels`), which
//! manages colour and brings HDR into range on its own. Elsewhere, and for a
//! file the system gives up on, the bundled decoder does it (feature `heif`).
//! That one hands out the values as coded, so what the file says about its
//! colours (an ICC profile, or the primaries and transfer function it names)
//! is applied ([`crate::color`]). It does not read gain maps: a HEIF with
//! one is an ordinary picture to it.

use crate::hdr::Picture;
use crate::{Error, Result};
use image::RgbImage;
use std::path::Path;

/// Upright sRGB pixels of the HEIF picture at `path`, with a long edge of at
/// most `edge`.
pub fn decode(path: &Path, edge: u32) -> Result<RgbImage> {
    #[cfg(target_vendor = "apple")]
    let system = crate::apple::pixels(path, edge);
    #[cfg(not(target_vendor = "apple"))]
    let system: Result<RgbImage> = Err(Error::Unsupported("HEIF pictures cannot be decoded in this build"));
    #[cfg(feature = "heif")]
    if system.is_err() {
        return bundled(path, edge);
    }
    system
}

/// The HDR HEIF picture at `path` as a [`Picture`] whose ordinary picture is
/// at most `edge` long, the same one [`decode`] gives. `None` when this build
/// cannot read the picture's HDR.
pub fn hdr(path: &Path, edge: u32) -> Result<Option<Picture>> {
    #[cfg(target_vendor = "apple")]
    let system = crate::hdr::rendered(path, edge);
    #[cfg(not(target_vendor = "apple"))]
    let system: Result<Option<Picture>> = Err(Error::Unsupported("HEIF pictures cannot be decoded in this build"));
    #[cfg(feature = "heif")]
    if system.is_err() {
        return bundled_hdr(path, edge);
    }
    system
}

/// What the bundled decoder gives: sRGB pixels, or for a picture coded for
/// HDR screens its values (16 bits a channel) and what they mean.
#[cfg(feature = "heif")]
enum Bundled {
    Sdr(RgbImage),
    Hdr(crate::color::Space, image::ImageBuffer<image::Rgb<u16>, Vec<u16>>),
}

#[cfg(feature = "heif")]
fn bundled_decode(path: &Path, edge: u32) -> Result<Bundled> {
    use heic_decoder::{ColorInfo, RgbOptions};
    use image::imageops;
    let failed = |e: heic_decoder::HeifError| Error::Decode(e.to_string());
    let picture = heic_decoder::decode(&std::fs::read(path)?).map_err(failed)?;
    // A file that does not name its YCbCr matrix is read as BT.601, as the
    // system's decoder on Apple platforms reads it (iPhones named none up to iOS 11).
    let coded = picture.color_info.map(|c| if c.matrix_coefficients == 2 { ColorInfo { matrix_coefficients: 6, ..c } } else { c });
    let options = RgbOptions { color: coded, ..RgbOptions::default() };
    let (primaries, transfer) = coded.map_or((2, 2), |c| (c.color_primaries, c.transfer_characteristics));
    let space = crate::color::Space::of(primaries, transfer, picture.metadata.icc_profile.as_deref());
    let (w, h) = (picture.width as u32, picture.height as u32);
    let long = w.max(h).max(1) as u64;
    let scaled = |side: u32| if long > edge as u64 { ((side as u64 * edge as u64 + long / 2) / long).max(1) as u32 } else { side };
    let (fw, fh) = (scaled(w), scaled(h));
    if !space.is_hdr() {
        let rgb = picture.to_rgb8_with_options(&options).map_err(failed)?;
        drop(picture);
        let mut img = RgbImage::from_raw(w, h, rgb.data).ok_or(Error::Malformed)?;
        if (fw, fh) != (w, h) {
            img = imageops::thumbnail(&img, fw, fh);
        }
        space.to_srgb(&mut img);
        return Ok(Bundled::Sdr(img));
    }
    // HDR keeps its depth until it is brought into the range of a screen.
    let rgb = picture.to_rgb16_with_options(&options).map_err(failed)?;
    drop(picture);
    let mut deep = image::ImageBuffer::<image::Rgb<u16>, Vec<u16>>::from_raw(w, h, rgb.data).ok_or(Error::Malformed)?;
    if (fw, fh) != (w, h) {
        deep = imageops::thumbnail(&deep, fw, fh);
    }
    Ok(Bundled::Hdr(space, deep))
}

/// [`decode`] by the bundled decoder alone.
#[cfg(feature = "heif")]
pub fn bundled(path: &Path, edge: u32) -> Result<RgbImage> {
    match bundled_decode(path, edge)? {
        Bundled::Sdr(img) => Ok(img),
        Bundled::Hdr(space, deep) => RgbImage::from_raw(deep.width(), deep.height(), space.tone_map(deep.as_raw())).ok_or(Error::Malformed),
    }
}

/// [`hdr`] by the bundled decoder alone.
#[cfg(feature = "heif")]
pub fn bundled_hdr(path: &Path, edge: u32) -> Result<Option<Picture>> {
    let Bundled::Hdr(space, deep) = bundled_decode(path, edge)? else { return Ok(None) };
    let base = RgbImage::from_raw(deep.width(), deep.height(), space.tone_map(deep.as_raw())).ok_or(Error::Malformed)?;
    let (w, h) = Picture::gain_size(deep.width(), deep.height());
    let small = image::imageops::resize(&deep, w, h, image::imageops::FilterType::Triangle);
    Ok(Some(Picture::from_renditions(base, &space.light(small.as_raw()), w, h)))
}

#[cfg(all(test, feature = "heif"))]
mod tests {
    use super::*;

    /// Four plain quarters, stored 64x40 and lying on its side, with the
    /// instruction to show it turned a quarter (drawn for this test; x265, heif-enc).
    const TURNED: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/assets/turned.heic");

    fn quarters(img: &RgbImage) -> [[bool; 3]; 4] {
        let (w, h) = img.dimensions();
        [(w / 4, h / 4), (3 * w / 4, h / 4), (w / 4, 3 * h / 4), (3 * w / 4, 3 * h / 4)].map(|(x, y)| img.get_pixel(x, y).0.map(|v| v > 128))
    }

    const UPRIGHT: [[bool; 3]; 4] = [[false, true, false], [true, true, false], [true, false, false], [false, false, true]];

    #[test]
    fn the_bundled_decoder_gives_upright_pixels() {
        let img = bundled(Path::new(TURNED), u32::MAX).unwrap();
        assert_eq!(img.dimensions(), (40, 64));
        assert_eq!(quarters(&img), UPRIGHT);
        let small = bundled(Path::new(TURNED), 32).unwrap();
        assert_eq!(small.dimensions(), (20, 32));
        assert_eq!(quarters(&small), UPRIGHT);
    }

    #[test]
    fn whichever_decoder_is_used_agrees_with_the_index() {
        let img = decode(Path::new(TURNED), u32::MAX).unwrap();
        assert_eq!(img.dimensions(), (40, 64));
        assert_eq!(quarters(&img), UPRIGHT);
        let src = crate::io::Src::open(Path::new(TURNED)).unwrap();
        let index = crate::container::parse(&src).heif.unwrap();
        assert_eq!((index.width, index.height, index.orientation), (64, 40, 8));
    }
}
