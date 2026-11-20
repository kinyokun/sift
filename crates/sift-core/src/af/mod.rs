//! Camera-recorded autofocus areas, decoded from vendor MakerNotes.
//!
//! Tag layouts follow the ExifTool tag tables (Canon AFInfo2/AFInfo3, Sony
//! FocusLocation/FocusFrameSize, Nikon AFInfo2 v0100-v0402, FujiFilm
//! FocusPixel/FocusSettings, Panasonic AFPointPosition/AFAreaSize, also in
//! Leica's cameras made by Panasonic, Pentax AFPointSelected/AFPointInfo/
//! AFInfoK3III, Ricoh AFAreaXPosition).

mod canon;
mod fuji;
mod nikon;
mod panasonic;
mod pentax;
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
    /// Vendor that produced the data: `canon`, `nikon`, `sony`, `fujifilm`, `panasonic`, `pentax` or `ricoh`.
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
    } else if make.starts_with("PENTAX") || make.starts_with("RICOH") || make.starts_with("ASAHI") {
        pentax::parse(mn, model, p)
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
    fn af_model(bytes: Vec<u8>, make: &str, model: &str) -> Option<AfInfo> {
        let src = Src::mem(bytes);
        let p = container::parse(&src);
        read(&p, make, model, 1)
    }

    fn bytes(tag: u16, v: &[u8]) -> Entry {
        (tag, 7, v.len() as u32, v.to_vec())
    }

    /// A Pentax MakerNote behind `header`, its offsets counted from the note.
    fn pentax(header: &[u8], entries: &[Entry]) -> Vec<u8> {
        let mut note = header.to_vec();
        note.extend(ifd(entries, header.len() as u32));
        note
    }

    fn centre(b: &AfBox) -> (f32, f32) {
        (b.x + b.w / 2.0, b.y + b.h / 2.0)
    }

    #[test]
    fn pentax_pef_point_in_focus_is_placed_on_the_point_layout() {
        // K-3 II, AF-S, single point Near-right (15), in focus.
        let entries = [shorts(0x000D, &[0x10]), shorts(0x000E, &[15, 0]), long(0x000F, 1 << 14)];
        let file = file("PENTAX", &[], |at| {
            let mut note = b"AOC\0II".to_vec();
            note.extend(ifd(&entries, at + 6));
            note
        });
        let af = af_model(file, "PENTAX", "PENTAX K-3 II").unwrap();
        assert_eq!(af.boxes.len(), 1);
        let (x, y) = centre(&af.boxes[0]);
        assert!(close(x, 3453.0 / 6016.0) && close(y, 0.5), "{:?}", af.boxes[0]);
        assert!(af.boxes[0].focused);
        assert_eq!((af.source, af.mode.as_deref()), ("pentax", Some("Single Point")));
    }

    #[test]
    fn pentax_dng_keeps_its_maker_note_as_private_data() {
        // KP, auto area: every point selected, the centre (14) in focus.
        let mut info = vec![1, 0, 27, 0];
        info.extend((0..27).collect::<Vec<usize>>().chunks(4).map(|c| c.iter().fold(0u8, |b, &k| b | (if k == 13 { 3 } else { 1 }) << (6 - 2 * (k % 4)))));
        let entries = [shorts(0x000D, &[0x10]), shorts(0x000E, &[0xFFFF, 0]), bytes(0x0245, &info), shorts(0x004F, &[1]), bytes(0x0034, &[1, 0, 0, 0])];
        let ifd0 = [text(0x010F, "RICOH IMAGING COMPANY, LTD."), bytes(0xC634, &pentax(b"PENTAX \0II", &entries))];
        let mut tiff = b"II\x2A\0\x08\0\0\0".to_vec();
        tiff.extend(ifd(&ifd0, 8));
        let src = Src::mem(tiff);
        let p = container::parse(&src);
        let af = read(&p, "RICOH IMAGING COMPANY, LTD.", "PENTAX KP", 1).unwrap();
        assert_eq!(af.boxes.len(), 1, "{af:?}");
        let (x, y) = centre(&af.boxes[0]);
        assert!(close(x, 0.5) && close(y, 0.5) && af.boxes[0].focused, "{af:?}");
        assert_eq!(af.mode.as_deref(), Some("Auto"));
        assert_eq!(crate::pentax::look(&p), Some("Bright"));
        assert_eq!(crate::pentax::continuous(&p), Some(true));
    }

    #[test]
    fn pentax_zones_and_eleven_point_pairs() {
        let note = |entries: &[Entry]| file("RICOH IMAGING COMPANY, LTD.", &[], |_| pentax(b"PENTAX \0II", entries));
        // K-1 zone around the centre, nothing said about focus: the nine points, none marked.
        let af = af_model(note(&[shorts(0x000E, &[273, 0])]), "RICOH IMAGING COMPANY, LTD.", "PENTAX K-1").unwrap();
        assert_eq!(af.boxes.len(), 9);
        assert!(af.boxes.iter().all(|b| !b.focused));
        let xs: Vec<f32> = af.boxes.iter().map(|b| centre(b).0 * 7360.0).collect();
        assert!(xs.iter().all(|x| (3238.0 - 1.0..=4121.0 + 1.0).contains(x)), "{xs:?}");
        assert_eq!(af.mode.as_deref(), Some("Zone Select"));
        // K-50: AFInfo reports mid-left and centre in focus.
        let mut af_info = vec![0; 12];
        af_info[0x0B] = 4;
        let af = af_model(note(&[shorts(0x000E, &[6, 0]), bytes(0x021F, &af_info)]), "PENTAX", "PENTAX K-50").unwrap();
        let xs: Vec<f32> = af.boxes.iter().map(|b| centre(b).0).collect();
        assert!(xs.len() == 2 && close(xs[0], 2118.0 / 6000.0) && close(xs[1], 0.5), "{xs:?}");
        // Auto area with nothing in focus, and manual focus, place nothing.
        assert!(af_model(note(&[shorts(0x000E, &[0xFFFF, 0])]), "PENTAX", "PENTAX K-50").is_none());
        assert!(af_model(note(&[shorts(0x000D, &[3]), shorts(0x000E, &[6, 0])]), "PENTAX", "PENTAX K-50").is_none());
        // Live view: the contrast-detect area in a 720 x 480 frame.
        let af = af_model(note(&[shorts(0x000D, &[0x20]), shorts(0x0231, &[336, 216, 48, 48])]), "PENTAX", "PENTAX K-70").unwrap();
        assert!(close(af.boxes[0].x, 336.0 / 720.0) && close(af.boxes[0].h, 0.1), "{af:?}");
        // A body whose points are not mapped still places its centre point.
        let af = af_model(note(&[shorts(0x000E, &[0xFFFE, 0])]), "PENTAX", "PENTAX 645Z").unwrap();
        assert_eq!(centre(&af.boxes[0]), (0.5, 0.5));
    }

    #[test]
    fn ricoh_gr_iii_areas_are_read_from_af_info() {
        // One area in focus, and one outside the selection that was not.
        let words: Vec<u8> = [2, 0x2002, 25, 2, 2, 0, 0, 720, 480, 360, 240, 64, 52, 15, 720, 480, 100, 100, 0, 0, 3].iter().flat_map(|w: &u16| w.to_le_bytes()).collect();
        let file = file("RICOH IMAGING COMPANY, LTD.", &[], |_| pentax(b"RICOH\0II", &[bytes(0x040C, &words), shorts(0x004F, &[258])]));
        let src = Src::mem(file);
        let p = container::parse(&src);
        let af = read(&p, "RICOH IMAGING COMPANY, LTD.", "RICOH GR III", 1).unwrap();
        assert_eq!(af.boxes.len(), 1);
        let b = af.boxes[0];
        assert!(close(centre(&b).0, 0.5) && close(b.w, 64.0 / 720.0) && close(b.h, 52.0 / 480.0) && b.focused, "{b:?}");
        assert_eq!((af.source, af.mode.as_deref()), ("ricoh", Some("Contrast-detect Select")));
        assert_eq!(crate::pentax::look(&p), Some("Monotone"));
    }

    #[test]
    fn ricoh_compact_af_area_is_read_in_the_cropped_picture() {
        let entries = [shorts(0x1006, &[3]), shorts(0x1010, &[1]), shorts(0x1200, &[1]), long(0x1203, 1944), long(0x1204, 1287), long(0x1603, 3936), long(0x1604, 2608)];
        let file = file("RICOH IMAGING COMPANY, LTD.", &[], |at| {
            let mut note = b"Ricoh\0\0\0".to_vec();
            note.extend(ifd(&entries, at + 8));
            note
        });
        let src = Src::mem(file);
        let p = container::parse(&src);
        let af = read(&p, "RICOH IMAGING COMPANY, LTD.", "GR II", 1).unwrap();
        let (x, y) = centre(&af.boxes[0]);
        assert!(close(x, 1944.0 / 3936.0) && close(y, 1287.0 / 2608.0) && af.boxes[0].focused, "{af:?}");
        assert_eq!((af.source, af.mode.as_deref()), ("ricoh", Some("Spot AF")));
        assert_eq!(crate::pentax::look(&p), Some("Vivid"));
    }
}
