//! Watermarks on exported pictures: a picture (a logo, a signature) or text
//! with fields filled in from the photo, laid in a corner, at a chosen spot
//! or repeated over the whole picture, turned, in its own colour or one that
//! stands out from what is under it, with a soft shadow if wanted.
//!
//! Sizes are shares of the picture's shorter side, so a mark comes out alike
//! on upright and level pictures and at any export size.

use crate::exif::Exif;
use crate::frame::{self, Fonts, Line};
use ab_glyph::{Font, PxScale, ScaleFont};
use image::{imageops, RgbImage, Rgba, RgbaImage};
use serde::Deserialize;

/// Where a watermark goes: a corner, the middle of a side, or the centre.
#[derive(Deserialize, Clone, Copy, Debug, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum Place {
    Nw,
    N,
    Ne,
    W,
    C,
    E,
    Sw,
    S,
    #[default]
    Se,
}

impl Place {
    /// Column and row in a three by three grid.
    fn cell(self) -> (u8, u8) {
        let i = self as u8;
        (i % 3, i / 3)
    }
}

/// The colour a watermark is drawn in: its own (white, for text), white or
/// black, whichever stands out from the picture under it, or one colour
/// (`"#rrggbb"`).
#[derive(Deserialize, Clone, Copy, Debug, PartialEq, Eq, Default)]
#[serde(try_from = "String")]
pub enum Tint {
    #[default]
    Own,
    Auto,
    Rgb([u8; 3]),
}

impl TryFrom<String> for Tint {
    type Error = String;
    fn try_from(s: String) -> Result<Tint, String> {
        match s.as_str() {
            "own" => Ok(Tint::Own),
            "auto" => Ok(Tint::Auto),
            _ => {
                let v = s.strip_prefix('#').filter(|h| h.len() == 6).and_then(|h| u32::from_str_radix(h, 16).ok()).ok_or(format!("not a colour: {s}"))?;
                Ok(Tint::Rgb([(v >> 16) as u8, (v >> 8) as u8, v as u8]))
            }
        }
    }
}

/// How a watermark is laid on a picture.
#[derive(Deserialize, Clone, Debug, PartialEq)]
#[serde(default)]
pub struct Layout {
    pub place: Place,
    /// Where its centre goes instead of `place`, as a share of the
    /// picture's width and height; it is kept inside the picture.
    pub at: Option<[f32; 2]>,
    /// Room between it and the edges.
    pub margin: f32,
    /// A picture's width; the size of the letters of text.
    pub size: f32,
    pub opacity: f32,
    /// Degrees, clockwise.
    pub angle: f32,
    /// Repeated over the whole picture, `gap` apart, every other row
    /// shifted by half a step.
    pub tile: bool,
    pub gap: f32,
    pub tint: Tint,
    pub shadow: bool,
}

impl Default for Layout {
    fn default() -> Layout {
        Layout { place: Place::Se, at: None, margin: 0.03, size: 0.2, opacity: 0.8, angle: 0.0, tile: false, gap: 0.12, tint: Tint::Own, shadow: false }
    }
}

enum What {
    /// Premultiplied.
    Picture(RgbaImage),
    Text { template: String, fonts: Box<Fonts> },
}

/// A watermark ready to be laid on any number of pictures.
pub struct Watermark {
    what: What,
    layout: Layout,
}

/// Fields text can carry, filled in from each photo.
const FIELDS: [&str; 8] = ["{year}", "{date}", "{artist}", "{copyright}", "{camera}", "{lens}", "{params}", "{file}"];

impl Watermark {
    pub fn picture(img: &RgbaImage, layout: Layout) -> Watermark {
        Watermark { what: What::Picture(premultiplied(img)), layout }
    }

    /// Text in Inter at `weight` (100 to 900); `None` when there is none.
    pub fn text(template: &str, weight: f32, layout: Layout) -> Option<Watermark> {
        let template = template.trim();
        (!template.is_empty()).then(|| Watermark { what: What::Text { template: template.to_string(), fonts: Box::new(Fonts::weighing(weight, 700.0)) }, layout })
    }

    /// The text has a field that is filled in from what the camera recorded.
    pub fn needs_exif(&self) -> bool {
        matches!(&self.what, What::Text { template, .. } if FIELDS.iter().any(|f| *f != "{file}" && template.contains(f)))
    }

    /// Lays the watermark on `img`, the photo described by `exif` and named
    /// `file` (without its extension).
    pub fn apply(&self, img: &mut RgbImage, exif: &Exif, file: &str) {
        let l = &self.layout;
        let (iw, ih) = img.dimensions();
        let short = iw.min(ih) as f32;
        // Lines of text line up on the side of the picture they are near.
        let align = match (l.tile, l.at) {
            (false, None) => l.place.cell().0 as f32 / 2.0,
            _ => 0.5,
        };
        let (stamp, unit) = match &self.what {
            What::Picture(p) => {
                let w = (short * l.size.clamp(0.01, 1.0)).round().max(1.0) as u32;
                let h = ((w as f32 * p.height() as f32 / p.width().max(1) as f32).round() as u32).max(1);
                (resized(p, w, h), w.min(h) as f32)
            }
            What::Text { template, fonts } => {
                let text = filled(template, exif, file);
                if text.trim().is_empty() {
                    return;
                }
                let px = short * l.size.clamp(0.002, 0.5);
                (text_stamp(fonts, &text, px, align), px)
            }
        };
        let mut stamp = rotated(&stamp, l.angle);
        if !l.tile && (stamp.width() > iw || stamp.height() > ih) {
            let k = (iw as f32 / stamp.width() as f32).min(ih as f32 / stamp.height() as f32);
            let (w, h) = (((stamp.width() as f32 * k) as u32).max(1), ((stamp.height() as f32 * k) as u32).max(1));
            stamp = resized(&stamp, w, h);
        }
        let spots = if l.tile { tiles(iw, ih, &stamp, short * l.gap.max(0.0)) } else { vec![single(iw, ih, &stamp, l, short)] };
        let fills: Vec<Option<[u8; 3]>> = spots
            .iter()
            .map(|&(x, y)| match l.tint {
                Tint::Own => None,
                Tint::Rgb(c) => Some(c),
                Tint::Auto => Some(if luminance_under(img, &stamp, x, y) > 0.62 { [24, 24, 24] } else { [255, 255, 255] }),
            })
            .collect();
        let opacity = l.opacity.clamp(0.0, 1.0);
        let shadow = l.shadow.then(|| Shadow::of(&stamp, unit));
        let own = mean_colour(&stamp);
        for (&(x, y), fill) in spots.iter().zip(&fills) {
            if let Some(s) = &shadow {
                // Dark under a light mark, light under a dark one.
                let color = if luminance(fill.unwrap_or(own)) > 0.5 { [0; 3] } else { [255; 3] };
                s.put(img, x, y, color, opacity * 0.7);
            }
            put(img, &stamp, x, y, *fill, opacity);
        }
    }
}

/// `template` with its fields filled in for one photo: `{year}` (the year
/// it was taken, this year when that is not known), `{date}`, `{artist}`
/// and `{copyright}` as set in the camera, `{camera}`, `{lens}`, `{params}`
/// (focal length, aperture, shutter, ISO) and `{file}`. Space left by a
/// field without a value is closed up.
pub fn filled(template: &str, exif: &Exif, file: &str) -> String {
    let date = exif.datetime.as_deref().unwrap_or("");
    let year = date.get(..4).filter(|y| y.bytes().all(|b| b.is_ascii_digit())).map_or_else(this_year, str::to_string);
    let text = template
        .replace("{year}", &year)
        .replace("{date}", &date.get(..10).map(|d| d.replace('-', ".")).unwrap_or_default())
        .replace("{artist}", exif.artist.as_deref().unwrap_or(""))
        .replace("{copyright}", exif.copyright.as_deref().unwrap_or(""))
        .replace("{camera}", &frame::camera_name(exif))
        .replace("{lens}", exif.lens.as_deref().unwrap_or(""))
        .replace("{file}", file);
    // Shooting data keeps the wider spacing it is written with.
    let params = frame::params_text(exif);
    text.lines()
        .map(|line| line.split_whitespace().collect::<Vec<_>>().join(" ").replace("{params}", &params).trim().to_string())
        .collect::<Vec<_>>()
        .join("\n")
        .trim_matches('\n')
        .to_string()
}

fn this_year() -> String {
    let days = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs() / 86_400) as i64;
    // Howard Hinnant's civil_from_days.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    (yoe + era * 400 + i64::from(month <= 2)).to_string()
}

/// White text on a transparent ground (premultiplied), its lines `px` high
/// and aligned by `align` (0 left, 0.5 centred, 1 right), cut to the ink so
/// that it keeps the same margin as a picture.
fn text_stamp(fonts: &Fonts, text: &str, px: f32, align: f32) -> RgbaImage {
    let lines: Vec<&str> = text.lines().collect();
    let line = |text| Line { text, bold: false, size: px, color: [255; 3] };
    let widths: Vec<f32> = lines.iter().map(|t| frame::width(fonts, &line(t))).collect();
    let sf = fonts.regular.as_scaled(PxScale::from(px));
    let (ascent, descent) = (sf.ascent(), sf.descent());
    let step = ascent - descent + sf.line_gap();
    let pad = (px * 0.15).ceil() + 1.0;
    let widest = widths.iter().copied().fold(0.0, f32::max);
    let (w, h) = ((widest + 2.0 * pad).ceil() as u32, (step * (lines.len() as f32 - 1.0) + ascent - descent + 2.0 * pad).ceil() as u32);
    let mut cover = vec![0f32; (w * h) as usize];
    for (i, (text, width)) in lines.iter().zip(&widths).enumerate() {
        let origin = (pad + (widest - width) * align, pad + ascent + step * i as f32);
        frame::layout(fonts, &line(text), origin, |x, y, c| {
            if (0..w as i32).contains(&x) && (0..h as i32).contains(&y) {
                let at = &mut cover[(y as u32 * w + x as u32) as usize];
                *at = (*at + c).min(1.0);
            }
        });
    }
    let inked = |i: usize| cover[i] * 255.0 >= 0.5;
    let (mut x0, mut y0, mut x1, mut y1) = (w, h, 0, 0);
    for (i, _) in cover.iter().enumerate().filter(|&(i, _)| inked(i)) {
        let (x, y) = (i as u32 % w, i as u32 / w);
        (x0, y0, x1, y1) = (x0.min(x), y0.min(y), x1.max(x), y1.max(y));
    }
    if x0 > x1 {
        return RgbaImage::new(1, 1);
    }
    RgbaImage::from_fn(x1 - x0 + 1, y1 - y0 + 1, |x, y| {
        let a = (cover[((y + y0) * w + x + x0) as usize] * 255.0).round() as u8;
        Rgba([a; 4])
    })
}

fn premultiplied(img: &RgbaImage) -> RgbaImage {
    let mut out = img.clone();
    for p in out.pixels_mut() {
        let a = p[3] as u32;
        for k in 0..3 {
            p[k] = ((p[k] as u32 * a + 127) / 255) as u8;
        }
    }
    out
}

/// A premultiplied picture resized (Lanczos), kept premultiplied.
fn resized(img: &RgbaImage, w: u32, h: u32) -> RgbaImage {
    if img.dimensions() == (w, h) {
        return img.clone();
    }
    let mut out = imageops::resize(img, w, h, imageops::FilterType::Lanczos3);
    // Ringing must not leave more colour than cover.
    for p in out.pixels_mut() {
        for k in 0..3 {
            p[k] = p[k].min(p[3]);
        }
    }
    out
}

/// A premultiplied picture turned clockwise by `angle` degrees about its
/// centre, on a canvas just large enough for it.
fn rotated(img: &RgbaImage, angle: f32) -> RgbaImage {
    if angle.abs() < 0.05 {
        return img.clone();
    }
    let (sin, cos) = (angle as f64).to_radians().sin_cos();
    let (w, h) = (img.width() as f64, img.height() as f64);
    let (nw, nh) = ((w * cos.abs() + h * sin.abs()).ceil().max(1.0), (w * sin.abs() + h * cos.abs()).ceil().max(1.0));
    let at = |x: i64, y: i64| -> [f64; 4] {
        if x < 0 || y < 0 || x >= img.width() as i64 || y >= img.height() as i64 {
            return [0.0; 4];
        }
        img.get_pixel(x as u32, y as u32).0.map(f64::from)
    };
    RgbaImage::from_fn(nw as u32, nh as u32, |x, y| {
        let (dx, dy) = (x as f64 + 0.5 - nw / 2.0, y as f64 + 0.5 - nh / 2.0);
        // The point of the upright mark that lands here.
        let (sx, sy) = (dx * cos + dy * sin + w / 2.0 - 0.5, -dx * sin + dy * cos + h / 2.0 - 0.5);
        let (x0, y0) = (sx.floor(), sy.floor());
        let (fx, fy) = (sx - x0, sy - y0);
        let (x0, y0) = (x0 as i64, y0 as i64);
        let (a, b, c, d) = (at(x0, y0), at(x0 + 1, y0), at(x0, y0 + 1), at(x0 + 1, y0 + 1));
        Rgba(std::array::from_fn(|k| ((a[k] * (1.0 - fx) + b[k] * fx) * (1.0 - fy) + (c[k] * (1.0 - fx) + d[k] * fx) * fy).round().clamp(0.0, 255.0) as u8))
    })
}

/// Top left corner of a single mark.
fn single(iw: u32, ih: u32, stamp: &RgbaImage, l: &Layout, short: f32) -> (i64, i64) {
    let (bw, bh) = (stamp.width() as f32, stamp.height() as f32);
    let (iw, ih) = (iw as f32, ih as f32);
    let (x, y) = match l.at {
        Some([cx, cy]) => ((cx * iw - bw / 2.0).clamp(0.0, (iw - bw).max(0.0)), (cy * ih - bh / 2.0).clamp(0.0, (ih - bh).max(0.0))),
        None => {
            let m = short * l.margin.clamp(0.0, 0.45);
            let along = |room: f32, len: f32, at: u8| {
                let free = (room - len).max(0.0);
                match at {
                    0 => m.min(free),
                    1 => free / 2.0,
                    _ => free - m.min(free),
                }
            };
            let (col, row) = l.place.cell();
            (along(iw, bw, col), along(ih, bh, row))
        }
    };
    (x.round() as i64, y.round() as i64)
}

/// Top left corners of marks repeated over the picture, one in its centre.
fn tiles(iw: u32, ih: u32, stamp: &RgbaImage, gap: f32) -> Vec<(i64, i64)> {
    let (bw, bh) = (stamp.width() as f32, stamp.height() as f32);
    let (sx, sy) = ((bw + gap).max(1.0), (bh + gap).max(1.0));
    let (ox, oy) = ((iw as f32 - bw) / 2.0, (ih as f32 - bh) / 2.0);
    let rows = ((oy + bh) / sy).ceil() as i64 + 1;
    let cols = ((ox + bw) / sx).ceil() as i64 + 2;
    let mut out = Vec::new();
    for r in -rows..=rows {
        let y = oy + r as f32 * sy;
        if y >= ih as f32 || y + bh <= 0.0 {
            continue;
        }
        let shift = if r.rem_euclid(2) == 1 { sx / 2.0 } else { 0.0 };
        for c in -cols..=cols {
            let x = ox + shift + c as f32 * sx;
            if x < iw as f32 && x + bw > 0.0 {
                out.push((x.round() as i64, y.round() as i64));
            }
        }
    }
    out
}

fn luminance(c: [u8; 3]) -> f32 {
    (0.2126 * c[0] as f32 + 0.7152 * c[1] as f32 + 0.0722 * c[2] as f32) / 255.0
}

/// Mean luminance of the picture where the mark would cover it.
fn luminance_under(img: &RgbImage, stamp: &RgbaImage, x0: i64, y0: i64) -> f32 {
    let step = (stamp.width().max(stamp.height()) / 64).max(1) as usize;
    let (mut sum, mut weight) = (0.0, 0.0);
    for y in (0..stamp.height()).step_by(step) {
        for x in (0..stamp.width()).step_by(step) {
            let (px, py) = (x0 + x as i64, y0 + y as i64);
            if px < 0 || py < 0 || px >= img.width() as i64 || py >= img.height() as i64 {
                continue;
            }
            let a = stamp.get_pixel(x, y)[3] as f32;
            sum += luminance(img.get_pixel(px as u32, py as u32).0) * a;
            weight += a;
        }
    }
    if weight > 0.0 { sum / weight } else { 0.0 }
}

/// The mark's own colour on average.
fn mean_colour(stamp: &RgbaImage) -> [u8; 3] {
    let mut sum = [0f64; 4];
    for p in stamp.pixels() {
        for k in 0..4 {
            sum[k] += p[k] as f64;
        }
    }
    if sum[3] <= 0.0 {
        return [255; 3];
    }
    std::array::from_fn(|k| (sum[k] / sum[3] * 255.0).round().clamp(0.0, 255.0) as u8)
}

/// Lays a premultiplied mark with its top left corner at `(x0, y0)`, in
/// `fill` instead of its own colours when given.
fn put(img: &mut RgbImage, stamp: &RgbaImage, x0: i64, y0: i64, fill: Option<[u8; 3]>, opacity: f32) {
    let (iw, ih) = (img.width() as i64, img.height() as i64);
    for (x, y, m) in stamp.enumerate_pixels() {
        let (px, py) = (x0 + x as i64, y0 + y as i64);
        if m[3] == 0 || px < 0 || py < 0 || px >= iw || py >= ih {
            continue;
        }
        let a = m[3] as f32 / 255.0 * opacity;
        let dst = img.get_pixel_mut(px as u32, py as u32);
        for k in 0..3 {
            let src = fill.map_or(m[k] as f32, |f| f[k] as f32 * m[3] as f32 / 255.0) * opacity;
            dst[k] = (dst[k] as f32 * (1.0 - a) + src).round().clamp(0.0, 255.0) as u8;
        }
    }
}

/// A soft shadow of a mark: its cover blurred and moved down a little.
struct Shadow {
    cover: Vec<f32>,
    w: u32,
    h: u32,
    /// How far the shadow reaches out of the mark's box on each side.
    pad: i64,
    drop: i64,
}

impl Shadow {
    fn of(stamp: &RgbaImage, unit: f32) -> Shadow {
        let radius = (unit * 0.08).max(1.0);
        let pad = (radius * 2.5).ceil() as i64;
        let (w, h) = (stamp.width() + 2 * pad as u32, stamp.height() + 2 * pad as u32);
        let mut cover = vec![0f32; (w * h) as usize];
        for (x, y, p) in stamp.enumerate_pixels() {
            cover[((y + pad as u32) * w + x + pad as u32) as usize] = p[3] as f32 / 255.0;
        }
        // Three box blurs come close to a Gaussian.
        let r = (radius / 1.7).ceil().max(1.0) as usize;
        for _ in 0..3 {
            box_blur(&mut cover, w as usize, h as usize, r);
        }
        Shadow { cover, w, h, pad, drop: (radius * 0.4).round() as i64 }
    }

    fn put(&self, img: &mut RgbImage, x0: i64, y0: i64, color: [u8; 3], strength: f32) {
        let (iw, ih) = (img.width() as i64, img.height() as i64);
        let (ox, oy) = (x0 - self.pad, y0 - self.pad + self.drop);
        for y in 0..self.h as i64 {
            let py = oy + y;
            if py < 0 || py >= ih {
                continue;
            }
            for x in 0..self.w as i64 {
                let px = ox + x;
                let a = (self.cover[(y * self.w as i64 + x) as usize] * 1.4).min(1.0) * strength;
                if px < 0 || px >= iw || a <= 0.002 {
                    continue;
                }
                let dst = img.get_pixel_mut(px as u32, py as u32);
                for k in 0..3 {
                    dst[k] = (dst[k] as f32 * (1.0 - a) + color[k] as f32 * a).round() as u8;
                }
            }
        }
    }
}

fn box_blur(v: &mut [f32], w: usize, h: usize, r: usize) {
    let mut line = Vec::new();
    let mut pass = |v: &mut [f32], len: usize, count: usize, at: &dyn Fn(usize, usize) -> usize| {
        for n in 0..count {
            line.clear();
            line.extend((0..len).map(|i| v[at(n, i)]));
            let mut sum: f32 = line.iter().take(r + 1).sum();
            for i in 0..len {
                v[at(n, i)] = sum / (2 * r + 1) as f32;
                if i + r + 1 < len {
                    sum += line[i + r + 1];
                }
                if i >= r {
                    sum -= line[i - r];
                }
            }
        }
    };
    pass(v, w, h, &|row, i| row * w + i);
    pass(v, h, w, &|col, i| i * w + col);
}

/// The colour all four corners of a picture share, when it has no
/// transparency of its own: the plain ground a logo was saved on.
pub fn backdrop(img: &RgbaImage) -> Option<[u8; 3]> {
    let (w, h) = img.dimensions();
    if w < 2 || h < 2 || img.pixels().any(|p| p[3] < 255) {
        return None;
    }
    let corners = [(0, 0), (w - 1, 0), (0, h - 1), (w - 1, h - 1)].map(|(x, y)| img.get_pixel(x, y).0);
    let mean: [u8; 3] = std::array::from_fn(|k| (corners.iter().map(|c| c[k] as u32).sum::<u32>() / 4) as u8);
    corners.iter().all(|c| (0..3).all(|k| c[k].abs_diff(mean[k]) <= 12)).then_some(mean)
}

/// A picture with its plain ground (see [backdrop]) made transparent: how
/// much each pixel differs from the ground is how opaque it is, and the
/// ground is taken out of the colour of its soft edges.
pub fn knocked_out(mut img: RgbaImage) -> RgbaImage {
    let Some(ground) = backdrop(&img) else { return img };
    let g = ground.map(|v| v as f32 / 255.0);
    for p in img.pixels_mut() {
        let c = [p[0], p[1], p[2]].map(|v| v as f32 / 255.0);
        let reach = (0..3)
            .map(|k| match c[k] - g[k] {
                d if d > 0.0 => d / (1.0 - g[k]).max(1e-4),
                d if d < 0.0 => -d / g[k].max(1e-4),
                _ => 0.0,
            })
            .fold(0.0f32, f32::max)
            .min(1.0);
        // What JPEG noise leaves on the ground goes too.
        let alpha = ((reach - 0.06) / 0.94).clamp(0.0, 1.0);
        for k in 0..3 {
            let own = if reach > 0.0 { (c[k] - g[k]) / reach + g[k] } else { c[k] };
            p[k] = (own.clamp(0.0, 1.0) * 255.0).round() as u8;
        }
        p[3] = (alpha * 255.0).round() as u8;
    }
    img
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::Rgb;

    fn lit(img: &RgbImage, ground: Rgb<u8>) -> Vec<(u32, u32)> {
        img.enumerate_pixels().filter(|p| *p.2 != ground).map(|p| (p.0, p.1)).collect()
    }

    fn bounds(points: &[(u32, u32)]) -> (u32, u32, u32, u32) {
        let xs = points.iter().map(|p| p.0);
        let ys = points.iter().map(|p| p.1);
        (xs.clone().min().unwrap(), ys.clone().min().unwrap(), xs.max().unwrap(), ys.max().unwrap())
    }

    fn layout(patch: impl FnOnce(&mut Layout)) -> Layout {
        let mut l = Layout::default();
        patch(&mut l);
        l
    }

    #[test]
    fn a_picture_goes_in_its_corner_and_lets_the_photo_through() {
        let mut img = RgbImage::from_pixel(400, 200, Rgb([0, 0, 0]));
        let mark = RgbaImage::from_pixel(10, 5, Rgba([255, 255, 255, 255]));
        Watermark::picture(&mark, layout(|l| (l.size, l.opacity) = (0.25, 0.5))).apply(&mut img, &Exif::default(), "");
        // 50 px wide (a quarter of the shorter side), 6 px in from the edges.
        let (x0, y0, x1, y1) = bounds(&lit(&img, Rgb([0, 0, 0])));
        assert_eq!((x1 + 1 + 6, y1 + 1 + 6), (400, 200));
        assert_eq!((x1 - x0 + 1, y1 - y0 + 1), (50, 25));
        assert!((120..=135).contains(&img.get_pixel(x0 + 25, y0 + 12).0[0]), "half opacity");
    }

    #[test]
    fn a_mark_can_go_anywhere_and_stays_inside() {
        let mark = RgbaImage::from_pixel(10, 10, Rgba([255, 255, 255, 255]));
        let mut img = RgbImage::new(400, 200);
        Watermark::picture(&mark, layout(|l| (l.size, l.at) = (0.1, Some([0.25, 0.5])))).apply(&mut img, &Exif::default(), "");
        assert_eq!(bounds(&lit(&img, Rgb([0, 0, 0]))), (90, 90, 109, 109));
        let mut img = RgbImage::new(400, 200);
        Watermark::picture(&mark, layout(|l| (l.size, l.at) = (0.1, Some([1.0, 0.0])))).apply(&mut img, &Exif::default(), "");
        assert_eq!(bounds(&lit(&img, Rgb([0, 0, 0]))), (380, 0, 399, 19));
    }

    #[test]
    fn a_turned_mark_stands_upright() {
        let mark = RgbaImage::from_pixel(40, 10, Rgba([255, 255, 255, 255]));
        let mut img = RgbImage::new(400, 400);
        Watermark::picture(&mark, layout(|l| (l.size, l.place, l.angle) = (0.4, Place::C, 90.0))).apply(&mut img, &Exif::default(), "");
        let (x0, y0, x1, y1) = bounds(&lit(&img, Rgb([0, 0, 0])));
        assert!((y1 - y0) > 150 && (x1 - x0) < 50, "{x0} {y0} {x1} {y1}");
    }

    #[test]
    fn tiles_cover_the_whole_picture_evenly() {
        let mark = RgbaImage::from_pixel(10, 10, Rgba([255, 255, 255, 255]));
        let mut img = RgbImage::new(600, 400);
        Watermark::picture(&mark, layout(|l| (l.size, l.tile, l.gap) = (0.1, true, 0.1))).apply(&mut img, &Exif::default(), "");
        // One in the middle of the picture, and some in every quarter.
        assert_ne!(img.get_pixel(300, 200).0, [0, 0, 0]);
        for (x, y) in [(0, 0), (300, 0), (0, 200), (300, 200)] {
            let quarter = (x..x + 300).flat_map(|x| (y..y + 200).map(move |y| (x, y))).filter(|&(x, y)| img.get_pixel(x, y).0 != [0, 0, 0]).count();
            assert!(quarter > 4000, "quarter at {x},{y}: {quarter}");
        }
    }

    #[test]
    fn text_is_filled_in_from_the_photo() {
        let exif = Exif { datetime: Some("2024-05-06 07:08:09".into()), artist: Some("Ann Lee".into()), model: Some("Z 8".into()), make: Some("NIKON CORPORATION".into()), ..Exif::default() };
        assert_eq!(filled("© {year} {artist}", &exif, "DSC_1"), "© 2024 Ann Lee");
        assert_eq!(filled("{camera} · {date}\n{file}", &exif, "DSC_1"), "NIKON Z 8 · 2024.05.06\nDSC_1");
        // A field without a value leaves no gap; without a date the year is this one.
        let year = filled("{year}", &Exif::default(), "");
        assert!(year.len() == 4 && year.as_str() >= "2025", "{year}");
        assert_eq!(filled("©  {copyright} {year}", &Exif::default(), ""), format!("© {year}"));
    }

    #[test]
    fn text_comes_out_as_tall_as_asked_in_its_corner() {
        let mut img = RgbImage::new(800, 400);
        let mark = Watermark::text("HELLO", 450.0, layout(|l| (l.size, l.margin) = (0.1, 0.05))).unwrap();
        assert!(!mark.needs_exif());
        mark.apply(&mut img, &Exif::default(), "");
        let (x0, y0, x1, y1) = bounds(&lit(&img, Rgb([0, 0, 0])));
        // Capitals of 40 px letters are about 29 px tall, and the ink ends 20 px in from the edges.
        assert!((25..=33).contains(&(y1 - y0 + 1)), "{}", y1 - y0 + 1);
        assert_eq!((x1 + 1 + 20, y1 + 1 + 20), (800, 400));
        assert!(x0 > 600);
        assert_eq!(img.get_pixel((x0 + x1) / 2, y0).0[0], img.get_pixel((x0 + x1) / 2, y0).0[1], "white");
    }

    #[test]
    fn automatic_colour_stands_out_from_the_picture() {
        for (ground, expect_dark) in [([250u8; 3], true), ([10u8; 3], false)] {
            let mut img = RgbImage::from_pixel(400, 200, Rgb(ground));
            let mark = Watermark::text("WWWW", 700.0, layout(|l| (l.size, l.tint, l.opacity) = (0.2, Tint::Auto, 1.0))).unwrap();
            mark.apply(&mut img, &Exif::default(), "");
            let darkest = img.pixels().map(|p| p.0[0]).min().unwrap();
            let lightest = img.pixels().map(|p| p.0[0]).max().unwrap();
            if expect_dark {
                assert!(darkest < 40, "{darkest}");
            } else {
                assert!(lightest > 240, "{lightest}");
            }
        }
    }

    #[test]
    fn a_shadow_darkens_around_a_light_mark() {
        let mark = RgbaImage::from_pixel(20, 20, Rgba([255, 255, 255, 255]));
        let mut img = RgbImage::from_pixel(400, 400, Rgb([128, 128, 128]));
        Watermark::picture(&mark, layout(|l| (l.size, l.place, l.shadow, l.opacity) = (0.1, Place::C, true, 1.0))).apply(&mut img, &Exif::default(), "");
        // The mark covers 180..220; just below it the shadow shows.
        assert_eq!(img.get_pixel(200, 200).0, [255; 3]);
        assert!(img.get_pixel(200, 222).0[0] < 110, "{:?}", img.get_pixel(200, 222));
        assert_eq!(img.get_pixel(20, 20).0, [128; 3]);
    }

    #[test]
    fn a_colour_replaces_the_marks_own() {
        let mark = RgbaImage::from_pixel(10, 10, Rgba([0, 0, 0, 255]));
        let mut img = RgbImage::new(200, 200);
        Watermark::picture(&mark, layout(|l| (l.size, l.place, l.tint, l.opacity) = (0.2, Place::C, Tint::Rgb([255, 0, 0]), 1.0))).apply(&mut img, &Exif::default(), "");
        assert_eq!(img.get_pixel(100, 100).0, [255, 0, 0]);
        assert_eq!(Tint::try_from("#00ff80".to_string()), Ok(Tint::Rgb([0, 255, 128])));
        assert!(Tint::try_from("teal".to_string()).is_err());
    }

    #[test]
    fn a_logo_on_a_plain_ground_loses_its_ground() {
        // Black letters with a grey edge on white, as a JPEG logo comes.
        let img = RgbaImage::from_fn(40, 20, |x, _| match x {
            10..=19 => Rgba([0, 0, 0, 255]),
            20 => Rgba([128, 128, 128, 255]),
            _ => Rgba([255, 254, 255, 255]),
        });
        assert_eq!(backdrop(&img), Some([255, 254, 255]));
        let out = knocked_out(img);
        assert_eq!(out.get_pixel(0, 0)[3], 0);
        assert_eq!(out.get_pixel(15, 5).0, [0, 0, 0, 255]);
        let edge = out.get_pixel(20, 5);
        assert!((110..=130).contains(&edge[3]) && edge[0] < 10, "{edge:?}");
        // A picture that is transparent already, or has no plain ground, stays as it is.
        let clear = RgbaImage::from_pixel(4, 4, Rgba([255, 255, 255, 0]));
        assert_eq!(backdrop(&clear), None);
        let busy = RgbaImage::from_fn(4, 4, |x, y| Rgba([(x * 80) as u8, (y * 80) as u8, 0, 255]));
        assert_eq!(knocked_out(busy.clone()), busy);
    }
}
