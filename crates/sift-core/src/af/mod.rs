//! Camera-recorded autofocus areas, decoded from vendor MakerNotes.
//!
//! Tag layouts follow the ExifTool tag tables (Canon AFInfo2/AFInfo3, Sony
//! FocusLocation/FocusFrameSize, Nikon AFInfo2 v0100-v0402).

mod canon;
mod nikon;
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
    /// Vendor that produced the data: `canon`, `nikon` or `sony`.
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
    } else {
        None
    }?;
    info.boxes = info.boxes.into_iter().filter_map(AfBox::clamp).map(|b| b.oriented(orientation)).collect();
    (!info.boxes.is_empty()).then_some(info)
}
