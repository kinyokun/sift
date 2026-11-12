use super::{AfBox, AfInfo};
use crate::container::{u16s, MakerNote};

/// Tracking modes on mirrorless bodies can report hundreds of tiny cells.
const MAX_BOXES: usize = 512;
/// Largest phase-detect viewfinder array Canon has shipped (EOS-1D X II / 7D II class).
const DSLR_MAX_POINTS: usize = 65;
/// Edge of a viewfinder AF point as a fraction of frame width.
const NOMINAL_POINT: f32 = 0.035;

fn mode_name(v: u16) -> Option<&'static str> {
    Some(match v {
        0 => "Manual Focus",
        1 => "AF Point Expansion",
        2 => "Single-point AF",
        4 => "Auto",
        5 => "Face Detect AF",
        6 => "Face + Tracking",
        7 => "Zone AF",
        8 => "AF Point Expansion (4 point)",
        9 => "Spot AF",
        10 => "AF Point Expansion (8 point)",
        11 | 12 => "Flexizone Multi",
        13 => "1-point AF",
        14 | 16 | 17 => "Large Zone AF",
        19..=21 => "Flexible Zone AF",
        22 => "Whole Area AF",
        _ => return None,
    })
}

/// Canon AFInfo2 (0x0026) / AFInfo3 (0x003C): a serial array of 16-bit words.
pub fn parse(mn: &MakerNote, model: &str) -> Option<AfInfo> {
    let t = &mn.parent;
    let ifd = t.ifd_at(mn.pos)?;
    let e = ifd.get(0x0026).or_else(|| ifd.get(0x003C))?;
    let v = u16s(&t.data(e)?, t.le);
    if v.len() < 8 {
        return None;
    }
    let n = v[2] as usize;
    let (af_w, af_h) = (v[6] as f32, v[7] as f32);
    if n == 0 || af_w <= 0.0 || af_h <= 0.0 || v.len() < 8 + n * 4 {
        return None;
    }
    let words = n.div_ceil(16);
    let widths = &v[8..8 + n];
    let heights = &v[8 + n..8 + 2 * n];
    let xs = &v[8 + 2 * n..8 + 3 * n];
    let ys = &v[8 + 3 * n..8 + 4 * n];
    let bit = |base: usize, k: usize| v.get(base + k / 16).is_some_and(|w| (w >> (k % 16)) & 1 == 1);
    let in_focus = 8 + 4 * n;
    let selected = in_focus + words;

    // EOS bodies store Y pointing up from the frame centre; compacts point down.
    let y_sign = if model.to_ascii_uppercase().contains("EOS") { -1.0 } else { 1.0 };
    // Viewfinder AF arrays of older DSLRs (EOS 6D, ...) report area sizes that
    // are several times larger than the real points; draw those at a nominal size.
    let dslr_array = n <= DSLR_MAX_POINTS;
    let make_box = |k: usize, focused: bool| {
        let (mut w, mut h) = (widths[k] as i16 as f32, heights[k] as i16 as f32);
        if w <= 0.0 || h <= 0.0 || (dslr_array && (w > af_w * 0.08 || h > af_h * 0.12)) {
            w = af_w * NOMINAL_POINT;
            h = w;
        }
        let cx = af_w / 2.0 + xs[k] as i16 as f32;
        let cy = af_h / 2.0 + y_sign * ys[k] as i16 as f32;
        AfBox::centered(cx / af_w, cy / af_h, w / af_w, h / af_h, focused)
    };

    let mut boxes: Vec<AfBox> = (0..n).filter(|&k| bit(in_focus, k)).map(|k| make_box(k, true)).collect();
    if boxes.is_empty() {
        boxes = (0..n).filter(|&k| bit(selected, k)).map(|k| make_box(k, false)).collect();
    }
    boxes.truncate(MAX_BOXES);
    Some(AfInfo { boxes, source: "canon", mode: mode_name(v[1]).map(str::to_string) })
}
