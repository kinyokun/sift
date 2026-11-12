use super::{AfBox, AfInfo};
use crate::container::{sony_maker_ifd, u16s, Parsed};

/// Fallback focus-frame edge as a fraction of image width when the camera
/// records only a location.
const DEFAULT_FRAME: f32 = 0.04;

pub fn parse(p: &Parsed) -> Option<AfInfo> {
    let (t, ifd) = sony_maker_ifd(p)?;
    let vals = |tag: u16| ifd.get(tag).map(|e| t.uints(e));

    // FocusLocation: image width, image height, x, y (origin top-left).
    let loc = vals(0x2027).or_else(|| vals(0x204A))?;
    if loc.len() < 4 {
        return None;
    }
    let (w, h, x, y) = (loc[0] as f32, loc[1] as f32, loc[2] as f32, loc[3] as f32);
    if w < 16.0 || h < 16.0 || x <= 0.0 || y <= 0.0 || x >= w || y >= h {
        return None;
    }
    // FocusFrameSize: width, height, validity flag.
    // Stored as undefined bytes, so decode the three little/big-endian words by hand.
    let frame = ifd.get(0x2037).and_then(|e| t.data(e)).map(|d| u16s(&d, t.le).into_iter().map(u32::from).collect::<Vec<_>>());
    let (fw, fh) = match frame {
        Some(f) if f.len() >= 3 && f[2] != 0 && f[0] > 0 && f[1] > 0 => (f[0] as f32, f[1] as f32),
        _ => (w * DEFAULT_FRAME, w * DEFAULT_FRAME),
    };
    Some(AfInfo {
        boxes: vec![AfBox::centered(x / w, y / h, fw / w, fh / h, true)],
        source: "sony",
        mode: None,
    })
}
