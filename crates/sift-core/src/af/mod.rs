//! Camera-recorded autofocus areas, decoded from vendor MakerNotes.
//!
//! Tag layouts follow the ExifTool tag tables (Canon AFInfo2/AFInfo3, Sony
//! FocusLocation/FocusFrameSize, Nikon AFInfo2 v0100-v0402, FujiFilm
//! FocusPixel/FocusSettings, Panasonic AFPointPosition/AFAreaSize, also in
//! Leica's cameras made by Panasonic).

mod canon;
mod fuji;
mod nikon;
mod panasonic;
mod sony;

use crate::container::Parsed;
use serde::{Deserialize, Serialize};

/// A rectangle in normalised image coordinates (0..1, origin top-left).
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq)]
pub struct AfBox {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    /// True when the camera reported this area as in focus.
    pub focused: bool,
}

#[derive(Serialize, Clone, Debug, Default)]
pub struct AfInfo {
    pub boxes: Vec<AfBox>,
    /// Vendor that produced the data: `canon`, `nikon`, `sony`, `fujifilm` or `panasonic`.
    pub source: &'static str,
    /// AF area mode as named by the camera maker, when known.
    pub mode: Option<String>,
}

impl AfBox {
    pub(crate) fn centered(cx: f32, cy: f32, w: f32, h: f32, focused: bool) -> AfBox {
        AfBox { x: cx - w / 2.0, y: cy - h / 2.0, w, h, focused }
    }

    fn clamp(self) -> Option<AfBox> {
        let (x0, y0) = (self.x.max(0.0), self.y.max(0.0));
        let (x1, y1) = ((self.x + self.w).min(1.0), (self.y + self.h).min(1.0));
        (x1 > x0 && y1 > y0).then_some(AfBox { x: x0, y: y0, w: x1 - x0, h: y1 - y0, focused: self.focused })
    }

    /// Maps a box from sensor orientation to display orientation.
    pub fn oriented(self, orientation: u8) -> AfBox {
        let AfBox { x, y, w, h, focused } = self;
        let (x, y, w, h) = match orientation {
            2 => (1.0 - x - w, y, w, h),
            3 => (1.0 - x - w, 1.0 - y - h, w, h),
            4 => (x, 1.0 - y - h, w, h),
            5 => (y, x, h, w),
            6 => (1.0 - y - h, x, h, w),
            7 => (1.0 - y - h, 1.0 - x - w, h, w),
            8 => (y, 1.0 - x - w, h, w),
            _ => (x, y, w, h),
        };
        AfBox { x, y, w, h, focused }
    }
}

/// Decodes AF areas in display orientation. Returns `None` when the file
/// carries no usable AF record.
pub fn read(p: &Parsed, make: &str, model: &str, orientation: u8) -> Option<AfInfo> {
    let mn = p.maker.as_ref()?;
    let make = make.to_ascii_uppercase();
    let mut info = if make.starts_with("CANON") {
        canon::parse(mn, model)
    } else if make.starts_with("NIKON") {
        nikon::parse(mn, model, p)
    } else if make.starts_with("SONY") {
        sony::parse(p)
    } else if make.starts_with("FUJIFILM") {
        fuji::parse(mn, p)
    } else if make.starts_with("PANASONIC") || make.starts_with("LEICA") {
        panasonic::parse(mn, p, &make)
    } else {
        None
    }?;
    info.boxes = info.boxes.into_iter().filter_map(AfBox::clamp).map(|b| b.oriented(orientation)).collect();
    (!info.boxes.is_empty()).then_some(info)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::container;
    use crate::io::Src;

    /// An IFD entry: tag, type, count and the value's bytes (little-endian).
    type Entry = (u16, u16, u32, Vec<u8>);

    fn shorts(tag: u16, v: &[u16]) -> Entry {
        (tag, 3, v.len() as u32, v.iter().flat_map(|x| x.to_le_bytes()).collect())
    }

    fn long(tag: u16, v: u32) -> Entry {
        (tag, 4, 1, v.to_le_bytes().to_vec())
    }

    fn rationals(tag: u16, v: &[(u32, u32)]) -> Entry {
        (tag, 5, v.len() as u32, v.iter().flat_map(|(n, d)| [n.to_le_bytes(), d.to_le_bytes()].concat()).collect())
    }

    fn text(tag: u16, s: &str) -> Entry {
        let mut v = s.as_bytes().to_vec();
        v.push(0);
        (tag, 2, v.len() as u32, v)
    }

    /// An IFD to be placed at `at` (offsets count from the TIFF header), with
    /// the values that do not fit in an entry right after it, in order.
    fn ifd(entries: &[Entry], at: u32) -> Vec<u8> {
        let table = 2 + entries.len() * 12 + 4;
        let mut out = (entries.len() as u16).to_le_bytes().to_vec();
        let mut values = Vec::new();
        for (tag, typ, count, v) in entries {
            out.extend(tag.to_le_bytes());
            out.extend(typ.to_le_bytes());
            out.extend(count.to_le_bytes());
            if v.len() <= 4 {
                out.extend(v);
                out.extend(vec![0; 4 - v.len()]);
            } else {
                out.extend((at + (table + values.len()) as u32).to_le_bytes());
                values.extend(v);
                values.resize(values.len().next_multiple_of(2), 0);
            }
        }
        out.extend([0; 4]);
        out.extend(values);
        out
    }

    /// A little-endian TIFF: IFD0 with `make` and `extra`, an EXIF IFD of a
    /// 6000 x 4000 picture whose MakerNote `note(position)` returns.
    fn file(make: &str, extra: &[Entry], note: impl Fn(u32) -> Vec<u8>) -> Vec<u8> {
        let mut ifd0 = vec![text(0x010F, make), long(0x8769, 0)];
        ifd0.extend_from_slice(extra);
        let exif_at = 8 + ifd(&ifd0, 8).len() as u32;
        ifd0[1] = long(0x8769, exif_at);
        // The MakerNote is the EXIF IFD's only value stored outside it, so it sits right after the table.
        let note_at = exif_at + 2 + 3 * 12 + 4;
        let note = note(note_at);
        let exif = [long(0xA002, 6000), long(0xA003, 4000), (0x927C, 7, note.len() as u32, note)];
        let mut out = b"II\x2A\0\x08\0\0\0".to_vec();
        out.extend(ifd(&ifd0, 8));
        out.extend(ifd(&exif, exif_at));
        out
    }

    fn af_of(bytes: Vec<u8>, make: &str) -> Option<AfInfo> {
        let src = Src::mem(bytes);
        let p = container::parse(&src);
        read(&p, make, "", 1)
    }

    fn fuji(entries: &[Entry]) -> Vec<u8> {
        file("FUJIFILM", &[], |_| {
            let mut note = b"FUJIFILM\x0C\0\0\0".to_vec();
            note.extend(ifd(entries, 12));
            note
        })
    }

    fn panasonic(extra: &[Entry], entries: &[Entry]) -> Vec<u8> {
        file("Panasonic", extra, |at| {
            let mut note = b"Panasonic\0\0\0".to_vec();
            note.extend(ifd(entries, at + 12));
            note
        })
    }

    /// A MakerNote of Panasonic's tags behind `header`, in a file by `make`.
    fn panasonic_type(make: &str, header: &[u8], entries: &[Entry]) -> Vec<u8> {
        file(make, &[], |at| {
            let mut note = header.to_vec();
            note.extend(ifd(entries, at + header.len() as u32));
            note
        })
    }

    fn close(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-4
    }

    #[test]
    fn fujifilm_focus_pixel_is_placed_in_the_picture() {
        // AF-S, zone, 5 x 5 points of the 13 x 9 grid.
        let af = af_of(fuji(&[shorts(0x1023, &[2000, 2400]), long(0x102D, 0x00A5_0101)]), "FUJIFILM").unwrap();
        let b = af.boxes[0];
        assert!(close(b.x + b.w / 2.0, 1.0 / 3.0) && close(b.y + b.h / 2.0, 0.6), "{b:?}");
        assert!(close(b.w, 5.0 / 13.0) && close(b.h, 5.0 / 9.0), "{b:?}");
        assert!(b.focused);
        assert_eq!(af.mode.as_deref(), Some("Zone 5x5"));
        assert_eq!(af.source, "fujifilm");
    }

    #[test]
    fn fujifilm_manual_focus_and_odd_zones_are_not_taken_at_their_word() {
        let af = af_of(fuji(&[shorts(0x1021, &[1]), shorts(0x1023, &[3000, 2000])]), "FUJIFILM").unwrap();
        assert!(!af.boxes[0].focused);
        assert_eq!(af.mode.as_deref(), Some("Manual"));
        // A zone size that does not read as square gets the nominal frame.
        let af = af_of(fuji(&[shorts(0x1023, &[3000, 2000]), long(0x102D, 0x0027_0101)]), "FUJIFILM").unwrap();
        assert!(close(af.boxes[0].w, 0.04));
        assert_eq!(af.mode.as_deref(), Some("Zone"));
        // Nothing focused on, or a point outside the picture.
        assert!(af_of(fuji(&[shorts(0x1023, &[0, 0])]), "FUJIFILM").is_none());
        assert!(af_of(fuji(&[shorts(0x1023, &[6001, 10])]), "FUJIFILM").is_none());
    }

    #[test]
    fn panasonic_af_area_is_read_as_fractions() {
        let af = af_of(panasonic(&[], &[rationals(0x004D, &[(256, 1024), (768, 1024)]), rationals(0x00DE, &[(100, 1024), (150, 1024)]), (0x000F, 1, 2, vec![16, 0])]), "Panasonic").unwrap();
        let b = af.boxes[0];
        assert!(close(b.x + b.w / 2.0, 0.25) && close(b.y + b.h / 2.0, 0.75), "{b:?}");
        assert!(close(b.w, 100.0 / 1024.0) && close(b.h, 150.0 / 1024.0), "{b:?}");
        assert_eq!(af.mode.as_deref(), Some("1-area"));
        assert_eq!(af.source, "panasonic");
        // No AF area used.
        assert!(af_of(panasonic(&[], &[rationals(0x004D, &[(16777216, 1), (16777216, 1)])]), "Panasonic").is_none());
    }

    #[test]
    fn leica_cameras_made_by_panasonic_are_read_like_panasonic() {
        let entries = [rationals(0x004D, &[(256, 1024), (512, 1024)])];
        let centre = |af: Option<AfInfo>| af.map(|af| (af.boxes[0].x + af.boxes[0].w / 2.0, af.boxes[0].y + af.boxes[0].h / 2.0));
        // D-Lux 7, C-Lux, V-Lux 5.
        let c = centre(af_of(panasonic_type("LEICA CAMERA AG", b"LEICA CAMERA AG\0\0\0", &entries), "LEICA CAMERA AG")).unwrap();
        assert!(close(c.0, 0.25) && close(c.1, 0.5), "{c:?}");
        // D-LUX (Typ 109) and older.
        let c = centre(af_of(panasonic_type("LEICA", b"LEICA\0\0\0", &entries), "LEICA")).unwrap();
        assert!(close(c.0, 0.25) && close(c.1, 0.5), "{c:?}");
        // The M8 starts its own kind of MakerNote the same way.
        assert!(af_of(panasonic_type("Leica Camera AG", b"LEICA\0\0\0", &entries), "Leica Camera AG").is_none());
    }

    #[test]
    fn panasonic_af_area_of_a_cropped_rw2_stays_in_the_picture() {
        // A DC-GH6 file shot in 16:9: the sensor area its JPEGs show, and the picture inside it.
        let borders = [shorts(0x0004, &[8]), shorts(0x0005, &[8]), shorts(0x0006, &[4344]), shorts(0x0007, &[5784])];
        let crop = [shorts(0x002F, &[552]), shorts(0x0030, &[8]), shorts(0x0031, &[3800]), shorts(0x0032, &[5784])];
        let rw2 = |entries: &[Entry]| {
            let mut bytes = panasonic(&[borders.clone(), crop.clone()].concat(), entries);
            bytes[2] = b'U';
            bytes
        };
        // Every rendition is cut to the picture, so the fractions are kept as they are.
        let entries = [rationals(0x004D, &[(467, 1024), (593, 1024)]), rationals(0x00DE, &[(46, 1024), (82, 1024)])];
        let b = af_of(rw2(&entries), "Panasonic").unwrap().boxes[0];
        assert!(close(b.x + b.w / 2.0, 467.0 / 1024.0) && close(b.y + b.h / 2.0, 593.0 / 1024.0), "{b:?}");
        assert!(close(b.w, 46.0 / 1024.0) && close(b.h, 82.0 / 1024.0), "{b:?}");
        // Without a recorded size, the default frame is square in the 5776 x 3248 picture.
        let b = af_of(rw2(&entries[..1]), "Panasonic").unwrap().boxes[0];
        assert!(close(b.w, 0.04) && close(b.h, 0.04 * 5776.0 / 3248.0), "{b:?}");
    }
}
