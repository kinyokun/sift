//! Pentax and Ricoh AF areas.
//!
//! Most Pentax bodies record which viewfinder AF points were selected and
//! which were in focus, not where those points are; the point positions
//! below were measured on photos and come from the Focus Points Lightroom
//! plug-in (github.com/musselwhizzle/Focus-Points, Apache-2.0). The K-3
//! Mark III and the GR III and later record each area's position instead.

use super::{AfBox, AfInfo};
use crate::container::{MakerNote, Parsed};
use crate::io::u16_at;
use crate::pentax::{self, Kind};
use crate::tiff::{Ifd, Tiff};

/// Edge of the drawn frame as a fraction of image width when the camera
/// records only where it focused.
const DEFAULT_FRAME: f32 = 0.04;
/// Live view (contrast-detect) and the K-3 Mark III record areas in this frame.
const LIVE_FRAME: (f32, f32) = (720.0, 480.0);

/// The viewfinder AF points of a family of bodies, numbered as the camera
/// numbers them (row by row from the top left).
struct Layout {
    /// Image size the positions were measured in.
    frame: (f32, f32),
    /// Size of a point, unless the point gives its own.
    size: (f32, f32),
    /// Distance between neighbouring points, which zones and expanded areas count in.
    step: (f32, f32),
    /// Centre and size (zero for `size`) of each point.
    points: &'static [(f32, f32, f32, f32)],
    /// The point "Fixed Center" means.
    center: usize,
}

/// K-1 and K-1 Mark II: 33 points.
static K1: Layout = Layout {
    frame: (7360.0, 4912.0),
    size: (150.0, 150.0),
    step: (441.5, 319.0),
    #[rustfmt::skip]
    points: &[
        (2797.0, 1818.0, 0.0, 0.0), (3238.0, 1818.0, 0.0, 0.0), (3680.0, 1818.0, 0.0, 0.0), (4121.0, 1818.0, 0.0, 0.0), (4563.0, 1818.0, 0.0, 0.0),
        (2355.0, 2137.0, 0.0, 0.0), (2797.0, 2137.0, 0.0, 0.0), (3238.0, 2137.0, 0.0, 0.0), (3680.0, 2137.0, 0.0, 0.0), (4121.0, 2137.0, 0.0, 0.0), (4563.0, 2137.0, 0.0, 0.0), (5005.0, 2137.0, 0.0, 0.0),
        (1914.0, 2456.0, 150.0, 350.0), (2355.0, 2456.0, 0.0, 0.0), (2797.0, 2456.0, 0.0, 0.0), (3238.0, 2456.0, 0.0, 0.0), (3680.0, 2456.0, 200.0, 200.0), (4121.0, 2456.0, 0.0, 0.0), (4563.0, 2456.0, 0.0, 0.0), (5005.0, 2456.0, 0.0, 0.0), (5446.0, 2456.0, 150.0, 350.0),
        (2355.0, 2775.0, 0.0, 0.0), (2797.0, 2775.0, 0.0, 0.0), (3238.0, 2775.0, 0.0, 0.0), (3680.0, 2775.0, 0.0, 0.0), (4121.0, 2775.0, 0.0, 0.0), (4563.0, 2775.0, 0.0, 0.0), (5005.0, 2775.0, 0.0, 0.0),
        (2797.0, 3094.0, 0.0, 0.0), (3238.0, 3094.0, 0.0, 0.0), (3680.0, 3094.0, 0.0, 0.0), (4121.0, 3094.0, 0.0, 0.0), (4563.0, 3094.0, 0.0, 0.0),
    ],
    center: 17,
};

/// K-3, K-3 II and KP: 27 points.
static K3: Layout = Layout {
    frame: (6016.0, 4000.0),
    size: (150.0, 150.0),
    step: (445.0, 324.0),
    #[rustfmt::skip]
    points: &[
        (2118.0, 1352.0, 0.0, 0.0), (2563.0, 1352.0, 0.0, 0.0), (3008.0, 1352.0, 0.0, 0.0), (3453.0, 1352.0, 0.0, 0.0), (3898.0, 1352.0, 0.0, 0.0),
        (2118.0, 1676.0, 0.0, 0.0), (2563.0, 1676.0, 0.0, 0.0), (3008.0, 1676.0, 0.0, 0.0), (3453.0, 1676.0, 0.0, 0.0), (3898.0, 1676.0, 0.0, 0.0),
        (1221.0, 2000.0, 73.0, 245.0), (2118.0, 2000.0, 0.0, 0.0), (2563.0, 2000.0, 0.0, 0.0), (3008.0, 2000.0, 0.0, 0.0), (3453.0, 2000.0, 0.0, 0.0), (3898.0, 2000.0, 0.0, 0.0), (4795.0, 2000.0, 73.0, 245.0),
        (2118.0, 2324.0, 0.0, 0.0), (2563.0, 2324.0, 0.0, 0.0), (3008.0, 2324.0, 0.0, 0.0), (3453.0, 2324.0, 0.0, 0.0), (3898.0, 2324.0, 0.0, 0.0),
        (2118.0, 2648.0, 0.0, 0.0), (2563.0, 2648.0, 0.0, 0.0), (3008.0, 2648.0, 0.0, 0.0), (3453.0, 2648.0, 0.0, 0.0), (3898.0, 2648.0, 0.0, 0.0),
    ],
    center: 14,
};

/// The 11-point bodies (K-5, K-30, K-50, K-70, KF, K-S2, ...): upper-left,
/// top, upper-right; left, mid-left, centre, mid-right, right; lower-left,
/// bottom, lower-right.
static ELEVEN: Layout = Layout {
    frame: (6000.0, 4000.0),
    size: (122.0, 122.0),
    step: (882.0, 636.0),
    #[rustfmt::skip]
    points: &[
        (2118.0, 1364.0, 0.0, 0.0), (3000.0, 1364.0, 0.0, 0.0), (3881.0, 1364.0, 0.0, 0.0),
        (1272.0, 2000.0, 73.0, 245.0), (2118.0, 2000.0, 0.0, 0.0), (3000.0, 2000.0, 0.0, 0.0), (3881.0, 2000.0, 0.0, 0.0), (4728.0, 2000.0, 73.0, 245.0),
        (2118.0, 2636.0, 0.0, 0.0), (3000.0, 2636.0, 0.0, 0.0), (3881.0, 2636.0, 0.0, 0.0),
    ],
    center: 6,
};

impl Layout {
    /// Point `n` (1-based) in image fractions.
    fn point(&self, n: usize, focused: bool) -> Option<AfBox> {
        let &(x, y, w, h) = self.points.get(n.checked_sub(1)?)?;
        let (w, h) = if w > 0.0 { (w, h) } else { self.size };
        let (fw, fh) = self.frame;
        Some(AfBox::centered(x / fw, y / fh, w / fw, h / fh, focused))
    }

    /// Points within `reach` steps of point `n`, the point itself included.
    fn around(&self, n: usize, reach: f32) -> Vec<usize> {
        let Some(&(cx, cy, _, _)) = n.checked_sub(1).and_then(|i| self.points.get(i)) else { return Vec::new() };
        let near = |d: f32, step: f32| d.abs() <= step * (reach + 0.25);
        (1..=self.points.len()).filter(|&k| near(self.points[k - 1].0 - cx, self.step.0) && near(self.points[k - 1].1 - cy, self.step.1)).collect()
    }

    /// The point nearest to a position measured in `other`'s points, if one is close.
    fn nearest(&self, other: &Layout, n: usize) -> Option<usize> {
        let &(x, y, _, _) = other.points.get(n.checked_sub(1)?)?;
        let (x, y) = (x / other.frame.0, y / other.frame.1);
        let dist = |k: usize| {
            let p = self.points[k - 1];
            ((p.0 / self.frame.0 - x) / (self.step.0 / self.frame.0)).hypot((p.1 / self.frame.1 - y) / (self.step.1 / self.frame.1))
        };
        (1..=self.points.len()).min_by(|&a, &b| dist(a).total_cmp(&dist(b))).filter(|&k| dist(k) < 0.5)
    }
}

/// The model without the brand, as Pentax spells it after "PENTAX ".
fn short_model(model: &str) -> &str {
    model.trim().trim_start_matches("PENTAX ").trim()
}

fn layout_of(model: &str) -> Option<&'static Layout> {
    Some(match short_model(model) {
        "K-1" | "K-1 Mark II" => &K1,
        "K-3" | "K-3 II" | "KP" => &K3,
        "K-5" | "K-5 II" | "K-5 II s" | "K-7" | "K-30" | "K-50" | "K-500" | "K-r" | "K-x" | "K-S1" | "K-S2" | "K-70" | "KF" | "K10D" | "K20D" | "K200D" | "K100D" | "K100D Super" | "K110D" | "*ist D" | "*ist DS" | "*ist DS2" => &ELEVEN,
        _ => return None,
    })
}

fn u16s(d: &[u8], le: bool) -> Vec<u16> {
    (0..d.len() / 2).filter_map(|i| u16_at(d, i * 2, le)).collect()
}

/// AFPointInfo (0x0245, KF, KP, K-1, K-70): a point count, then two bits per
/// point from the most significant: 0x2 in focus, any bit selected.
fn point_info(t: &Tiff, ifd: &Ifd, layout: &Layout) -> Option<(Vec<usize>, Vec<usize>)> {
    let d = t.data(ifd.get(0x0245)?)?;
    let n = u16_at(&d, 2, t.le)? as usize;
    if n != layout.points.len() || d.len() < 4 + n.div_ceil(4) {
        return None;
    }
    let bits = |k: usize| (d[4 + k / 4] >> (6 - 2 * (k % 4))) & 3;
    let focused = (0..n).filter(|&k| bits(k) & 2 != 0).map(|k| k + 1).collect();
    let selected = (0..n).filter(|&k| bits(k) != 0).map(|k| k + 1).collect();
    Some((focused, selected))
}

/// AFInfo (0x021F) byte 11 on 11-point bodies that have no AFPointInfo: the
/// point or pair of points in focus.
fn eleven_in_focus(code: u8) -> Option<&'static [usize]> {
    Some(match code {
        1 => &[9, 10],
        2 => &[10],
        3 => &[11, 10],
        4 => &[5, 6],
        5 => &[6],
        6 => &[7, 6],
        7 => &[1, 2],
        8 => &[2],
        9 => &[3, 2],
        10 => &[8],
        11 => &[9, 5],
        12 => &[1, 5],
        13 => &[10, 6],
        14 => &[2, 6],
        15 => &[11, 7],
        16 => &[3, 7],
        17 => &[4],
        18 => &[5],
        19 => &[6],
        20 => &[7],
        _ => return None,
    })
}

/// FocusMode (0x000D) without the release-priority bit.
fn contrast_detect(mode: u32) -> Option<&'static str> {
    match mode & 0xFF {
        0x20 => Some("Contrast-detect"),
        0x21 => Some("Tracking Contrast-detect"),
        _ => None,
    }
}

/// ContrastDetectAFArea (0x0231): left, top, width and height in 720 x 480.
fn contrast_area(t: &Tiff, ifd: &Ifd) -> Option<AfBox> {
    let v = t.uints(ifd.get(0x0231)?);
    let [l, top, w, h] = v[..] else { return None };
    let (fw, fh) = LIVE_FRAME;
    (w > 0 && h > 0).then(|| AfBox { x: l as f32 / fw, y: top as f32 / fh, w: w as f32 / fw, h: h as f32 / fh, focused: true })
}

fn selection_name(mode: u32) -> Option<&'static str> {
    Some(match mode {
        0 => "Manual Focus",
        1 => "Spot",
        2 => "Select (5-points)",
        3 => "Expanded Area (S)",
        4 => "Expanded Area (M)",
        5 => "Expanded Area (L)",
        6 => "Select (S)",
        7 => "Zone Select (21-point)",
        8 => "Select XS",
        0xFF => "Auto Area",
        0x2001 => "Contrast-detect Auto Area",
        0x2002 => "Contrast-detect Select",
        0x2003 => "Pinpoint",
        0x2004 => "Tracking",
        0x2005 => "Continuous",
        0x2006 => "Face Detection",
        0x2007 => "Contrast-detect Select (S)",
        0x2008 => "Contrast-detect Select (M)",
        0x2009 => "Contrast-detect Select (L)",
        0x200A => "Contrast-detect Zone Select",
        0x200B => "Contrast-detect Spot",
        _ => return None,
    })
}

/// AFInfoK3III (0x040C, K-3 Mark III, GR III and later): AF mode, selection
/// mode, the most areas and the areas recorded, three more words, then seven
/// words per area: frame width and height, centre, size (zero for
/// phase-detect points) and flags (0x4 in focus, 0x8 inside the selection).
fn areas(t: &Tiff, ifd: &Ifd) -> Option<AfInfo> {
    let v = u16s(&t.data(ifd.get(0x040C)?)?, t.le);
    if v.len() < 7 || v[0] == 255 {
        return None;
    }
    let mut focused = Vec::new();
    let mut selected = Vec::new();
    for a in v[7..].as_chunks::<7>().0.iter().take(v[3] as usize) {
        let [fw, fh, x, y, w, h, flags] = a.map(f32::from);
        if fw <= 0.0 || fh <= 0.0 {
            continue;
        }
        let flags = flags as u16;
        let (in_focus, inside) = (flags & 0x4 != 0, flags & 0x8 != 0);
        // Phase-detect points of the K-3 Mark III, as measured on its photos.
        let (w, h) = if w > 0.0 && h > 0.0 { (w / fw, h / fh) } else { (166.0 / 6192.0, 166.0 / 4128.0) };
        let b = AfBox::centered(x / fw, y / fh, w, h, in_focus);
        if in_focus {
            focused.push(b);
        } else if inside {
            selected.push(b);
        }
    }
    let boxes = if focused.is_empty() { selected } else { focused };
    Some(AfInfo { boxes, source: "pentax", mode: selection_name(v[1] as u32).map(str::to_string) })
}

/// Pentax AFPointSelected (0x000E): the point or the centre of the zone, and
/// on newer bodies how far the area reaches around it.
fn selection(t: &Tiff, ifd: &Ifd, layout: Option<&Layout>) -> (Vec<usize>, Option<&'static str>) {
    let v = ifd.get(0x000E).map(|e| t.uints(e)).unwrap_or_default();
    let Some(&first) = v.first() else { return (Vec::new(), None) };
    let second = v.get(1).copied().unwrap_or(0);
    let n = layout.map_or(0, |l| l.points.len()) as u32;
    let wide = n > 11;
    let reach = |r: f32, p: usize| layout.map(|l| l.around(p, r)).unwrap_or_default();
    match first {
        0xFFFF => (layout.map(|l| (1..=l.points.len()).collect()).unwrap_or_default(), Some("Auto")),
        0xFFFE => (layout.map(|l| vec![l.center]).unwrap_or_default(), Some("Fixed Center")),
        0xFFFD => (Vec::new(), Some("Automatic Tracking AF")),
        0xFFFC => (Vec::new(), Some("Face Detect AF")),
        0xFFFB => (Vec::new(), Some("AF Select")),
        p if (1..=n).contains(&p) => match (wide, second) {
            (_, 0) => (vec![p as usize], Some("Single Point")),
            (true, 1) => (reach(1.0, p as usize), Some("Expanded Area (S)")),
            (true, 3) => (reach(2.0, p as usize), Some("Expanded Area (M)")),
            (true, 5) => (reach(9.0, p as usize), Some("Expanded Area (L)")),
            (false, 1) => (reach(1.0, p as usize), Some("Expanded Area")),
            _ => (vec![p as usize], None),
        },
        p if wide && (257..=256 + n).contains(&p) => (reach(1.0, (p - 256) as usize), Some("Zone Select")),
        _ => (Vec::new(), None),
    }
}

/// Points in focus, when the body says.
fn in_focus(t: &Tiff, ifd: &Ifd, layout: &Layout, model: &str) -> Option<Vec<usize>> {
    let model = short_model(model);
    let mask = || ifd.get(0x000F).filter(|e| e.typ == 4).and_then(|e| t.uint(e));
    if std::ptr::eq(layout, &K3) {
        // AFPointsInFocus (0x000F): a bit per point.
        let m = mask()?;
        return Some((0..27).filter(|b| m >> b & 1 == 1).map(|b| b + 1).collect());
    }
    if matches!(model, "K-S1" | "K-S2") {
        // The same mask, with the 11 points at the places of the 27-point bodies' that they share.
        let m = mask()?;
        return Some((0..27).filter(|b| m >> b & 1 == 1).filter_map(|b| ELEVEN.nearest(&K3, b + 1)).collect());
    }
    if std::ptr::eq(layout, &ELEVEN) {
        let d = t.data(ifd.get(0x021F)?)?;
        return eleven_in_focus(*d.get(0x0B)?).map(<[usize]>::to_vec);
    }
    None
}

/// `aspect` is the picture's width over its height.
fn pentax(t: &Tiff, ifd: &Ifd, model: &str, aspect: f32) -> Option<AfInfo> {
    let info = |boxes: Vec<AfBox>, mode: Option<&str>| Some(AfInfo { boxes, source: "pentax", mode: mode.map(str::to_string) });
    if ifd.get(0x040C).is_some() {
        return areas(t, ifd);
    }
    let focus_mode = ifd.get(0x000D).and_then(|e| t.uint(e)).unwrap_or(0);
    if focus_mode == 3 {
        return None;
    }
    if let Some(mode) = contrast_detect(focus_mode) {
        return info(vec![contrast_area(t, ifd)?], Some(mode));
    }
    let layout = layout_of(model);
    let (mut selected, mode) = selection(t, ifd, layout);
    let Some(layout) = layout else {
        // Without the point layout only the centre point can be placed.
        return (mode == Some("Fixed Center")).then(|| AfInfo { boxes: vec![AfBox::centered(0.5, 0.5, DEFAULT_FRAME, DEFAULT_FRAME * aspect, true)], source: "pentax", mode: mode.map(str::to_string) });
    };
    let mut focused = in_focus(t, ifd, layout, model);
    if let Some((f, s)) = point_info(t, ifd, layout) {
        (focused, selected) = (Some(f), s);
    }
    let everything = selected.len() == layout.points.len();
    let boxes: Vec<AfBox> = match focused {
        Some(f) if !f.is_empty() => f.into_iter().filter_map(|n| layout.point(n, true)).collect(),
        // Auto area with nothing in focus says nothing about where the camera looked.
        _ if everything => return None,
        // A single point the camera does not say missed is where it focused.
        f => {
            let single = f.is_none() && selected.len() == 1;
            selected.into_iter().filter_map(|n| layout.point(n, single)).collect()
        }
    };
    info(boxes, mode)
}

/// Ricoh AFAreaXPosition / YPosition (0x1203 / 0x1204): the AF area's centre in
/// pixels of the recorded picture, whose size is CroppedImageWidth / Height
/// (0x1603 / 0x1604) when the camera crops.
fn ricoh(t: &Tiff, ifd: &Ifd, p: &Parsed) -> Option<AfInfo> {
    let get = |tag: u16| ifd.get(tag).and_then(|e| t.uint(e));
    let focus_mode = get(0x1006);
    if focus_mode == Some(1) {
        return None;
    }
    let (x, y) = (get(0x1203)? as f32, get(0x1204)? as f32);
    let (w, h) = match (get(0x1603), get(0x1604)) {
        (Some(w), Some(h)) if w > 0 && h > 0 => (w as f32, h as f32),
        _ => {
            let exif = crate::exif::read(p);
            (exif.width? as f32, exif.height? as f32)
        }
    };
    if x <= 0.0 || y <= 0.0 || x >= w || y >= h {
        return None;
    }
    let mode = focus_mode.and_then(|m| {
        Some(match m {
            2 => "Multi AF",
            3 => "Spot AF",
            4 => "Snap",
            5 => "Infinity",
            7 => "Face Detect",
            8 => "Subject Tracking",
            9 => "Pinpoint AF",
            _ => return None,
        })
    });
    let focused = get(0x1200) != Some(0);
    Some(AfInfo { boxes: vec![AfBox::centered(x / w, y / h, DEFAULT_FRAME, DEFAULT_FRAME * w / h, focused)], source: "ricoh", mode: mode.map(str::to_string) })
}

pub fn parse(mn: &MakerNote, model: &str, p: &Parsed) -> Option<AfInfo> {
    match pentax::maker_ifd(mn)? {
        (Kind::Pentax, t, ifd) => {
            let exif = crate::exif::read(p);
            let aspect = match (exif.width, exif.height) {
                (Some(w), Some(h)) if h > 0 => w as f32 / h as f32,
                _ => 1.5,
            };
            let mut info = pentax(&t, &ifd, model, aspect)?;
            if model.trim_start().starts_with("RICOH") {
                info.source = "ricoh";
            }
            Some(info)
        }
        (Kind::Ricoh, t, ifd) => ricoh(&t, &ifd, p),
    }
}
