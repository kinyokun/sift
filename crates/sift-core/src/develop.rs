//! Full-resolution RAW development for cameras whose embedded preview is too
//! small to judge focus (older Sony bodies embed only 1616x1080).
//!
//! Sensor decoding, demosaicing and colour conversion come from `rawler`
//! (the dnglab decoder). The result is then tone-matched to the camera's own
//! embedded JPEG by per-channel histogram specification, so the developed
//! image keeps the in-camera look instead of a flat linear rendering.

use crate::preview;
use crate::{Error, Result};
use image::RgbImage;
use std::path::Path;

/// Histogram resolution for the 16-bit developed image.
const BINS: usize = 4096;
const REFERENCE_EDGE: u32 = 512;

fn cdf(hist: &[u64]) -> Vec<f64> {
    let total = hist.iter().sum::<u64>().max(1) as f64;
    let mut acc = 0u64;
    hist.iter()
        .map(|&h| {
            acc += h;
            acc as f64 / total
        })
        .collect()
}

/// For each source bin, the 8-bit level whose cumulative share in the
/// reference matches the source's.
fn match_lut(src_hist: &[u64], ref_hist: &[u64; 256]) -> Vec<f32> {
    let (src, reference) = (cdf(src_hist), cdf(ref_hist));
    let mut level = 0usize;
    src.iter()
        .map(|&p| {
            while level < 255 && reference[level] < p {
                level += 1;
            }
            // interpolate inside the reference step for a smooth curve
            let lo = if level == 0 { 0.0 } else { reference[level - 1] };
            let span = (reference[level] - lo).max(1e-9);
            (level as f64 - 1.0 + ((p - lo) / span).clamp(0.0, 1.0)).max(0.0) as f32
        })
        .collect()
}

/// Decodes and demosaics `path` to 16-bit sRGB at full sensor resolution.
fn linear(path: &Path) -> Result<(u32, u32, Vec<u16>)> {
    let err = |e: rawler::RawlerError| Error::Decode(e.to_string());
    let raw = rawler::decode_file(path).map_err(err)?;
    let developed = rawler::imgop::develop::RawDevelop::default()
        .develop_intermediate(&raw)
        .map_err(err)?
        .to_dynamic_image()
        .ok_or(Error::Malformed)?
        .into_rgb16();
    let (w, h) = developed.dimensions();
    Ok((w, h, developed.into_raw()))
}

fn bin(v: u16) -> usize {
    v as usize * BINS / 65536
}

/// Maps 16-bit pixels to 8 bits through one lookup table per channel.
fn apply(data: &[u16], luts: &[Vec<f32>]) -> Vec<u8> {
    let mut out = Vec::with_capacity(data.len());
    for px in data.as_chunks::<3>().0 {
        for c in 0..3 {
            let pos = px[c] as f32 * (BINS - 1) as f32 / 65535.0;
            let (i, frac) = (pos as usize, pos.fract());
            let (a, b) = (luts[c][i], luts[c][(i + 1).min(BINS - 1)]);
            out.push((a + (b - a) * frac).round().clamp(0.0, 255.0) as u8);
        }
    }
    out
}

/// Develops `path` at full sensor resolution, tone-matched to the camera's
/// embedded JPEG. The result is in sensor orientation.
pub fn develop(path: &Path) -> Result<RgbImage> {
    let (w, h, data) = linear(path)?;
    let mut src_hist = vec![[0u64; BINS]; 3];
    for px in data.as_chunks::<3>().0.iter().step_by(7) {
        for c in 0..3 {
            src_hist[c][bin(px[c])] += 1;
        }
    }
    // Reference look: the camera's embedded JPEG, unrotated like the sensor data.
    let luts: Vec<Vec<f32>> = match reference(path) {
        Some(reference) => {
            let mut ref_hist = [[0u64; 256]; 3];
            for px in reference.pixels() {
                for c in 0..3 {
                    ref_hist[c][px[c] as usize] += 1;
                }
            }
            (0..3).map(|c| match_lut(&src_hist[c], &ref_hist[c])).collect()
        }
        None => vec![neutral_lut(1.0); 3],
    };
    RgbImage::from_raw(w, h, apply(&data, &luts)).ok_or(Error::Malformed)
}

/// Share of pixels allowed to clip when the exposure is normalised.
const CLIP_SHARE: f64 = 0.002;
/// Strength of the contrast curve of the neutral rendering (0 = none).
const NEUTRAL_CONTRAST: f32 = 0.3;

/// One tone curve for all channels: exposure gain, then a gentle S-curve.
fn neutral_lut(gain: f32) -> Vec<f32> {
    (0..BINS)
        .map(|b| {
            let x = (b as f32 / (BINS - 1) as f32 * gain).min(1.0);
            let s = x * x * (3.0 - 2.0 * x);
            (x + (s - x) * NEUTRAL_CONTRAST) * 255.0
        })
        .collect()
}

/// Develops `path` without the in-camera look: sensor data, white balance,
/// colour matrix and one plain tone curve. The result is in sensor orientation.
pub fn neutral(path: &Path) -> Result<RgbImage> {
    let (w, h, data) = linear(path)?;
    // Normalise exposure so the brightest pixels just reach white.
    let mut hist = vec![0u64; BINS];
    for px in data.as_chunks::<3>().0.iter().step_by(7) {
        hist[bin(px[0].max(px[1]).max(px[2]))] += 1;
    }
    let total: u64 = hist.iter().sum();
    let mut above = 0u64;
    let white = (0..BINS).rev().find(|&b| {
        above += hist[b];
        above as f64 > total as f64 * CLIP_SHARE
    });
    let gain = white.map_or(1.0, |b| ((BINS - 1) as f32 / b.max(1) as f32).clamp(1.0, 4.0));
    RgbImage::from_raw(w, h, apply(&data, &vec![neutral_lut(gain); 3])).ok_or(Error::Malformed)
}

fn reference(path: &Path) -> Option<RgbImage> {
    let src = crate::io::Src::open(path).ok()?;
    let p = crate::container::parse(&src);
    let j = p.preview_for(REFERENCE_EDGE)?;
    preview::decode_jpeg(&src.read(j.offset, j.len as usize)?, REFERENCE_EDGE).ok()
}
