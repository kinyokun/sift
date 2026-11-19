use super::{AfBox, AfInfo};
use crate::container::{MakerNote, Parsed};
use crate::io::u32_at;
use crate::tiff::{Entry, Ifd, Tiff};

/// Edge of the drawn frame as a fraction of image width when the camera
/// records only the centre of the AF area.
const DEFAULT_FRAME: f32 = 0.04;

fn mode_name(v: &[u32]) -> Option<&'static str> {
    Some(match v {
        [0, 1] => "9-area",
        [0, 16] => "3-area (high speed)",
        [0, 23] => "23-area",
        [0, 49] => "49-area",
        [0, 225] => "225-area",
        [1, 0] => "Spot Focusing",
        [1, 1] => "5-area",
        [16, 0] => "1-area",
        [16, 16] => "1-area (high speed)",
        [16, 32] => "1-area +",
        [17, 0] => "Full Area",
        [32, 0] | [240, 0] => "Tracking",
        [32, 16] => "Zone",
        [32, 18] => "Zone (horizontal/vertical)",
        [64, 0] => "Face Detect",
        [64, 1] => "Face Detect (animal detect on)",
        [64, 2] => "Face Detect (animal detect off)",
        [128, 0] => "Pinpoint focus",
        _ => return None,
    })
}

/// The Panasonic MakerNote: a header, then an IFD whose offsets are counted
/// from the enclosing TIFF block. Its byte order is not always the block's,
/// so both are tried. Leica's cameras made by Panasonic write the same IFD
/// behind headers of their own; Leica's other cameras also start with
/// `LEICA\0\0\0` but hold other tags, and say `Leica Camera AG` as their make.
fn maker_ifd<'a>(mn: &MakerNote<'a>, make: &str) -> Option<(Tiff<'a>, Ifd)> {
    let t = mn.parent;
    let head = t.src.read(mn.pos, 18)?;
    let skip = if head.starts_with(b"Panasonic\0") {
        12
    } else if head.starts_with(b"LEICA CAMERA AG\0") {
        18
    } else if head.starts_with(b"LEICA\0\0\0") && make == "LEICA" {
        8
    } else {
        return None;
    };
    let plausible = |t: Tiff<'a>| t.ifd_at(mn.pos + skip).filter(|ifd| ifd.entries.iter().all(|e| (1..=13).contains(&e.typ))).map(|ifd| (t, ifd));
    plausible(t).or_else(|| plausible(Tiff { le: !t.le, ..t }))
}

/// Both values of an unsigned RATIONAL pair; `None` when a denominator is zero.
fn pair(t: &Tiff, e: &Entry) -> Option<(f32, f32)> {
    if e.typ != 5 || e.count < 2 {
        return None;
    }
    let d = t.data(e)?;
    let ratio = |at: usize| {
        let (n, den) = (u32_at(&d, at, t.le)?, u32_at(&d, at + 4, t.le)?);
        (den != 0).then(|| (n as f64 / den as f64) as f32)
    };
    Some((ratio(0)?, ratio(8)?))
}

/// Panasonic AFPointPosition (MakerNote 0x004D): the centre of the primary AF
/// area as fractions of the recorded picture, and AFAreaSize (0x00DE, recent
/// bodies) its size the same way.
pub fn parse(mn: &MakerNote, p: &Parsed, make: &str) -> Option<AfInfo> {
    let (t, ifd) = maker_ifd(mn, make)?;
    // Cameras that did not use an AF area write 16777216 for both.
    let (x, y) = pair(&t, ifd.get(0x004D)?).filter(|(x, y)| (0.0..=1.0).contains(x) && (0.0..=1.0).contains(y) && (*x, *y) != (0.0, 0.0))?;
    // An RW2 shot in 16:9, 3:2 or 1:1 is shown cut to the picture, like the camera's JPEG.
    let (pw, ph) = match p.recorded_crop() {
        Some(c) => ((c.crop[2] - c.crop[0]) as f32, (c.crop[3] - c.crop[1]) as f32),
        None => {
            let exif = crate::exif::read(p);
            (exif.width.unwrap_or(3) as f32, exif.height.unwrap_or(2) as f32)
        }
    };
    let (w, h) = ifd
        .get(0x00DE)
        .and_then(|e| pair(&t, e))
        .filter(|(w, h)| *w > 0.0 && *h > 0.0 && *w <= 1.0 && *h <= 1.0)
        .unwrap_or((DEFAULT_FRAME, DEFAULT_FRAME * pw / ph));
    let manual = ifd.get(0x0007).and_then(|e| t.uint(e)) == Some(2);
    let mode = if manual { Some("Manual") } else { ifd.get(0x000F).and_then(|e| mode_name(&t.uints(e))) };
    Some(AfInfo { boxes: vec![AfBox::centered(x, y, w, h, !manual)], source: "panasonic", mode: mode.map(str::to_string) })
}
