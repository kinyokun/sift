//! Pentax and Ricoh MakerNotes. Pentax DSLRs, the K-3 Mark III and the
//! Ricoh GR III / IIIx / IV write Pentax's tags; older Ricoh compacts such as
//! the GR II write Ricoh's own. Tag meanings follow ExifTool's Pentax and
//! Ricoh tables.

use crate::container::{MakerNote, Parsed};
use crate::tiff::{Ifd, Tiff};

/// Which tag table a MakerNote holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Pentax,
    Ricoh,
}

fn plausible<'a>(t: Tiff<'a>, pos: u64) -> Option<(Tiff<'a>, Ifd)> {
    t.ifd_at(pos).filter(|ifd| ifd.entries.iter().all(|e| (1..=13).contains(&e.typ))).map(|ifd| (t, ifd))
}

/// The MakerNote's IFD:
/// - `AOC\0` (PEF and Pentax JPEGs): a byte-order mark (or two spaces for the
///   enclosing block's), the IFD, offsets counted from the enclosing TIFF block.
/// - `PENTAX \0` (in-camera DNGs, newer JPEGs) and `RICOH\0` (GR III and
///   later): a byte-order mark, the IFD, offsets counted from the note.
/// - `Ricoh\0\0\0` (GR II and earlier): the IFD, offsets counted from the
///   enclosing block, in either byte order.
pub fn maker_ifd<'a>(mn: &MakerNote<'a>) -> Option<(Kind, Tiff<'a>, Ifd)> {
    let src = mn.parent.src;
    let h = src.read(mn.pos, 10)?;
    let order = |b: &[u8]| match b {
        b"II" => Some(true),
        b"MM" => Some(false),
        _ => None,
    };
    let pentax = |(t, ifd)| (Kind::Pentax, t, ifd);
    if h.starts_with(b"AOC\0") {
        let le = order(&h[4..6]).unwrap_or(mn.parent.le);
        return plausible(Tiff { le, ..mn.parent }, mn.pos + 6).map(pentax);
    }
    if h.starts_with(b"PENTAX \0") {
        return plausible(Tiff { src, base: mn.pos, le: order(&h[8..10])? }, mn.pos + 10).map(pentax);
    }
    if h.starts_with(b"RICOH\0") {
        if let Some(le) = order(&h[6..8]) {
            return plausible(Tiff { src, base: mn.pos, le }, mn.pos + 8).map(pentax);
        }
    }
    if h[..8].eq_ignore_ascii_case(b"Ricoh\0\0\0") {
        let t = mn.parent;
        return plausible(t, mn.pos + 8).or_else(|| plausible(Tiff { le: !t.le, ..t }, mn.pos + 8)).map(|(t, ifd)| (Kind::Ricoh, t, ifd));
    }
    None
}

fn pentax_ifd<'a>(p: &Parsed<'a>) -> Option<(Kind, Tiff<'a>, Ifd)> {
    maker_ifd(p.maker.as_ref()?)
}

/// Pentax ImageTone (0x004F), called Custom Image on the DSLRs and Image
/// Control on the GR III.
fn image_tone(v: u32) -> Option<&'static str> {
    Some(match v {
        0 => "Natural",
        1 => "Bright",
        2 => "Portrait",
        3 => "Landscape",
        4 => "Vibrant",
        5 => "Monochrome",
        6 => "Muted",
        7 => "Reversal Film",
        8 => "Bleach Bypass",
        9 => "Radiant",
        10 => "Cross Processing",
        11 => "Flat",
        256 => "Standard",
        257 => "Vivid",
        258 => "Monotone",
        259 => "Soft Monotone",
        260 => "Hard Monotone",
        261 => "Hi-contrast B&W",
        262 => "Positive Film",
        263 => "Bleach Bypass 2",
        264 => "Retro",
        265 => "HDR Tone",
        266 => "Cross Processing 2",
        267 => "Negative Film",
        32768 | 33024 => "Standard",
        32769 => "Hard",
        32770 | 33026 => "Soft",
        33025 => "Solid",
        33027 => "High Contrast",
        _ => return None,
    })
}

/// Ricoh ImageEffects (0x1010).
fn image_effect(v: u32) -> Option<&'static str> {
    Some(match v {
        0 => "Standard",
        1 => "Vivid",
        3 => "Black & White",
        5 => "B&W Toning Effect",
        6 => "Setting 1",
        7 => "Setting 2",
        9 => "High-contrast B&W",
        10 => "Cross Process",
        11 => "Positive Film",
        12 => "Bleach Bypass",
        13 => "Retro",
        15 => "Miniature",
        17 => "High Key",
        _ => return None,
    })
}

/// The name of the in-camera look the photo was rendered with.
pub fn look(p: &Parsed) -> Option<&'static str> {
    let (kind, t, ifd) = pentax_ifd(p)?;
    match kind {
        Kind::Pentax => image_tone(t.uint(ifd.get(0x004F)?)?),
        Kind::Ricoh => image_effect(t.uint(ifd.get(0x1010)?)?),
    }
}

/// Whether the shutter was set to continuous release: Pentax DriveMode
/// (0x0034) byte 0, or Ricoh DriveMode (0x1002, a SHORT only on the GR models).
pub fn continuous(p: &Parsed) -> Option<bool> {
    let (kind, t, ifd) = pentax_ifd(p)?;
    match kind {
        Kind::Pentax => match *t.data(ifd.get(0x0034)?)?.first()? {
            0 => Some(false),
            1..=5 => Some(true),
            _ => None,
        },
        Kind::Ricoh => {
            let e = ifd.get(0x1002).filter(|e| e.typ == 3)?;
            match t.uint(e)? {
                0 => Some(false),
                1 | 8 => Some(true),
                _ => None,
            }
        }
    }
}
