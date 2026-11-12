//! Colour looks that can be applied to a photo: lookup tables (Adobe
//! `.cube`, Autodesk `.3dl`, HaldCLUT `.png`) and Nikon Picture Controls
//! (`.NP3`, the format of Imaging Cloud recipes).
//!
//! Every look becomes one sRGB -> sRGB 3D lookup table. Lookup-table files
//! are applied exactly. NP3 files only store slider values; Nikon's own maths is
//! not public, so [`Np3::to_lut`] is an approximation of the camera's
//! rendering, good for judging a look but not identical to in-camera output.
//! Field offsets follow the MIT-licensed `nikon-flexible-color-picture-control`
//! project by ssssota.

use crate::{Error, Result};
use image::RgbImage;
use std::path::Path;

/// Grid size used for looks generated from slider values.
const GRID: usize = 33;

pub struct Lut3 {
    size: usize,
    /// `size^3` RGB triples in 0..1, red varying fastest.
    data: Vec<[f32; 3]>,
}

impl Lut3 {
    fn from_fn(size: usize, f: impl Fn([f32; 3]) -> [f32; 3]) -> Lut3 {
        let step = 1.0 / (size - 1) as f32;
        let mut data = Vec::with_capacity(size * size * size);
        for b in 0..size {
            for g in 0..size {
                for r in 0..size {
                    data.push(f([r as f32 * step, g as f32 * step, b as f32 * step]));
                }
            }
        }
        Lut3 { size, data }
    }

    fn from_fn_index(size: usize, f: impl Fn(usize, usize, usize) -> [f32; 3]) -> Lut3 {
        let mut data = Vec::with_capacity(size * size * size);
        for b in 0..size {
            for g in 0..size {
                for r in 0..size {
                    data.push(f(r, g, b));
                }
            }
        }
        Lut3 { size, data }
    }

    fn at(&self, r: usize, g: usize, b: usize) -> [f32; 3] {
        self.data[(b * self.size + g) * self.size + r]
    }

    /// Trilinear lookup of one 8-bit pixel.
    fn map(&self, px: [u8; 3]) -> [u8; 3] {
        let top = (self.size - 1) as f32;
        let pos = px.map(|v| v as f32 / 255.0 * top);
        let lo = pos.map(|p| (p as usize).min(self.size - 2));
        let f = [pos[0] - lo[0] as f32, pos[1] - lo[1] as f32, pos[2] - lo[2] as f32];
        let mut out = [0f32; 3];
        for (db, wb) in [(0, 1.0 - f[2]), (1, f[2])] {
            for (dg, wg) in [(0, 1.0 - f[1]), (1, f[1])] {
                for (dr, wr) in [(0, 1.0 - f[0]), (1, f[0])] {
                    let v = self.at(lo[0] + dr, lo[1] + dg, lo[2] + db);
                    let w = wr * wg * wb;
                    for c in 0..3 {
                        out[c] += v[c] * w;
                    }
                }
            }
        }
        out.map(|v| (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u8)
    }

    /// Applies the look to every pixel, using all CPU cores.
    pub fn apply(&self, img: &mut RgbImage) {
        let row = img.width() as usize * 3;
        if row == 0 {
            return;
        }
        let workers = std::thread::available_parallelism().map_or(4, |n| n.get());
        let rows_per_part = (img.height() as usize).div_ceil(workers).max(1);
        std::thread::scope(|s| {
            for part in img.as_flat_samples_mut().samples.chunks_mut(rows_per_part * row) {
                s.spawn(move || {
                    for px in part.as_chunks_mut::<3>().0 {
                        *px = self.map(*px);
                    }
                });
            }
        });
    }
}

/// Parses an Adobe/Resolve `.cube` file (3D, or 1D applied per channel).
pub fn parse_cube(text: &str) -> Result<Lut3> {
    let bad = || Error::Decode("not a valid .cube lookup table".into());
    let (mut size3, mut size1) = (0usize, 0usize);
    let (mut lo, mut hi) = ([0f32; 3], [1f32; 3]);
    let mut rows: Vec<[f32; 3]> = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with("TITLE") {
            continue;
        }
        let mut words = line.split_whitespace();
        let first = words.next().unwrap_or("");
        let nums: Vec<f32> = words.filter_map(|w| w.parse().ok()).collect();
        match first {
            "LUT_3D_SIZE" => size3 = *nums.first().ok_or_else(bad)? as usize,
            "LUT_1D_SIZE" => size1 = *nums.first().ok_or_else(bad)? as usize,
            "DOMAIN_MIN" if nums.len() == 3 => lo = [nums[0], nums[1], nums[2]],
            "DOMAIN_MAX" if nums.len() == 3 => hi = [nums[0], nums[1], nums[2]],
            _ => {
                if let (Ok(r), 2) = (first.parse::<f32>(), nums.len()) {
                    rows.push([r, nums[0], nums[1]]);
                }
            }
        }
    }
    // Output values are stored relative to the declared domain.
    let norm = |v: [f32; 3]| [0, 1, 2].map(|c| (v[c] - lo[c]) / (hi[c] - lo[c]).max(1e-6));
    if (2..=256).contains(&size3) && rows.len() == size3 * size3 * size3 {
        return Ok(Lut3 { size: size3, data: rows.into_iter().map(norm).collect() });
    }
    if size1 >= 2 && rows.len() == size1 {
        let curve = |c: usize, x: f32| {
            let pos = x.clamp(0.0, 1.0) * (size1 - 1) as f32;
            let i = (pos as usize).min(size1 - 2);
            let (a, b) = (norm(rows[i])[c], norm(rows[i + 1])[c]);
            a + (b - a) * (pos - i as f32)
        };
        return Ok(Lut3::from_fn(GRID, |p| [curve(0, p[0]), curve(1, p[1]), curve(2, p[2])]));
    }
    Err(bad())
}

/// Parses an Autodesk/Lustre `.3dl` table: an optional line of input mesh
/// points, then one integer triple per entry with blue varying fastest.
pub fn parse_3dl(text: &str) -> Result<Lut3> {
    let bad = || Error::Decode("not a valid .3dl lookup table".into());
    let mut declared_depth = None;
    let mut rows: Vec<[f32; 3]> = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let words: Vec<&str> = line.split_whitespace().collect();
        if words[0].eq_ignore_ascii_case("mesh") {
            // "Mesh <input bits> <output bits>"
            declared_depth = words.get(2).and_then(|w| w.parse::<u32>().ok());
            continue;
        }
        let nums: Vec<f32> = words.iter().filter_map(|w| w.parse().ok()).collect();
        if nums.len() == words.len() && nums.len() == 3 {
            rows.push([nums[0], nums[1], nums[2]]);
        }
        // Anything else is a header: the mesh point line, "3DMESH", "LUT8", "gamma 1.0".
    }
    let size = (rows.len() as f64).cbrt().round() as usize;
    if size < 2 || size * size * size != rows.len() {
        return Err(bad());
    }
    let peak = rows.iter().flatten().fold(0f32, |m, v| m.max(*v));
    let max = match declared_depth {
        Some(bits @ 8..=16) => ((1u32 << bits) - 1) as f32,
        _ => [255.0, 1023.0, 4095.0, 65535.0].into_iter().find(|m| peak <= *m).ok_or_else(bad)?,
    };
    // Reorder from blue-fastest to our red-fastest layout.
    Ok(Lut3::from_fn_index(size, |r, g, b| rows[(r * size + g) * size + b].map(|v| v / max)))
}

/// Largest HaldCLUT level accepted (level 12 is a 144-point cube).
const MAX_HALD_LEVEL: u32 = 12;

/// Reads a HaldCLUT: a square image of `level^3` pixels a side holding a
/// `level^2`-point cube, red varying fastest.
pub fn parse_hald(bytes: &[u8]) -> Result<Lut3> {
    let bad = || Error::Decode("not a HaldCLUT image (it must be square, level\u{b3} pixels wide)".into());
    let img = image::load_from_memory(bytes).map_err(|_| bad())?.into_rgb16();
    let level = (img.width() as f64).cbrt().round() as u32;
    if img.width() != img.height() || !(2..=MAX_HALD_LEVEL).contains(&level) || level.pow(3) != img.width() {
        return Err(bad());
    }
    let data = img.pixels().map(|p| p.0.map(|v| v as f32 / 65535.0)).collect();
    Ok(Lut3 { size: (level * level) as usize, data })
}

/// Hue, chroma and brightness sliders of one colour band or tonal zone.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Hcb {
    pub hue: f32,
    pub chroma: f32,
    pub brightness: f32,
}

/// Slider values of a Nikon Flexible Color Picture Control.
#[derive(Clone, Debug, Default)]
pub struct Np3 {
    pub name: String,
    pub contrast: f32,
    pub highlights: f32,
    pub shadows: f32,
    pub white_level: f32,
    pub black_level: f32,
    pub saturation: f32,
    /// Red, orange, yellow, green, cyan, blue, purple, magenta.
    pub blender: [Hcb; 8],
    /// Highlights, mid-tones, shadows; `hue` is an absolute angle here.
    pub grading: [Hcb; 3],
    pub blending: f32,
    pub balance: f32,
    /// Custom tone curve: 257 samples in 0..=32767.
    pub curve: Option<Vec<u16>>,
    /// Global hue rotation in degrees (classic Picture Controls only).
    pub hue_shift: f32,
    /// Base Picture Control code as stored in the file.
    pub base: u16,
    /// Classic sliders as stored (contrast, brightness, saturation, hue), if set.
    pub classic: [Option<f32>; 4],
    /// Sharpening, mid-range sharpening and clarity as stored, if set.
    pub detail: [Option<f32>; 3],
    /// The file itself, for [`crate::nikon::record_look`].
    pub file: Vec<u8>,
}

/// Hue centres of the colour blender bands, in degrees.
const BAND_HUES: [f32; 8] = [0.0, 30.0, 60.0, 120.0, 180.0, 240.0, 270.0, 300.0];

// Record tags. The file is "NCP\0" followed by (tag u32, length u32, value)
// records, all big-endian, ending with a zero tag.
const TAG_NAME: u32 = 0x200;
const TAG_BASE: u32 = 0x300;
const TAG_CURVE: u32 = 2;
/// Classic Picture Controls: contrast, brightness, saturation, hue.
const TAGS_CLASSIC: [u32; 4] = [0x800, 0x900, 0xa00, 0xb00];
/// Sharpening, mid-range sharpening, clarity.
const TAGS_DETAIL: [u32; 3] = [0x600, 0x1600, 0x700];
/// Flexible Color: contrast, highlights, shadows, white, black, saturation.
const TAGS_FLEXIBLE: [u32; 6] = [0x1900, 0x1a00, 0x1b00, 0x1c00, 0x1d00, 0x1e00];
const TAG_BLENDER: u32 = 0x1f00;
const TAG_GRADING: u32 = 0x2000;
/// Samples in a custom tone curve; they are the last bytes of its record.
const CURVE_POINTS: usize = 257;

/// A slider byte: 0x80 is zero. 0x00, 0x01 and 0xFF mean "not set".
fn slider_opt(b: u8) -> Option<f32> {
    (!matches!(b, 0x00 | 0x01 | 0xff)).then_some(b as f32 - 128.0)
}

fn slider(b: u8) -> f32 {
    slider_opt(b).unwrap_or(0.0)
}

/// Contrast and saturation the built-in base looks add over a plain rendering.
/// The real base curves live in camera firmware; these are rough stand-ins.
fn base_offsets(base: u16) -> (f32, f32) {
    match base {
        0x00c3 => (22.0, 28.0),  // Vivid
        0x03c2 => (-18.0, -8.0), // Neutral
        _ => (0.0, 0.0),         // Standard, Flexible Color and others
    }
}

pub fn parse_np3(buf: &[u8]) -> Result<Np3> {
    if buf.len() < 12 || !buf.starts_with(b"NCP") {
        return Err(Error::Decode("not a Nikon Picture Control file".into()));
    }
    let mut np3 = Np3 { file: buf.to_vec(), ..Np3::default() };
    let (mut classic, mut base) = ([0f32; 4], 0u16);
    let mut pos = 4;
    while pos + 8 <= buf.len() {
        let word = |o: usize| u32::from_be_bytes(buf[o..o + 4].try_into().unwrap());
        let (tag, len) = (word(pos), word(pos + 4) as usize);
        if tag == 0 || pos + 8 + len > buf.len() {
            break;
        }
        let v = &buf[pos + 8..pos + 8 + len];
        pos += 8 + len;
        // Two-byte sliders are (value, divisor).
        let stored = || slider_opt(v[0]).map(|s| s / (*v.get(1).unwrap_or(&1)).max(1) as f32);
        let scaled = || stored().unwrap_or(0.0);
        match tag {
            TAG_NAME => np3.name = String::from_utf8_lossy(&v[..v.iter().position(|&b| b == 0).unwrap_or(v.len())]).trim().to_string(),
            TAG_BASE if len >= 2 => base = u16::from_be_bytes([v[0], v[1]]),
            TAG_CURVE if len >= CURVE_POINTS * 2 => {
                let raw = &v[len - CURVE_POINTS * 2..];
                np3.curve = Some(raw.as_chunks::<2>().0.iter().map(|c| u16::from_be_bytes(*c)).collect());
            }
            TAG_BLENDER if len >= 24 => {
                for (i, band) in np3.blender.iter_mut().enumerate() {
                    *band = Hcb { hue: slider(v[i * 3]), chroma: slider(v[i * 3 + 1]), brightness: slider(v[i * 3 + 2]) };
                }
            }
            TAG_GRADING if len >= 19 => {
                for (i, zone) in np3.grading.iter_mut().enumerate() {
                    let o = i * 4;
                    *zone = Hcb { hue: (((v[o] & 0x0f) as u16) << 8 | v[o + 1] as u16) as f32, chroma: slider(v[o + 2]), brightness: slider(v[o + 3]) };
                }
                np3.blending = slider(v[16]);
                np3.balance = slider(v[18]);
            }
            _ if len == 2 => {
                if let Some(i) = TAGS_DETAIL.iter().position(|t| *t == tag) {
                    np3.detail[i] = stored();
                } else if let Some(i) = TAGS_CLASSIC.iter().position(|t| *t == tag) {
                    classic[i] = scaled();
                    np3.classic[i] = stored();
                } else if let Some(i) = TAGS_FLEXIBLE.iter().position(|t| *t == tag) {
                    let field = [&mut np3.contrast, &mut np3.highlights, &mut np3.shadows, &mut np3.white_level, &mut np3.black_level, &mut np3.saturation];
                    *field.into_iter().nth(i).unwrap() = scaled();
                }
            }
            _ => {}
        }
    }
    np3.base = base;
    // Classic sliders run -3..+3; bring them onto the -100..+100 scale.
    let (base_contrast, base_saturation) = base_offsets(base);
    np3.contrast += classic[0] * 30.0 + base_contrast;
    np3.shadows += classic[1] * 25.0;
    np3.highlights += classic[1] * 15.0;
    np3.saturation += classic[2] * 30.0 + base_saturation;
    np3.hue_shift = classic[3] * 3.0;
    if np3.curve.is_some() {
        // A custom curve replaces the contrast slider (and the base curve it would bend).
        np3.contrast = 0.0;
    }
    Ok(np3)
}

fn luma(p: [f32; 3]) -> f32 {
    0.2126 * p[0] + 0.7152 * p[1] + 0.0722 * p[2]
}

fn smooth(a: f32, b: f32, x: f32) -> f32 {
    let t = ((x - a) / (b - a)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// RGB -> (hue in degrees, saturation, value).
fn to_hsv(p: [f32; 3]) -> (f32, f32, f32) {
    let (max, min) = (p[0].max(p[1]).max(p[2]), p[0].min(p[1]).min(p[2]));
    let d = max - min;
    let h = if d < 1e-6 {
        0.0
    } else if max == p[0] {
        60.0 * ((p[1] - p[2]) / d).rem_euclid(6.0)
    } else if max == p[1] {
        60.0 * ((p[2] - p[0]) / d + 2.0)
    } else {
        60.0 * ((p[0] - p[1]) / d + 4.0)
    };
    (h, if max > 1e-6 { d / max } else { 0.0 }, max)
}

fn from_hsv(h: f32, s: f32, v: f32) -> [f32; 3] {
    let f = |n: f32| {
        let k = (n + h.rem_euclid(360.0) / 60.0) % 6.0;
        v - v * s * k.min(4.0 - k).clamp(0.0, 1.0)
    };
    [f(5.0), f(3.0), f(1.0)]
}

impl Np3 {
    /// Tone response on luminance: the custom curve if present, else the sliders.
    fn tone(&self, y: f32) -> f32 {
        if let Some(curve) = &self.curve {
            let pos = y.clamp(0.0, 1.0) * 256.0;
            let i = (pos as usize).min(255);
            let (a, b) = (curve[i] as f32, curve[i + 1] as f32);
            return (a + (b - a) * (pos - i as f32)) / 32767.0;
        }
        let mut y = y;
        // White/black level move the end points.
        let (black, white) = (self.black_level / 100.0 * 0.12, 1.0 + self.white_level / 100.0 * 0.15);
        y = black + y * (white - black);
        // Highlights and shadows lift or lower their half of the range.
        y += self.highlights / 100.0 * 0.22 * smooth(0.45, 1.0, y) * (1.0 - smooth(0.85, 1.15, y) * 0.6);
        y += self.shadows / 100.0 * 0.22 * (1.0 - smooth(0.0, 0.55, y)) * smooth(-0.1, 0.2, y);
        // Contrast pivots around mid grey.
        let k = self.contrast / 100.0;
        let s = y.clamp(0.0, 1.0);
        let curved = s * s * (3.0 - 2.0 * s);
        if k >= 0.0 {
            y + (curved - s) * k
        } else {
            y + (s - curved) * k * -0.6 + (0.5 - y) * -k * 0.25
        }
    }

    fn pixel(&self, p: [f32; 3]) -> [f32; 3] {
        // 1. Tone, channel by channel like a film curve. (Scaling all channels
        //    by a luminance gain blows saturated shadows out.)
        let mut p = p.map(|v| self.tone(v).clamp(0.0, 1.0));

        // 2. Global saturation around luminance.
        let sat = (1.0 + self.saturation / 100.0 * 0.6).max(0.0);
        let y = luma(p);
        p = p.map(|v| y + (v - y) * sat);

        // 3. Colour blender: per-hue shifts, fading out towards grey.
        let (h, s, v) = to_hsv(p.map(|c| c.clamp(0.0, 1.0)));
        let (mut dh, mut ds, mut dv) = (0.0, 0.0, 0.0);
        for (i, band) in self.blender.iter().enumerate() {
            let prev = BAND_HUES[(i + 7) % 8];
            let next = BAND_HUES[(i + 1) % 8];
            let centre = BAND_HUES[i];
            let dist = (h - centre + 540.0).rem_euclid(360.0) - 180.0;
            let width = if dist < 0.0 { (centre - prev).rem_euclid(360.0) } else { (next - centre).rem_euclid(360.0) };
            let w = (1.0 - dist.abs() / width).max(0.0);
            dh += w * band.hue / 100.0 * 30.0;
            ds += w * band.chroma / 100.0 * 0.8;
            dv += w * band.brightness / 100.0 * 0.25;
        }
        let colourful = smooth(0.02, 0.25, s);
        p = from_hsv(h + self.hue_shift + dh * colourful, (s * (1.0 + ds * colourful)).clamp(0.0, 1.0), (v * (1.0 + dv * colourful)).clamp(0.0, 1.0));

        // 4. Colour grading: tint highlights, mid-tones and shadows.
        let y = luma(p);
        let split = 0.5 + self.balance / 100.0 * 0.25;
        let soft = 0.18 + (self.blending + 100.0) / 200.0 * 0.3;
        let high = smooth(split - soft * 0.2, split + soft, y);
        let low = 1.0 - smooth(split - soft, split + soft * 0.2, y);
        let mid = (1.0 - high - low).max(0.0) + high.min(low);
        for (zone, w) in self.grading.iter().zip([high, mid, low]) {
            let tint = from_hsv(zone.hue, 1.0, 1.0);
            let tint_luma = luma(tint);
            let amount = zone.chroma / 100.0 * 0.5 * w;
            for c in 0..3 {
                p[c] += (tint[c] - tint_luma) * amount + zone.brightness / 100.0 * 0.08 * w;
            }
        }
        p.map(|v| v.clamp(0.0, 1.0))
    }

    pub fn to_lut(&self) -> Lut3 {
        Lut3::from_fn(GRID, |p| self.pixel(p))
    }

    /// The same look as settings for Nikon's own renderer. The SDK takes no
    /// curve data, so a look with a custom tone curve also carries the file,
    /// to be recorded in a copy of the NEF and developed as shot. Where that
    /// is not possible ([`crate::nikon::Rendered::recorded`] is false),
    /// [`Np3::apply_curve`] has to follow the render.
    pub fn recipe(&self) -> crate::nikon::Recipe {
        use crate::nikon::{Base, Recipe};
        let base = match self.base {
            0x0020 => Base::FlexibleColor,
            0x03c2 => Base::Neutral,
            0x00c3 => Base::Vivid,
            0x064d => Base::Monochrome,
            0x0486 => Base::Portrait,
            0x04c7 => Base::Landscape,
            _ => Base::Standard,
        };
        let mut recipe = Recipe::base(base);
        recipe.recorded = Some(self.file.clone());
        let raw = |v: Option<f32>| v.map_or(f64::NAN, f64::from);
        recipe.classic = self.classic.map(raw);
        recipe.detail = self.detail.map(raw);
        let hcb = |h: &Hcb| [h.hue, h.chroma, h.brightness].map(f64::from);
        let values = [self.contrast, self.highlights, self.shadows, self.white_level, self.black_level, self.saturation]
            .map(f64::from)
            .into_iter()
            .chain(self.blender.iter().flat_map(hcb))
            .chain(self.grading.iter().flat_map(hcb))
            .chain([self.blending, self.balance].map(f64::from));
        if base == Base::FlexibleColor {
            for (slot, v) in recipe.flexible.iter_mut().zip(values) {
                *slot = v;
            }
        }
        recipe
    }

    /// Applies the custom tone curve the way the camera does: on luminance,
    /// leaving the colour differences alone (so darkening adds saturation).
    pub fn apply_curve(&self, img: &mut RgbImage) {
        let Some(curve) = &self.curve else { return };
        let table: Vec<f32> = (0..=255u32)
            .map(|y| {
                let pos = y as f32 / 255.0 * 256.0;
                let i = (pos as usize).min(255);
                let (a, b) = (curve[i] as f32, curve[i + 1] as f32);
                (a + (b - a) * (pos - i as f32)) / 32767.0 * 255.0 - y as f32
            })
            .collect();
        for px in img.as_flat_samples_mut().samples.as_chunks_mut::<3>().0 {
            let y = (0.299 * px[0] as f32 + 0.587 * px[1] as f32 + 0.114 * px[2] as f32).round().clamp(0.0, 255.0);
            let shift = table[y as usize];
            *px = px.map(|v| (v as f32 + shift).round().clamp(0.0, 255.0) as u8);
        }
    }
}

pub struct Look {
    pub name: String,
    pub lut: Lut3,
    /// True when the table approximates the look instead of reproducing it.
    pub approximate: bool,
    /// The Picture Control behind the look, for Nikon's own renderer.
    pub np3: Option<Np3>,
}

pub fn is_look_file(path: &Path) -> bool {
    matches!(crate::formats::ext_of(path).as_str(), "cube" | "3dl" | "png" | "np3")
}

pub fn load(path: &Path) -> Result<Look> {
    let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("Look").to_string();
    match crate::formats::ext_of(path).as_str() {
        "cube" => Ok(Look { name: stem, lut: parse_cube(&String::from_utf8_lossy(&std::fs::read(path)?))?, approximate: false, np3: None }),
        "np3" => {
            let np3 = parse_np3(&std::fs::read(path)?)?;
            Ok(Look { name: if np3.name.is_empty() { stem } else { np3.name.clone() }, lut: np3.to_lut(), approximate: true, np3: Some(np3) })
        }
        "3dl" => Ok(Look { name: stem, lut: parse_3dl(&String::from_utf8_lossy(&std::fs::read(path)?))?, approximate: false, np3: None }),
        "png" => Ok(Look { name: stem, lut: parse_hald(&std::fs::read(path)?)?, approximate: false, np3: None }),
        _ => Err(Error::Unsupported("looks are .cube, .3dl, HaldCLUT .png or .NP3 files")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rec(tag: u32, value: &[u8]) -> Vec<u8> {
        [&tag.to_be_bytes()[..], &(value.len() as u32).to_be_bytes(), value].concat()
    }

    /// A Flexible Color file with the given contrast and saturation sliders.
    fn np3(contrast: i8, saturation: i8) -> Vec<u8> {
        let byte = |v: i8| (128 + v as i16) as u8;
        let mut name = b"Test".to_vec();
        name.resize(20, 0);
        [
            b"NCP\0".to_vec(),
            rec(0x100, b"0310"),
            rec(0x200, &name),
            rec(0x300, &[0x00, 0x20]),
            rec(0x600, &[0xff, 4]), // "not set" must not read as +127
            rec(0x1900, &[byte(contrast), 1]),
            rec(0x1e00, &[byte(saturation), 1]),
            rec(0x1f00, &[0x80; 28]),
            0u32.to_be_bytes().to_vec(),
        ]
        .concat()
    }

    #[test]
    fn neutral_picture_control_leaves_colours_alone() {
        let parsed = parse_np3(&np3(0, 0)).unwrap();
        assert_eq!(parsed.name, "Test");
        let lut = parsed.to_lut();
        for px in [[0, 0, 0], [255, 255, 255], [200, 40, 90], [12, 180, 240], [128, 128, 128]] {
            let out = lut.map(px);
            for c in 0..3 {
                assert!((out[c] as i32 - px[c] as i32).abs() <= 1, "{px:?} -> {out:?}");
            }
        }
    }

    #[test]
    fn sliders_move_the_picture_the_right_way() {
        let lut = parse_np3(&np3(50, 60)).unwrap().to_lut();
        let out = lut.map([180, 110, 100]);
        assert!(out[0] as i32 - out[2] as i32 > 80, "more saturated: {out:?}");
        assert!(lut.map([60, 60, 60])[0] < 60 && lut.map([200, 200, 200])[0] > 200, "more contrast");
    }

    #[test]
    fn classic_files_and_custom_curves_are_read_as_records() {
        // Classic Vivid-based control: no Flexible Color records, a custom curve that inverts tones.
        let curve: Vec<u8> = (0..257u32).flat_map(|i| (((256 - i) * 32767 / 256) as u16).to_be_bytes()).collect();
        let file = [
            b"NCP\0".to_vec(),
            rec(0x100, b"0300"),
            rec(0x300, &[0x00, 0xc3]),
            rec(0x800, &[0x01, 4]), // contrast "not set" because the curve replaces it
            rec(0xa00, &[0x80, 4]),
            rec(2, &[vec![0u8; 64], curve].concat()),
            0u32.to_be_bytes().to_vec(),
        ]
        .concat();
        let parsed = parse_np3(&file).unwrap();
        assert_eq!(parsed.curve.as_ref().map(Vec::len), Some(257));
        assert_eq!(parsed.contrast, 0.0);
        assert!(parsed.saturation > 0.0, "Vivid base adds saturation");
        let lut = parsed.to_lut();
        assert!(lut.map([20, 20, 20])[0] > 200 && lut.map([235, 235, 235])[0] < 60, "curve applied");
    }

    #[test]
    fn three_dl_and_hald_tables_match_the_same_cube() {
        // The same look (swap red and blue, halve green) in both formats.
        let look = |r: f32, g: f32, b: f32| [b, g * 0.5, r];
        let n = 4usize;
        let mut text = String::from("# comment\n0 341 682 1023\n");
        for r in 0..n {
            for g in 0..n {
                for b in 0..n {
                    let v = look(r as f32 / 3.0, g as f32 / 3.0, b as f32 / 3.0).map(|v| (v * 1023.0).round() as u32);
                    text.push_str(&format!("{} {} {}\n", v[0], v[1], v[2]));
                }
            }
        }
        let from_3dl = parse_3dl(&text).unwrap();
        assert_eq!(from_3dl.map([255, 200, 0]), [0, 100, 255]);
        assert!(parse_3dl("1 2 3\n4 5 6\n").is_err());

        // Level 2 Hald: an 8x8 image holding a 4-point cube.
        let mut img = image::RgbImage::new(8, 8);
        for (i, px) in img.pixels_mut().enumerate() {
            let (r, g, b) = (i % 4, i / 4 % 4, i / 16);
            px.0 = look(r as f32 / 3.0, g as f32 / 3.0, b as f32 / 3.0).map(|v| (v * 255.0).round() as u8);
        }
        let mut png = Vec::new();
        img.write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png).unwrap();
        let from_hald = parse_hald(&png).unwrap();
        assert_eq!(from_hald.map([255, 200, 0]), [0, 100, 255]);
        for px in [[10, 20, 30], [200, 100, 50], [128, 128, 128]] {
            let (a, b) = (from_3dl.map(px), from_hald.map(px));
            assert!((0..3).all(|c| a[c].abs_diff(b[c]) <= 1), "{px:?}: {a:?} vs {b:?}");
        }
        // An ordinary photo is not mistaken for a table.
        let mut photo = Vec::new();
        image::RgbImage::new(64, 48).write_to(&mut std::io::Cursor::new(&mut photo), image::ImageFormat::Png).unwrap();
        assert!(parse_hald(&photo).is_err());
    }

    #[test]
    fn cube_files_are_applied_exactly() {
        // A 2-point table that swaps red and blue.
        let mut text = String::from("TITLE \"swap\"\nLUT_3D_SIZE 2\n");
        for b in 0..2 {
            for g in 0..2 {
                for r in 0..2 {
                    text.push_str(&format!("{b} {g} {r}\n"));
                }
            }
        }
        let lut = parse_cube(&text).unwrap();
        assert_eq!(lut.map([250, 100, 10]), [10, 100, 250]);
        let mut img = RgbImage::from_pixel(64, 48, image::Rgb([250, 100, 10]));
        lut.apply(&mut img);
        assert!(img.pixels().all(|p| p.0 == [10, 100, 250]));
        assert!(parse_cube("LUT_3D_SIZE 4\n0 0 0\n").is_err());
    }
}
