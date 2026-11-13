//! Display images: the embedded full-size JPEG passed through untouched,
//! plus scaled renditions for grids and small screens.

use crate::container::{self, Format};
use crate::io::Src;
use crate::tiff::{Tiff, T_ORIENTATION};
use crate::{exif, formats, jpeg, Error, Result};
use image::{imageops, DynamicImage, RgbImage};
use std::io::Cursor;
use std::path::Path;

/// Makes `data` (a JPEG) carry `orientation` in its EXIF so every viewer
/// rotates it the same way. RAW previews are stored unrotated and usually
/// without EXIF; the orientation lives in the container.
fn tag_orientation(data: Vec<u8>, orientation: u8) -> Vec<u8> {
    let src = Src::mem(data);
    let slot = jpeg::scan(&src, 0).and_then(|info| {
        let (tiff, off) = Tiff::open(&src, info.exif_base?)?;
        let e = tiff.ifd(off)?.entries.into_iter().find(|e| e.tag == T_ORIENTATION && e.typ == 3)?;
        Some((e.pos as usize + 8, tiff.le))
    });
    let Src::Mem(mut bytes) = src else { unreachable!() };
    match slot {
        Some((pos, le)) => {
            let v = orientation as u16;
            bytes[pos..pos + 2].copy_from_slice(&if le { v.to_le_bytes() } else { v.to_be_bytes() });
        }
        None if orientation != 1 => {
            bytes.splice(2..2, jpeg::orientation_app1(orientation));
        }
        None => {}
    }
    bytes
}

/// Bytes a webview can show at full resolution, already tagged upright.
pub fn full(path: &Path) -> Result<Vec<u8>> {
    let src = Src::open(path)?;
    let p = container::parse(&src);
    match p.format {
        Format::Jpeg => Ok(std::fs::read(path)?),
        Format::Other => match formats::ext_of(path).as_str() {
            "png" | "webp" | "bmp" | "gif" => Ok(std::fs::read(path)?),
            _ => encode_jpeg(&image::open(path)?.to_rgb8(), 92),
        },
        _ => {
            let Some(j) = p.largest_preview() else {
                // A plain TIFF image has no embedded JPEG; decode it instead.
                if !formats::is_raw(path) {
                    return encode_jpeg(&image::open(path)?.to_rgb8(), 92);
                }
                return Err(Error::NoPreview);
            };
            let data = src.read(j.offset, j.len as usize).ok_or(Error::Malformed)?;
            Ok(tag_orientation(data, exif::orientation(&p)))
        }
    }
}

pub fn mime_of(data: &[u8]) -> &'static str {
    match data {
        [0xFF, 0xD8, ..] => "image/jpeg",
        [0x89, b'P', b'N', b'G', ..] => "image/png",
        [b'G', b'I', b'F', ..] => "image/gif",
        [b'B', b'M', ..] => "image/bmp",
        [b'R', b'I', b'F', b'F', ..] => "image/webp",
        _ => "application/octet-stream",
    }
}

/// Decodes a JPEG using DCT-domain downscaling so the result's long edge is
/// the smallest available size that is still at least `min_edge`.
pub fn decode_jpeg(data: &[u8], min_edge: u32) -> Result<RgbImage> {
    let mut dec = jpeg_decoder::Decoder::new(Cursor::new(data));
    dec.read_info()?;
    let info = dec.info().ok_or(Error::Malformed)?;
    let long = info.width.max(info.height) as f32;
    let k = (min_edge as f32 / long).min(1.0);
    dec.scale((info.width as f32 * k).ceil() as u16, (info.height as f32 * k).ceil() as u16)?;
    let px = dec.decode()?;
    let info = dec.info().ok_or(Error::Malformed)?;
    let (w, h) = (info.width as u32, info.height as u32);
    let rgb = match info.pixel_format {
        jpeg_decoder::PixelFormat::RGB24 => px,
        jpeg_decoder::PixelFormat::L8 => px.iter().flat_map(|&v| [v, v, v]).collect(),
        jpeg_decoder::PixelFormat::L16 => px.as_chunks::<2>().0.iter().flat_map(|c| [c[0], c[0], c[0]]).collect(),
        jpeg_decoder::PixelFormat::CMYK32 => px
            .as_chunks::<4>()
            .0
            .iter()
            .flat_map(|c| {
                let k = c[3] as u32;
                [(c[0] as u32 * k / 255) as u8, (c[1] as u32 * k / 255) as u8, (c[2] as u32 * k / 255) as u8]
            })
            .collect(),
    };
    RgbImage::from_raw(w, h, rgb).ok_or(Error::Malformed)
}

/// Decodes a JPEG at full size and applies its own EXIF orientation.
pub fn decode_upright(data: &[u8]) -> Result<RgbImage> {
    decode_upright_at(data, u32::MAX)
}

/// Like [`decode_upright`], at the smallest size whose long edge is still at
/// least `min_edge`.
pub fn decode_upright_at(data: &[u8], min_edge: u32) -> Result<RgbImage> {
    let src = Src::mem(data.to_vec());
    let orientation = exif::orientation(&container::parse(&src));
    Ok(orient(decode_jpeg(data, min_edge)?, orientation))
}

pub fn orient(img: RgbImage, orientation: u8) -> RgbImage {
    match orientation {
        2 => imageops::flip_horizontal(&img),
        3 => imageops::rotate180(&img),
        4 => imageops::flip_vertical(&img),
        5 => imageops::flip_horizontal(&imageops::rotate90(&img)),
        6 => imageops::rotate90(&img),
        7 => imageops::flip_horizontal(&imageops::rotate270(&img)),
        8 => imageops::rotate270(&img),
        _ => img,
    }
}

pub fn encode_jpeg(img: &RgbImage, quality: u8) -> Result<Vec<u8>> {
    let mut out = Vec::with_capacity((img.width() * img.height() / 4) as usize);
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, quality).encode_image(img)?;
    Ok(out)
}

/// Scales `img` down so that its long edge is at most `edge`.
pub fn fit(img: RgbImage, edge: u32) -> RgbImage {
    let long = img.width().max(img.height());
    if long <= edge {
        return img;
    }
    let k = edge as f32 / long as f32;
    let (w, h) = (((img.width() as f32 * k).round() as u32).max(1), ((img.height() as f32 * k).round() as u32).max(1));
    imageops::thumbnail(&img, w, h)
}

/// Upright RGB pixels with a long edge of at most `edge`, decoded from the
/// smallest embedded preview that is large enough. Camera JPEGs carry
/// screen-size previews too (MPF), so a thumbnail never needs the full image.
pub fn pixels(path: &Path, edge: u32) -> Result<RgbImage> {
    let src = Src::open(path)?;
    let p = container::parse(&src);
    let (img, orientation) = match p.preview_for(edge) {
        Some(j) => {
            let data = src.read(j.offset, j.len as usize).ok_or(Error::Malformed)?;
            (decode_jpeg(&data, edge)?, exif::orientation(&p))
        }
        None if !formats::is_raw(path) => (image::open(path)?.to_rgb8(), 1),
        None => return Err(Error::NoPreview),
    };
    Ok(orient(fit(img, edge), orientation))
}

/// An upright JPEG rendition with a long edge of at most `edge`.
pub fn scaled(path: &Path, edge: u32, quality: u8) -> Result<Vec<u8>> {
    encode_jpeg(&pixels(path, edge)?, quality)
}

/// A screen-size preview the camera already embedded, handed over untouched
/// (no decode, no re-encode). `None` when the file has none of a fitting size.
pub fn embedded_screen(path: &Path, edge: u32) -> Result<Option<Vec<u8>>> {
    let src = Src::open(path)?;
    let p = container::parse(&src);
    Ok(p.previews
        .iter()
        // Close enough to the requested size: 70% to 125% of it.
        .find(|j| (edge * 7 / 10..=edge + edge / 4).contains(&j.width.max(j.height)))
        .and_then(|j| src.read(j.offset, j.len as usize))
        .map(|data| tag_orientation(data, exif::orientation(&p))))
}

/// A screen-size JPEG: the embedded preview when there is one, else a scaled copy.
pub fn screen(path: &Path, edge: u32, quality: u8) -> Result<Vec<u8>> {
    match embedded_screen(path, edge)? {
        Some(data) => Ok(data),
        None => scaled(path, edge, quality),
    }
}

pub fn to_dynamic(img: RgbImage) -> DynamicImage {
    DynamicImage::ImageRgb8(img)
}
