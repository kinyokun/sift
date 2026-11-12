use super::{AfBox, AfInfo};
use crate::container::{nikon_maker_tiff, MakerNote, Parsed};
use crate::io::u16_at;

/// Fallback AF-area edge as a fraction of frame width.
const DEFAULT_AREA: f32 = 0.04;
/// Pitch between adjacent focus points in AF-image pixels, as measured on
/// Z 7II/Z 8/Z 9 files and used by ExifTool for every Z body.
const POINT_PITCH_X: f32 = 260.0;
const POINT_PITCH_Y: f32 = 290.0;
/// The 9x9 auto-area zones of Expeed 6 bodies span about this much of the frame.
const ZONE_COVERAGE: f32 = 0.9;

fn mode_name(v: u8) -> Option<&'static str> {
    Some(match v {
        192 => "Pinpoint",
        193 => "Single",
        194 => "Dynamic",
        195 => "Wide (S)",
        196 => "Wide (L)",
        197 => "Auto",
        198 => "Auto (People)",
        199 => "Auto (Animal)",
        204 => "Dynamic Area (S)",
        205 => "Dynamic Area (M)",
        206 => "Dynamic Area (L)",
        207 => "3D-tracking",
        208 => "Wide (C1/C2)",
        _ => return None,
    })
}

/// Field offsets of the coordinate block inside one AFInfo2 version.
struct Layout {
    /// AFImageWidth, AFImageHeight, X, Y, AreaWidth, AreaHeight as consecutive u16.
    coords: usize,
    /// Byte telling whether the camera reported focus was achieved.
    result: Option<usize>,
}

/// Auto-area focus-point grid for the AFPointsUsed bitmap of Expeed 7 bodies.
struct Grid {
    cols: usize,
    rows: usize,
    /// Full AF frame, used when the file does not record AFImageWidth/Height.
    frame: (f32, f32),
}

const GRID_45MP: Grid = Grid { cols: 27, rows: 15, frame: (8256.0, 5504.0) };
const GRID_24MP: Grid = Grid { cols: 23, rows: 13, frame: (6048.0, 4032.0) };
const GRID_DX: Grid = Grid { cols: 21, rows: 11, frame: (5568.0, 3712.0) };

fn auto_grid(version: &[u8], model: &str) -> Option<Grid> {
    let m = model.to_ascii_uppercase();
    let is = |names: &[&str]| names.iter().any(|n| m.ends_with(n));
    // The model decides: the Z5 II writes the same version tag as the DX Z50 II.
    if is(&["Z 8", "Z 9"]) {
        Some(GRID_45MP)
    } else if is(&["Z6_3", "Z F", "Z5_2"]) {
        Some(GRID_24MP)
    } else if is(&["Z50_2"]) {
        Some(GRID_DX)
    } else {
        match version {
            b"0400" => Some(GRID_45MP),
            b"0401" => Some(GRID_24MP),
            b"0402" => Some(GRID_DX),
            _ => None,
        }
    }
}

/// Nikon AFInfo2 (MakerNote tag 0x00B7).
pub fn parse(mn: &MakerNote, model: &str, p: &Parsed) -> Option<AfInfo> {
    let (t, ifd_off) = nikon_maker_tiff(mn.parent.src, mn)?;
    let ifd = t.ifd(ifd_off)?;
    let d = t.data(ifd.get(0x00B7)?)?;
    if d.len() < 8 {
        return None;
    }
    let version = &d[..4];
    let detection = d[4]; // 0 phase detect (viewfinder), 1 contrast detect, 2 hybrid
    let u16f = |o: usize| u16_at(&d, o, t.le).unwrap_or(0) as f32;

    let (layout, coords_available, mode) = match version {
        b"0100" if detection == 1 => (Layout { coords: 0x10, result: Some(0x1c) }, true, None),
        b"0101" if detection == 1 => (Layout { coords: 0x46, result: Some(0x52) }, true, None),
        b"0300" | b"0301" => (Layout { coords: 0x2a, result: None }, d[7] == 1, mode_name(d[5])),
        b"0400" | b"0401" | b"0402" => (Layout { coords: 0x3e, result: Some(0x4a) }, d[7] == 1, mode_name(d[5])),
        _ => return None,
    };
    let focused = layout.result.and_then(|o| d.get(o)).is_none_or(|&v| v != 0);
    let mode = mode.map(str::to_string);

    // Frame the AF coordinates refer to, and the crop actually recorded.
    let crop = ifd.get(0x0045).map(|e| t.uints(e)).filter(|c| c.len() == 4 && c[2] > 0 && c[3] > 0);
    let exif_dims = crate::exif::read(p);
    let (out_w, out_h) = match &crop {
        Some(c) => (c[2] as f32, c[3] as f32),
        None => (exif_dims.width.unwrap_or(0) as f32, exif_dims.height.unwrap_or(0) as f32),
    };
    let grid = auto_grid(version, model);
    let (mut af_w, mut af_h) = (u16f(layout.coords), u16f(layout.coords + 2));
    if af_w < 16.0 || af_h < 16.0 {
        (af_w, af_h) = grid.as_ref().map_or((out_w, out_h), |g| g.frame);
    }
    if af_w < 16.0 || af_h < 16.0 {
        return None;
    }
    // DX and other in-camera crops keep reporting positions in the full AF
    // frame; the recorded image is its centre section.
    let (crop_w, crop_h) = if out_w >= 16.0 && out_w < af_w * 0.98 { (out_w, out_h) } else { (af_w, af_h) };
    let place = |cx: f32, cy: f32, w: f32, h: f32, focused: bool| {
        AfBox::centered(
            (cx - (af_w - crop_w) / 2.0) / crop_w,
            (cy - (af_h - crop_h) / 2.0) / crop_h,
            w / crop_w,
            h / crop_h,
            focused,
        )
    };

    let (x, y) = (u16f(layout.coords + 4), u16f(layout.coords + 6));
    if coords_available && x > 0.0 && y > 0.0 {
        let (mut w, mut h) = (u16f(layout.coords + 8), u16f(layout.coords + 10));
        if w <= 0.0 || h <= 0.0 {
            w = af_w * DEFAULT_AREA;
            h = w;
        }
        return Some(AfInfo { boxes: vec![place(x, y, w, h, focused)], source: "nikon", mode });
    }

    // No coordinates: the camera recorded which auto-area points were used.
    let bitmap = d.get(0x0a..)?;
    let set = |i: usize| bitmap.get(i / 8).is_some_and(|b| b & (1 << (i % 8)) != 0);
    let mut boxes = Vec::new();
    if matches!(version, b"0300" | b"0301") && d[6] == 8 {
        // 81 zones, bit n+1 walks columns 5,6,4,7,3,8,2,9,1 and rows E,D,C,B,A,F,G,H,I.
        const COLS: [f32; 9] = [4.0, 5.0, 3.0, 6.0, 2.0, 7.0, 1.0, 8.0, 0.0];
        const ROWS: [f32; 9] = [4.0, 3.0, 2.0, 1.0, 0.0, 5.0, 6.0, 7.0, 8.0];
        let (zw, zh) = (af_w * ZONE_COVERAGE / 9.0, af_h * ZONE_COVERAGE / 9.0);
        for i in (0..81).filter(|&i| set(i)) {
            let cx = af_w / 2.0 + (COLS[i / 9] - 4.0) * zw;
            let cy = af_h / 2.0 + (ROWS[i % 9] - 4.0) * zh;
            boxes.push(place(cx, cy, zw * 0.92, zh * 0.92, focused));
        }
    } else if let Some(Grid { cols, rows, .. }) = grid {
        let side = POINT_PITCH_X * 0.8;
        for i in (0..cols * rows).filter(|&i| set(i)) {
            let (col, row) = ((i % cols) as f32, (i / cols) as f32);
            let cx = af_w / 2.0 + (col - (cols as f32 - 1.0) / 2.0) * POINT_PITCH_X;
            let cy = af_h / 2.0 + (row - (rows as f32 - 1.0) / 2.0) * POINT_PITCH_Y;
            boxes.push(place(cx, cy, side, side, focused));
        }
    }
    (!boxes.is_empty()).then_some(AfInfo { boxes, source: "nikon", mode })
}
