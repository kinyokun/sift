//! What was set up for a photo while culling, put on its pixels when it is
//! exported: turned by quarter turns, straightened and cut to its crop, made
//! smaller (watermarks are in `mark.rs`). And the two changes a JPEG takes
//! without its picture being encoded again: another orientation, and its
//! metadata left out.

use crate::io::{u16_at, Src};
use crate::jpeg;
use crate::tiff::{Tiff, T_ORIENTATION};
use image::{imageops, Rgb, RgbImage};

/// `img` turned clockwise by `quarters` quarter turns.
pub fn turn(img: RgbImage, quarters: u8) -> RgbImage {
    match quarters % 4 {
        1 => imageops::rotate90(&img),
        2 => imageops::rotate180(&img),
        3 => imageops::rotate270(&img),
        _ => img,
    }
}

/// The EXIF orientation that shows a picture of orientation `o` turned
/// clockwise by `quarters` more. The orientations are a turn, or a mirroring
/// after a turn; turning a mirrored picture on turns it back before the
/// mirror.
pub fn turned_orientation(o: u8, quarters: u8) -> u8 {
    const TURNS: [u8; 4] = [1, 6, 3, 8];
    const MIRRORED: [u8; 4] = [2, 5, 4, 7];
    let q = (quarters % 4) as usize;
    match (TURNS.iter().position(|&t| t == o), MIRRORED.iter().position(|&m| m == o)) {
        (Some(a), _) => TURNS[(a + q) % 4],
        (_, Some(a)) => MIRRORED[(a + 4 - q) % 4],
        _ => TURNS[q],
    }
}

/// `img` cut to `rect`, normalised `[x, y, width, height]` of the picture
/// straightened by `angle` degrees clockwise about its centre. The crop is
/// expected to lie on the straightened picture; where it does not, the
/// edge of the picture is repeated.
pub fn cut(img: &RgbImage, rect: [f32; 4], angle: f32) -> RgbImage {
    let (iw, ih) = (img.width(), img.height());
    if angle.abs() < 0.01 {
        let x = ((rect[0] * iw as f32).round() as u32).min(iw.saturating_sub(1));
        let y = ((rect[1] * ih as f32).round() as u32).min(ih.saturating_sub(1));
        let w = ((rect[2] * iw as f32).round() as u32).clamp(1, iw - x);
        let h = ((rect[3] * ih as f32).round() as u32).clamp(1, ih - y);
        return imageops::crop_imm(img, x, y, w, h).to_image();
    }
    let w = ((rect[2] * iw as f32).round() as u32).max(1);
    let h = ((rect[3] * ih as f32).round() as u32).max(1);
    let (sin, cos) = (angle as f64).to_radians().sin_cos();
    let (cx, cy) = (iw as f64 / 2.0, ih as f64 / 2.0);
    let (left, top) = (rect[0] as f64 * iw as f64, rect[1] as f64 * ih as f64);
    let mut out = RgbImage::new(w, h);
    let rows = (h as usize).div_ceil(std::thread::available_parallelism().map_or(4, |n| n.get()));
    let stride = w as usize * 3;
    std::thread::scope(|scope| {
        for (band, chunk) in out.chunks_mut(rows * stride).enumerate() {
            scope.spawn(move || {
                for (r, line) in chunk.chunks_mut(stride).enumerate() {
                    let y = (band * rows + r) as f64 + 0.5 + top - cy;
                    for (i, px) in line.chunks_mut(3).enumerate() {
                        let x = i as f64 + 0.5 + left - cx;
                        // The point of the upright picture that is shown here once it is turned.
                        let (sx, sy) = (x * cos + y * sin + cx, -x * sin + y * cos + cy);
                        px.copy_from_slice(&bilinear(img, sx - 0.5, sy - 0.5).0);
                    }
                }
            });
        }
    });
    out
}

fn bilinear(img: &RgbImage, x: f64, y: f64) -> Rgb<u8> {
    let (w, h) = (img.width() as i64, img.height() as i64);
    let (x0, y0) = (x.floor(), y.floor());
    let (fx, fy) = (x - x0, y - y0);
    let at = |xx: i64, yy: i64| img.get_pixel(xx.clamp(0, w - 1) as u32, yy.clamp(0, h - 1) as u32).0;
    let (x0, y0) = (x0 as i64, y0 as i64);
    let (a, b, c, d) = (at(x0, y0), at(x0 + 1, y0), at(x0, y0 + 1), at(x0 + 1, y0 + 1));
    let mut px = [0u8; 3];
    for k in 0..3 {
        let top = a[k] as f64 * (1.0 - fx) + b[k] as f64 * fx;
        let bottom = c[k] as f64 * (1.0 - fx) + d[k] as f64 * fx;
        px[k] = (top * (1.0 - fy) + bottom * fy).round().clamp(0.0, 255.0) as u8;
    }
    Rgb(px)
}

/// `img` made smaller so that its long edge is `edge` (Lanczos); one that is
/// already that small stays as it is.
pub fn shrink(img: RgbImage, edge: u32) -> RgbImage {
    let long = img.width().max(img.height());
    if edge == 0 || long <= edge {
        return img;
    }
    let k = edge as f32 / long as f32;
    let (w, h) = (((img.width() as f32 * k).round() as u32).max(1), ((img.height() as f32 * k).round() as u32).max(1));
    imageops::resize(&img, w, h, imageops::FilterType::Lanczos3)
}

/// The header segments of a JPEG, then where its picture starts and ends.
fn segments(data: &[u8]) -> Option<(Vec<jpeg::Segment>, usize, usize)> {
    let src = Src::mem(data.to_vec());
    let info = jpeg::scan(&src, 0)?;
    let sos = info.segments.iter().find(|s| s.marker == 0xDA)?.pos as usize;
    // The picture ends at the first EOI: inside the coded data a 0xFF is
    // followed by a 0 or a restart marker, and further scans by their headers.
    let mut at = sos + 2;
    let end = loop {
        match data.get(at..at + 2)? {
            [0xFF, 0xD9] => break at + 2,
            [0xFF, m] if *m != 0 && !(0xD0..=0xD7).contains(m) && *m != 0xFF => {
                let len = u16_at(data, at + 2, false)? as usize;
                at += 2 + len.max(2);
            }
            _ => at += 1,
        }
    };
    let header = info.segments.into_iter().filter(|s| s.marker != 0xDA).collect();
    Some((header, sos, end))
}

/// A copy of `data` (a JPEG) shown with `orientation`, the picture and
/// everything else as they were. `None` when that would take more than one
/// value: the file has an EXIF block without an orientation in it.
pub fn reoriented(data: &[u8], orientation: u8) -> Option<Vec<u8>> {
    let src = Src::mem(data.to_vec());
    let info = jpeg::scan(&src, 0)?;
    let Some(base) = info.exif_base else {
        return Some(with_orientation(data, &info.segments, orientation));
    };
    let (tiff, off) = Tiff::open(&src, base)?;
    let e = tiff.ifd(off)?.entries.into_iter().find(|e| e.tag == T_ORIENTATION && e.typ == 3)?;
    let mut out = data.to_vec();
    let pos = e.pos as usize + 8;
    let v = orientation as u16;
    out[pos..pos + 2].copy_from_slice(&if tiff.le { v.to_le_bytes() } else { v.to_be_bytes() });
    Some(out)
}

/// `data` with an EXIF block holding only `orientation` put in after its
/// JFIF header (when the orientation is anything but upright).
fn with_orientation(data: &[u8], segments: &[jpeg::Segment], orientation: u8) -> Vec<u8> {
    if orientation <= 1 {
        return data.to_vec();
    }
    let at = segments.first().filter(|s| s.marker == 0xE0).map_or(2, |s| (s.pos + s.len as u64) as usize);
    let mut out = Vec::with_capacity(data.len() + 40);
    out.extend_from_slice(&data[..at]);
    out.extend(jpeg::orientation_app1(orientation));
    out.extend_from_slice(&data[at..]);
    out
}

/// A copy of `data` (a JPEG) without what it records besides its picture:
/// EXIF (with the place it was taken), XMP, IPTC, comments, and the smaller
/// pictures some cameras put after it. Its colour profile and coding tables
/// stay, and so does how it is turned (`orientation`), as the only value of
/// a new EXIF block. The picture is not encoded again. One that is Adobe
/// RGB by its EXIF alone gets a profile that says so instead.
pub fn stripped(data: &[u8], orientation: u8) -> Option<Vec<u8>> {
    let (header, sos, end) = segments(data)?;
    let src = Src::mem(data.to_vec());
    let adobe_rgb = jpeg::scan(&src, 0)?.exif_base.is_some_and(|base| crate::exif::adobe_rgb_at(&src, base));
    let mut out = Vec::with_capacity(end);
    out.extend_from_slice(&data[..2]);
    let mut kept = Vec::new();
    for s in &header {
        let (pos, len) = (s.pos as usize, s.len as usize);
        let body = data.get(pos + 4..pos + len)?;
        let keep = match s.marker {
            0xE0 => body.starts_with(b"JFIF\0") || body.starts_with(b"JFXX\0"),
            0xE2 => body.starts_with(b"ICC_PROFILE\0"),
            // Says how the colours are coded.
            0xEE => body.starts_with(b"Adobe"),
            0xE1 | 0xE3..=0xED | 0xEF | 0xFE => false,
            _ => true,
        };
        if keep {
            kept.push(jpeg::Segment { marker: s.marker, pos: out.len() as u64, len: s.len });
            out.extend_from_slice(&data[pos..pos + len]);
        }
    }
    out.extend_from_slice(&data[sos..end]);
    Some(crate::color::profiled(with_orientation(&out, &kept, orientation), adobe_rgb))
}

/// The orientation a JPEG records, 1 when it records none.
pub fn orientation_of(data: &[u8]) -> u8 {
    let src = Src::mem(data.to_vec());
    let o = crate::exif::orientation(&crate::container::parse(&src));
    if (1..=8).contains(&o) { o } else { 1 }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::preview;

    /// A small picture no turn or mirroring leaves as it was.
    fn lopsided() -> RgbImage {
        RgbImage::from_fn(5, 3, |x, y| Rgb([(x * 50) as u8, (y * 100) as u8, (x * 7 + y * 31) as u8]))
    }

    #[test]
    fn turning_a_turned_picture_on_is_one_orientation() {
        let img = lopsided();
        for o in 1..=8u8 {
            for q in 0..4u8 {
                let by_hand = turn(preview::orient(img.clone(), o), q);
                let at_once = preview::orient(img.clone(), turned_orientation(o, q));
                assert_eq!(by_hand, at_once, "orientation {o}, {q} quarter turns");
            }
        }
    }

    #[test]
    fn a_crop_without_an_angle_is_a_plain_cut() {
        let img = RgbImage::from_fn(100, 50, |x, y| Rgb([x as u8, y as u8, 0]));
        let part = cut(&img, [0.1, 0.2, 0.5, 0.4], 0.0);
        assert_eq!((part.width(), part.height()), (50, 20));
        assert_eq!(part.get_pixel(0, 0).0, [10, 10, 0]);
    }

    #[test]
    fn a_straightened_crop_turns_the_picture_about_its_centre() {
        // A picture of a cross: after a quarter turn of straightening, the
        // horizontal bar stands upright in the middle of a square crop.
        let img = RgbImage::from_fn(200, 200, |_, y| if (95..105).contains(&y) { Rgb([255, 255, 255]) } else { Rgb([0, 0, 0]) });
        let part = cut(&img, [0.25, 0.25, 0.5, 0.5], 90.0);
        assert_eq!((part.width(), part.height()), (100, 100));
        assert!(part.get_pixel(50, 10).0[0] > 200, "the bar runs down the middle");
        assert!(part.get_pixel(10, 50).0[0] < 50);
        // A small angle tilts it: the bar leaves the middle row towards the edges.
        let tilted = cut(&img, [0.25, 0.25, 0.5, 0.5], 10.0);
        let row = |x: u32| (0..100).max_by_key(|&y| tilted.get_pixel(x, y).0[0]).unwrap() as i32;
        // Clockwise: the right end goes down on screen.
        assert!(row(90) - row(10) > 10, "{} {}", row(10), row(90));
    }

    fn encoded(img: &RgbImage) -> Vec<u8> {
        preview::encode_jpeg(img, 90).unwrap()
    }

    #[test]
    fn a_jpeg_is_turned_by_its_orientation_alone() {
        let plain = encoded(&lopsided());
        // No EXIF yet: one is put in, holding the orientation.
        let turned = reoriented(&plain, 6).unwrap();
        assert_eq!(orientation_of(&turned), 6);
        assert_eq!(preview::decode_upright(&turned).unwrap(), preview::orient(preview::decode_jpeg(&plain, u32::MAX).unwrap(), 6));
        // One that has an orientation gets another in its place, and nothing else changes.
        let again = reoriented(&turned, 3).unwrap();
        assert_eq!(again.len(), turned.len());
        assert_eq!(orientation_of(&again), 3);
    }

    #[test]
    fn stripping_keeps_the_picture_and_how_it_is_turned() {
        let plain = encoded(&lopsided());
        let mut tagged = reoriented(&plain, 8).unwrap();
        // A comment, some XMP and a trailing picture that must all go.
        let comment = [&[0xFF, 0xFE, 0, 7][..], b"hello"].concat();
        tagged.splice(2..2, comment);
        let xmp = [&[0xFF, 0xE1][..], &((2 + jpeg::XMP_HEADER.len() + 4) as u16).to_be_bytes(), jpeg::XMP_HEADER, b"<x/>"].concat();
        tagged.splice(2..2, xmp);
        tagged.extend(encoded(&lopsided()));
        let out = stripped(&tagged, 8).unwrap();
        assert!(!out.windows(5).any(|w| w == b"hello"));
        assert!(!out.windows(4).any(|w| w == b"<x/>"));
        assert_eq!(orientation_of(&out), 8);
        assert_eq!(out.iter().rev().take(2).copied().collect::<Vec<_>>(), [0xD9, 0xFF], "ends with the first picture");
        assert_eq!(preview::decode_upright(&out).unwrap(), preview::decode_upright(&reoriented(&plain, 8).unwrap()).unwrap());
        // Upright, there is no EXIF block at all.
        let upright = stripped(&plain, 1).unwrap();
        assert!(jpeg::scan(&Src::mem(upright.clone()), 0).unwrap().exif_base.is_none());
    }
}
