//! The still of a Live Photo as a HEIC where the system has no encoder for
//! one (Windows; on Apple platforms ImageIO writes it, `live.m`). The picture
//! of a JPEG is encoded again as HEVC by the `still265` crate, which is pure
//! Rust, and put in a HEIF file with the JPEG's EXIF block, the content
//! identifier in it (`still::exif`). The pixels stay as the JPEG stores them
//! and the file says how they are turned, as an iPhone's does.

use crate::{Error, Result};

/// The quantiser, 0 to 51. At 22 a camera picture of 24 to 45 megapixels
/// comes out about the size ImageIO gives it at quality 0.8 (`live.m`, the
/// size an iPhone's own HEIC has), and as close to the JPEG: 39 to 41 dB.
#[cfg(not(any(target_os = "ios", target_os = "android")))]
const QP: u8 = 22;

/// A HEIC of the picture in `jpeg` that carries `identifier`, which makes it
/// the still of the Live Photo whose movie has the same identifier. The
/// picture is taken as sRGB, like everywhere here: a colour profile in the
/// JPEG is not carried over. A picture of 45 megapixels takes a few seconds
/// on every core and about a gigabyte of memory.
#[cfg(not(any(target_os = "ios", target_os = "android")))]
pub fn encode(jpeg: &[u8], identifier: &str) -> Result<Vec<u8>> {
    use still265::{ColorSpace, Effort, HeicEncodeOptions, Image, ImageOrientation, RustStillHevcEncoder};
    let exif = super::still::exif(jpeg, identifier)?;
    // The crate has the two plain mirrorings the other way round: what it
    // writes for "horizontal" is read as top and bottom swapped, by Apple's
    // decoder too. The test below holds all eight against both decoders.
    let orientation = match super::still::orientation(&exif) {
        Some(2) => ImageOrientation::MirrorVertical,
        Some(3) => ImageOrientation::Rotate180,
        Some(4) => ImageOrientation::MirrorHorizontal,
        Some(5) => ImageOrientation::Transpose,
        Some(6) => ImageOrientation::Rotate90,
        Some(7) => ImageOrientation::Transverse,
        Some(8) => ImageOrientation::Rotate270,
        _ => ImageOrientation::Normal,
    };
    let pixels = crate::preview::decode_jpeg(jpeg, u32::MAX)?;
    let encode = move || {
        let image = Image::from_rgb8(pixels.as_raw(), pixels.width(), pixels.height(), ColorSpace::Srgb, false, 8);
        drop(pixels);
        still265::encode_heic(image, &RustStillHevcEncoder::new(Effort::Fast), QP, HeicEncodeOptions { exif: Some(exif), orientation, ..Default::default() })
    };
    // The encoder is young. Should it stumble over a picture, that is an
    // error like any other, not the end of the program.
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(encode)) {
        Ok(Ok(file)) => Ok(file),
        Ok(Err(e)) => Err(Error::Decode(format!("the HEIC encoder: {e}"))),
        Err(_) => Err(Error::Decode("the HEIC encoder gave up on this picture".into())),
    }
}

/// Phones make no Live Photos here, so their builds leave the encoder out.
#[cfg(any(target_os = "ios", target_os = "android"))]
pub fn encode(_: &[u8], _: &str) -> Result<Vec<u8>> {
    Err(Error::Unsupported("a HEIC still cannot be written on this system"))
}

#[cfg(all(test, not(any(target_os = "ios", target_os = "android"))))]
mod tests {
    use super::super::still::tests::{camera, with_exif};
    use super::*;
    use crate::io::Src;
    use crate::{container, exif, preview};
    use image::RgbImage;

    const ID: &str = "58813388-F52F-4098-9F81-35A7A9D1CF94";

    /// A picture that looks different from every side: two gradients and a
    /// bright block in one corner. The sides are odd, as a crop's may be.
    fn picture() -> RgbImage {
        RgbImage::from_fn(201, 119, |x, y| if x < 40 && y < 30 { image::Rgb([250, 240, 60]) } else { image::Rgb([(x * 255 / 201) as u8, (y * 255 / 119) as u8, 90]) })
    }

    /// How close two pictures are, in dB.
    fn closeness(a: &RgbImage, b: &RgbImage) -> f64 {
        assert_eq!(a.dimensions(), b.dimensions());
        let squares: f64 = a.iter().zip(b.iter()).map(|(x, y)| (*x as f64 - *y as f64).powi(2)).sum();
        10.0 * (255.0 * 255.0 * a.len() as f64 / squares.max(1e-9)).log10()
    }

    #[test]
    fn a_jpeg_becomes_a_heic_that_reads_back_the_same() {
        let dir = std::env::temp_dir().join(format!("sift-live-heic-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let jpeg = preview::encode_jpeg(&picture(), 95).unwrap();
        for orientation in 1..=8u8 {
            let turned = [&[0xFF, 0xD8][..], &crate::jpeg::orientation_app1(orientation), &jpeg[2..]].concat();
            let path = dir.join(format!("turned-{orientation}.heic"));
            std::fs::write(&path, encode(&turned, ID).unwrap()).unwrap();

            let src = Src::open(&path).unwrap();
            let p = container::parse(&src);
            assert_eq!(p.format, container::Format::Heif);
            assert_eq!(exif::orientation(&p), orientation);
            assert!(super::super::still_has(&path, ID).unwrap());
            // Whoever decodes it sees the picture the JPEG shows: the system
            // on Apple platforms, the bundled decoder elsewhere, and here both.
            let shown = preview::decode_upright(&turned).unwrap();
            let read = crate::heif::decode(&path, u32::MAX).unwrap();
            assert!(closeness(&read, &shown) > 34.0, "orientation {orientation}: {} dB", closeness(&read, &shown));
            #[cfg(feature = "heif")]
            {
                let read = crate::heif::bundled(&path, u32::MAX).unwrap();
                assert!(closeness(&read, &shown) > 34.0, "orientation {orientation}, bundled: {} dB", closeness(&read, &shown));
            }
        }

        // What the camera recorded comes along, its maker note excepted.
        let path = dir.join("camera.heic");
        std::fs::write(&path, encode(&with_exif(&jpeg, &camera(true, Some(&[b'N'; 120]))), ID).unwrap()).unwrap();
        let src = Src::open(&path).unwrap();
        let p = container::parse(&src);
        let read = exif::read(&p);
        assert_eq!((read.make.as_deref(), read.datetime.as_deref(), exif::orientation(&p)), (Some("Canon"), Some("2024-10-20 07:50:17"), 6));
        let note = p.maker.and_then(|note| src.read(note.pos, note.len)).unwrap();
        assert!(note.starts_with(b"Apple iOS\0") && !note.contains(&b'N'));
        assert!(encode(b"\x89PNG", ID).is_err());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
