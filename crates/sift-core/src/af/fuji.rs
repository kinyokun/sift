use super::{AfBox, AfInfo};
use crate::container::{MakerNote, Parsed};
use crate::io::u32_at;
use crate::tiff::{Ifd, Tiff};

/// Edge of the drawn frame as a fraction of image width when the camera
/// records only where it focused (single point, wide/tracking).
const DEFAULT_FRAME: f32 = 0.04;
/// Zone sizes count points of the 13 x 9 grid the zones are placed on.
const ZONE_GRID: (f32, f32) = (13.0, 9.0);

/// The FujiFilm MakerNote: `FUJIFILM`, the IFD's offset, then an IFD that is
/// always little-endian, with offsets counted from the start of the note.
pub(crate) fn maker_ifd<'a>(mn: &MakerNote<'a>) -> Option<(Tiff<'a>, Ifd)> {
    let src = mn.parent.src;
    let h = src.read(mn.pos, 12)?;
    if !h.starts_with(b"FUJIFILM") {
        return None;
    }
    let t = Tiff { src, base: mn.pos, le: true };
    Some((t, t.ifd(u32_at(&h, 8, true)?)?))
}

/// FujiFilm FocusPixel (MakerNote 0x1023): the point focused on, in pixels of
/// the image the EXIF block describes (ExifImageWidth/Height), which for a
/// RAF file is its embedded JPEG.
pub fn parse(mn: &MakerNote, p: &Parsed) -> Option<AfInfo> {
    let (t, ifd) = maker_ifd(mn)?;
    let vals = |tag: u16| ifd.get(tag).map(|e| t.uints(e)).unwrap_or_default();
    let one = |tag: u16| vals(tag).first().copied();

    let pixel = vals(0x1023);
    let (x, y) = (*pixel.first()? as f32, *pixel.get(1)? as f32);
    let exif = crate::exif::read(p);
    let (w, h) = (exif.width? as f32, exif.height? as f32);
    if w < 16.0 || h < 16.0 || x <= 0.0 || y <= 0.0 || x >= w || y >= h {
        return None;
    }

    // FocusSettings (0x102D, X-T3 and later): AF area mode, point and zone size.
    let settings = one(0x102D);
    let area = settings.map(|s| (s >> 8) & 0xF).or_else(|| {
        one(0x1022).and_then(|m| match m {
            1 => Some(0),
            256 => Some(1),
            512 => Some(2),
            _ => None,
        })
    });
    // Zone size as ExifTool reads it: width in the low four bits, height from bit 5.
    // Zones are square (3x3, 5x5, 7x7); other readings are not drawn to size.
    let zone = settings.map(|s| (s >> 16) & 0xFF).map(|z| (z & 0x0F, z >> 5)).filter(|(w, h)| w == h && [3, 5, 7].contains(w)).map(|(w, h)| (w as f32, h as f32));
    let manual = one(0x1021) == Some(1) || settings.is_some_and(|s| s & 0xF == 0);
    let focused = !manual && one(0x1301) != Some(1);

    let (fw, fh) = match (area, zone) {
        (Some(1), Some((zw, zh))) if zw > 0.0 && zh > 0.0 => (w * zw / ZONE_GRID.0, h * zh / ZONE_GRID.1),
        _ => (w * DEFAULT_FRAME, w * DEFAULT_FRAME),
    };
    let mode = match (area, zone) {
        _ if manual => Some("Manual".to_string()),
        (Some(0), _) => Some("Single Point".to_string()),
        (Some(1), Some((zw, zh))) => Some(format!("Zone {zw}x{zh}")),
        (Some(1), None) => Some("Zone".to_string()),
        (Some(2), _) => Some("Wide/Tracking".to_string()),
        _ => None,
    };
    Some(AfInfo { boxes: vec![AfBox::centered(x / w, y / h, fw / w, fh / h, focused)], source: "fujifilm", mode })
}
