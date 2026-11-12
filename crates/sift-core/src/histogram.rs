//! Tonal distribution of an image, for the viewer's histogram.

use image::{RgbImage, Rgba, RgbaImage};
use serde::Serialize;

const BINS: usize = 256;
/// A channel counts as clipped at these values.
const CLIP_LOW: u8 = 2;
const CLIP_HIGH: u8 = 253;

#[derive(Serialize, Debug)]
pub struct Histogram {
    pub r: Vec<u32>,
    pub g: Vec<u32>,
    pub b: Vec<u32>,
    pub luma: Vec<u32>,
    /// Share (0..1) of pixels with a channel crushed to black / blown to white.
    pub shadows: f32,
    pub highlights: f32,
}

/// Counts every pixel inside `crop` (normalised x, y, width, height; `None`
/// for the whole image).
pub fn of(img: &RgbImage, crop: Option<[f32; 4]>) -> Histogram {
    let (w, h) = (img.width(), img.height());
    let [cx, cy, cw, ch] = crop.unwrap_or([0.0, 0.0, 1.0, 1.0]);
    let x0 = ((cx * w as f32).round() as u32).min(w.saturating_sub(1));
    let y0 = ((cy * h as f32).round() as u32).min(h.saturating_sub(1));
    let x1 = (x0 + (cw * w as f32).round().max(1.0) as u32).min(w);
    let y1 = (y0 + (ch * h as f32).round().max(1.0) as u32).min(h);

    let mut bins = [[0u32; BINS]; 4];
    let (mut low, mut high, mut total) = (0u32, 0u32, 0u32);
    for y in y0..y1 {
        for x in x0..x1 {
            let [r, g, b] = img.get_pixel(x, y).0;
            bins[0][r as usize] += 1;
            bins[1][g as usize] += 1;
            bins[2][b as usize] += 1;
            let luma = (0.2126 * r as f32 + 0.7152 * g as f32 + 0.0722 * b as f32).round() as usize;
            bins[3][luma.min(BINS - 1)] += 1;
            low += (r.max(g).max(b) <= CLIP_LOW) as u32;
            high += (r.min(g).min(b) >= CLIP_HIGH) as u32;
            total += 1;
        }
    }
    let share = |n: u32| if total == 0 { 0.0 } else { n as f32 / total as f32 };
    let [r, g, b, luma] = bins.map(|b| b.to_vec());
    Histogram { r, g, b, luma, shadows: share(low), highlights: share(high) }
}

/// Colours of the clipping warning: blown highlights and crushed shadows.
pub const CLIP_HIGH_COLOR: [u8; 4] = [255, 45, 45, 255];
pub const CLIP_LOW_COLOR: [u8; 4] = [40, 120, 255, 255];

/// A transparent image of the same size that marks the pixels the histogram
/// counts as clipped, to lay over the photo.
pub fn clipping_mask(img: &RgbImage) -> RgbaImage {
    RgbaImage::from_fn(img.width(), img.height(), |x, y| {
        let [r, g, b] = img.get_pixel(x, y).0;
        if r.min(g).min(b) >= CLIP_HIGH {
            Rgba(CLIP_HIGH_COLOR)
        } else if r.max(g).max(b) <= CLIP_LOW {
            Rgba(CLIP_LOW_COLOR)
        } else {
            Rgba([0; 4])
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_channels_clipping_and_crop() {
        // Left half pure white, right half a dark red.
        let img = RgbImage::from_fn(40, 10, |x, _| if x < 20 { image::Rgb([255, 255, 255]) } else { image::Rgb([200, 10, 0]) });
        let all = of(&img, None);
        assert_eq!(all.r[255], 200);
        assert_eq!(all.r[200], 200);
        assert_eq!(all.b[0], 200);
        assert_eq!(all.luma.iter().sum::<u32>(), 400);
        assert!((all.highlights - 0.5).abs() < 1e-6 && all.shadows == 0.0);

        let right = of(&img, Some([0.5, 0.0, 0.5, 1.0]));
        assert_eq!(right.r[200], 200);
        assert_eq!(right.r[255], 0);
        assert_eq!(right.highlights, 0.0);
    }

    #[test]
    fn the_mask_marks_what_the_histogram_counts() {
        let img = RgbImage::from_fn(3, 1, |x, _| image::Rgb([[255, 254, 253], [1, 2, 0], [255, 255, 200]][x as usize]));
        let mask = clipping_mask(&img);
        assert_eq!(mask.get_pixel(0, 0).0, CLIP_HIGH_COLOR);
        assert_eq!(mask.get_pixel(1, 0).0, CLIP_LOW_COLOR);
        assert_eq!(mask.get_pixel(2, 0).0, [0; 4], "one channel short of white is not clipped");
        let h = of(&img, None);
        assert!((h.highlights - 1.0 / 3.0).abs() < 1e-6 && (h.shadows - 1.0 / 3.0).abs() < 1e-6);
    }
}
