//! Shooting-data frames: renders the photo onto a canvas with camera, lens
//! and exposure text. Latin text uses the bundled Inter variable font (SIL
//! OFL); characters it lacks fall back to a CJK system font when one exists.

use crate::exif::Exif;
use ab_glyph::{point, Font, FontRef, FontVec, GlyphId, PxScale, ScaleFont, VariableFont};
use image::{imageops, Rgb, RgbImage};
use serde::Deserialize;

static INTER: &[u8] = include_bytes!("../assets/Inter.ttf");

const CJK_FONTS: &[&str] = &[
    "/System/Library/Fonts/PingFang.ttc",
    "/System/Library/Fonts/LanguageSupport/PingFang.ttc",
    "/System/Library/Fonts/Hiragino Sans GB.ttc",
    "C:\\Windows\\Fonts\\msyh.ttc",
    "C:\\Windows\\Fonts\\simhei.ttf",
    "/system/fonts/NotoSansCJK-Regular.ttc",
    "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc",
];

#[derive(Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Style {
    /// Text bar under the photo.
    Bar,
    /// Border on all sides with centred text below.
    Border,
    /// Photo inset on a blurred copy of itself.
    Glass,
}

#[derive(Deserialize, Clone, Debug)]
pub struct FrameOptions {
    pub style: Style,
    #[serde(default)]
    pub dark: bool,
    #[serde(default = "yes")]
    pub camera: bool,
    #[serde(default = "yes")]
    pub lens: bool,
    #[serde(default = "yes")]
    pub params: bool,
    #[serde(default = "yes")]
    pub date: bool,
    /// Free text shown instead of the date, such as a credit line.
    #[serde(default)]
    pub caption: String,
}

fn yes() -> bool {
    true
}

struct Fonts {
    regular: FontRef<'static>,
    bold: FontRef<'static>,
    cjk: Option<FontVec>,
}

impl Fonts {
    fn load() -> Fonts {
        let weight = |w: f32| {
            let mut f = FontRef::try_from_slice(INTER).expect("bundled font");
            f.set_variation(b"wght", w);
            f
        };
        let cjk = CJK_FONTS.iter().find_map(|p| FontVec::try_from_vec_and_index(std::fs::read(p).ok()?, 0).ok());
        Fonts { regular: weight(450.0), bold: weight(650.0), cjk }
    }
}

struct Line<'a> {
    text: &'a str,
    bold: bool,
    size: f32,
    color: [u8; 3],
}

/// Lays `text` out and calls `put(x, y, coverage)` for every covered pixel.
/// Returns the advance width.
fn layout(fonts: &Fonts, line: &Line, origin: (f32, f32), mut put: impl FnMut(i32, i32, f32)) -> f32 {
    let main = if line.bold { &fonts.bold } else { &fonts.regular };
    let scale = PxScale::from(line.size);
    let mut x = origin.0;
    let mut prev: Option<GlyphId> = None;
    for ch in line.text.chars() {
        let id = main.glyph_id(ch);
        let mut draw = |font: &dyn FontDyn, id: GlyphId, x: f32| font.draw(id, scale, (x, origin.1), &mut put);
        if id.0 != 0 {
            let sf = main.as_scaled(scale);
            if let Some(p) = prev {
                x += sf.kern(p, id);
            }
            draw(main, id, x);
            x += sf.h_advance(id);
            prev = Some(id);
        } else if let Some(cjk) = fonts.cjk.as_ref().filter(|f| f.glyph_id(ch).0 != 0) {
            let id = cjk.glyph_id(ch);
            draw(cjk, id, x);
            x += cjk.as_scaled(scale).h_advance(id);
            prev = None;
        } else {
            x += main.as_scaled(scale).h_advance(main.glyph_id(' '));
            prev = None;
        }
    }
    x - origin.0
}

/// Object-safe glyph rasterisation shared by the bundled and system fonts.
trait FontDyn {
    fn draw(&self, id: GlyphId, scale: PxScale, at: (f32, f32), put: &mut dyn FnMut(i32, i32, f32));
}

impl<F: Font> FontDyn for F {
    fn draw(&self, id: GlyphId, scale: PxScale, at: (f32, f32), put: &mut dyn FnMut(i32, i32, f32)) {
        let glyph = id.with_scale_and_position(scale, point(at.0, at.1));
        if let Some(outline) = self.outline_glyph(glyph) {
            let b = outline.px_bounds();
            outline.draw(|gx, gy, c| put(b.min.x as i32 + gx as i32, b.min.y as i32 + gy as i32, c));
        }
    }
}

fn blend(img: &mut RgbImage, x: i32, y: i32, color: [u8; 3], a: f32) {
    if x < 0 || y < 0 || x >= img.width() as i32 || y >= img.height() as i32 || a <= 0.0 {
        return;
    }
    let px = img.get_pixel_mut(x as u32, y as u32);
    let a = a.min(1.0);
    for c in 0..3 {
        px[c] = (px[c] as f32 * (1.0 - a) + color[c] as f32 * a).round() as u8;
    }
}

fn width(fonts: &Fonts, line: &Line) -> f32 {
    layout(fonts, line, (0.0, 0.0), |_, _, _| {})
}

/// Draws a line with its baseline at `y`; `align` is 0 left, 0.5 centre, 1 right of `x`.
fn draw(img: &mut RgbImage, fonts: &Fonts, line: &Line, x: f32, y: f32, align: f32) {
    if line.text.is_empty() {
        return;
    }
    let x = x - width(fonts, line) * align;
    let color = line.color;
    layout(fonts, line, (x, y), |px, py, c| blend(img, px, py, color, c));
}

fn camera_name(exif: &Exif) -> String {
    let model = exif.model.clone().unwrap_or_default();
    let make = exif.make.clone().unwrap_or_default();
    let brand = make.split_whitespace().next().unwrap_or("");
    if brand.is_empty() || model.to_ascii_uppercase().contains(&brand.to_ascii_uppercase()) {
        model
    } else {
        format!("{brand} {model}")
    }
}

pub fn params_text(exif: &Exif) -> String {
    let mut parts = Vec::new();
    if let Some(f) = exif.focal {
        parts.push(format!("{}mm", f.round()));
    }
    if let Some(a) = exif.aperture {
        parts.push(format!("f/{}", if a.fract() == 0.0 { format!("{a:.0}") } else { format!("{a:.1}") }));
    }
    if let Some(s) = exif.shutter {
        parts.push(if s >= 1.0 { format!("{s:.1}s").replace(".0s", "s") } else { format!("1/{}s", (1.0 / s).round()) });
    }
    if let Some(i) = exif.iso {
        parts.push(format!("ISO {i}"));
    }
    parts.join("  ")
}

/// Copies `photo` onto `canvas` at `(ox, oy)` with anti-aliased rounded corners.
fn paste_rounded(canvas: &mut RgbImage, photo: &RgbImage, ox: u32, oy: u32, radius: f32) {
    if radius < 1.0 {
        // Square corners: copy the pixels untouched.
        return imageops::replace(canvas, photo, ox as i64, oy as i64);
    }
    let (w, h) = (photo.width() as f32, photo.height() as f32);
    for (x, y, px) in photo.enumerate_pixels() {
        let (fx, fy) = (x as f32 + 0.5, y as f32 + 0.5);
        let dx = (radius - fx).max(fx - (w - radius)).max(0.0);
        let dy = (radius - fy).max(fy - (h - radius)).max(0.0);
        if dx == 0.0 || dy == 0.0 {
            // Outside the corner squares the photo is fully opaque.
            canvas.put_pixel(ox + x, oy + y, *px);
            continue;
        }
        let cover = (radius + 0.5 - (dx * dx + dy * dy).sqrt()).clamp(0.0, 1.0);
        blend(canvas, (ox + x) as i32, (oy + y) as i32, px.0, cover);
    }
}

/// Frames an upright photo.
pub fn render(photo: &RgbImage, exif: &Exif, opt: &FrameOptions) -> RgbImage {
    let fonts = Fonts::load();
    let (w, h) = photo.dimensions();
    let unit = w.max(h) as f32;
    let (bg, fg, dim) = match (opt.style, opt.dark) {
        (Style::Glass, _) => ([0u8; 3], [255u8; 3], [200u8; 3]),
        (_, true) => ([12u8; 3], [245u8; 3], [150u8; 3]),
        (_, false) => ([255u8; 3], [20u8; 3], [125u8; 3]),
    };
    let camera = if opt.camera { camera_name(exif) } else { String::new() };
    let lens = if opt.lens { exif.lens.clone().unwrap_or_default() } else { String::new() };
    let params = if opt.params { params_text(exif) } else { String::new() };
    let note = if !opt.caption.trim().is_empty() {
        opt.caption.trim().to_string()
    } else if opt.date {
        exif.datetime.clone().unwrap_or_default().chars().take(10).collect::<String>().replace('-', ".")
    } else {
        String::new()
    };

    if opt.style == Style::Bar {
        let bar = (unit * 0.085).round();
        let mut canvas = RgbImage::from_pixel(w, h + bar as u32, Rgb(bg));
        imageops::replace(&mut canvas, photo, 0, 0);
        let pad = bar * 0.42;
        let (big, small) = (bar * 0.27, bar * 0.2);
        let top = h as f32 + bar * 0.45;
        let bottom = h as f32 + bar * 0.76;
        let one = h as f32 + bar * 0.6;
        // A side with a single line is centred vertically.
        let rows = |a: &str, b: &str| if a.is_empty() || b.is_empty() { (one, one) } else { (top, bottom) };
        let (l1, l2) = rows(&camera, &lens);
        draw(&mut canvas, &fonts, &Line { text: &camera, bold: true, size: big, color: fg }, pad, l1, 0.0);
        draw(&mut canvas, &fonts, &Line { text: &lens, bold: false, size: small, color: dim }, pad, l2, 0.0);
        let (r1, r2) = rows(&params, &note);
        let right = w as f32 - pad;
        draw(&mut canvas, &fonts, &Line { text: &params, bold: true, size: big, color: fg }, right, r1, 1.0);
        draw(&mut canvas, &fonts, &Line { text: &note, bold: false, size: small, color: dim }, right, r2, 1.0);
        return canvas;
    }

    let margin = (unit * 0.035).round();
    let foot = (unit * 0.11).round();
    let (cw, ch) = (w + 2 * margin as u32, h + margin as u32 + foot as u32);
    let mut canvas = if opt.style == Style::Glass {
        // A tiny copy scaled back up is a cheap, wide blur.
        let tiny = imageops::thumbnail(photo, 24, 24 * ch / cw.max(1));
        let mut blurred = imageops::resize(&tiny, cw, ch, imageops::FilterType::CatmullRom);
        for p in blurred.pixels_mut() {
            for c in 0..3 {
                p[c] = (p[c] as f32 * 0.62) as u8;
            }
        }
        blurred
    } else {
        RgbImage::from_pixel(cw, ch, Rgb(bg))
    };
    let radius = if opt.style == Style::Glass { unit * 0.014 } else { 0.0 };
    paste_rounded(&mut canvas, photo, margin as u32, margin as u32, radius);

    let detail = [lens.as_str(), params.as_str(), note.as_str()]
        .into_iter()
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("   ·   ");
    let cx = cw as f32 / 2.0;
    let base = margin + h as f32;
    let (big, small) = (foot * 0.24, foot * 0.16);
    let (y1, y2) = if camera.is_empty() || detail.is_empty() {
        (base + foot * 0.58, base + foot * 0.58)
    } else {
        (base + foot * 0.48, base + foot * 0.74)
    };
    draw(&mut canvas, &fonts, &Line { text: &camera, bold: true, size: big, color: fg }, cx, y1, 0.5);
    draw(&mut canvas, &fonts, &Line { text: &detail, bold: false, size: small, color: dim }, cx, y2, 0.5);
    canvas
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The photo must arrive on the canvas unchanged in every style.
    #[test]
    fn photo_pixels_are_not_altered_by_the_frame() {
        let photo = RgbImage::from_fn(400, 300, |x, y| Rgb([(x % 256) as u8, (y % 256) as u8, 77]));
        let exif = Exif { model: Some("Test".into()), ..Exif::default() };
        for style in [Style::Bar, Style::Border, Style::Glass] {
            let opt = FrameOptions { style, dark: false, camera: true, lens: true, params: true, date: true, caption: String::new() };
            let out = render(&photo, &exif, &opt);
            let (ox, oy) = if style == Style::Bar { (0, 0) } else { ((out.width() - 400) / 2, (out.width() - 400) / 2) };
            for (x, y) in [(200, 150), (40, 40), (360, 260), (200, 8), (8, 150)] {
                assert_eq!(out.get_pixel(ox + x, oy + y), photo.get_pixel(x, y), "{style:?} at {x},{y}");
            }
        }
    }
}
