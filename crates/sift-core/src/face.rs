//! Face and eye localisation with YuNet (Wu et al., "YuNet: A Tiny
//! Millisecond-level Face Detector", 2023), run through the pure-Rust `tract`
//! inference engine so it works offline on every platform. The ONNX model is
//! the MIT-licensed `face_detection_yunet_2023mar` from the OpenCV model zoo.

use crate::af::AfBox;
use crate::{Error, Result};
use image::{imageops, RgbImage};
use serde::Serialize;
use tract_onnx::prelude::*;

static MODEL: &[u8] = include_bytes!("../assets/yunet.onnx");
const INPUT: usize = 640;
const STRIDES: [usize; 3] = [8, 16, 32];
const SCORE_MIN: f32 = 0.6;
const NMS_IOU: f32 = 0.3;

#[derive(Serialize, Clone, Copy, Debug)]
pub struct Face {
    /// Bounding box in normalised image coordinates.
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    pub score: f32,
    /// Eye centres, image-left eye first.
    pub eyes: [(f32, f32); 2],
}

impl Face {
    fn area(&self) -> f32 {
        self.w * self.h
    }

    fn iou(&self, o: &Face) -> f32 {
        let ix = (self.x + self.w).min(o.x + o.w) - self.x.max(o.x);
        let iy = (self.y + self.h).min(o.y + o.h) - self.y.max(o.y);
        if ix <= 0.0 || iy <= 0.0 {
            return 0.0;
        }
        let inter = ix * iy;
        inter / (self.area() + o.area() - inter)
    }

    fn contains(&self, px: f32, py: f32) -> bool {
        let (mx, my) = (self.w * 0.15, self.h * 0.15);
        px >= self.x - mx && px <= self.x + self.w + mx && py >= self.y - my && py <= self.y + self.h + my
    }

    /// Midpoint between the eyes.
    pub fn eye_center(&self) -> (f32, f32) {
        ((self.eyes[0].0 + self.eyes[1].0) / 2.0, (self.eyes[0].1 + self.eyes[1].1) / 2.0)
    }
}

type Plan = Arc<TypedRunnableModel>;

pub struct Detector {
    plan: Plan,
}

impl Detector {
    pub fn new() -> Result<Detector> {
        let err = |e: TractError| Error::Decode(format!("face model: {e}"));
        let plan = tract_onnx::onnx()
            .model_for_read(&mut std::io::Cursor::new(MODEL))
            .map_err(err)?
            .with_input_fact(0, f32::fact([1, 3, INPUT, INPUT]).into())
            .map_err(err)?
            .into_optimized()
            .map_err(err)?
            .into_runnable()
            .map_err(err)?;
        Ok(Detector { plan })
    }

    /// Runs the network on one region `(x, y, w, h)` of `img`, in pixels.
    fn detect_region(&self, img: &RgbImage, region: (u32, u32, u32, u32), out: &mut Vec<Face>) -> Result<()> {
        let (rx, ry, rw, rh) = region;
        let scale = INPUT as f32 / rw.max(rh) as f32;
        let (sw, sh) = (((rw as f32 * scale) as u32).clamp(1, INPUT as u32), ((rh as f32 * scale) as u32).clamp(1, INPUT as u32));
        let small = imageops::thumbnail(&*imageops::crop_imm(img, rx, ry, rw, rh), sw, sh);

        // Letterboxed NCHW input in BGR order, values 0..255 as the model expects.
        let mut input = tract_ndarray::Array4::<f32>::zeros((1, 3, INPUT, INPUT));
        for (x, y, px) in small.enumerate_pixels() {
            for c in 0..3 {
                input[[0, c, y as usize, x as usize]] = px[2 - c] as f32;
            }
        }
        let tensor: Tensor = input.into();
        let res = self.plan.run(tvec!(tensor.into())).map_err(|e| Error::Decode(format!("face model: {e}")))?;
        if res.len() < 12 {
            return Err(Error::Decode("face model: unexpected outputs".into()));
        }
        let (iw, ih) = (img.width() as f32, img.height() as f32);
        for (i, &stride) in STRIDES.iter().enumerate() {
            let view = |k: usize| -> Result<Vec<f32>> {
                let v = res[k].to_plain_array_view::<f32>().map_err(|e| Error::Decode(e.to_string()))?;
                Ok(v.iter().copied().collect())
            };
            let (cls, obj, bbox, kps) = (view(i)?, view(i + 3)?, view(i + 6)?, view(i + 9)?);
            let cols = INPUT / stride;
            for idx in 0..cls.len().min(cols * cols) {
                let score = (cls[idx].clamp(0.0, 1.0) * obj[idx].clamp(0.0, 1.0)).sqrt();
                if score < SCORE_MIN {
                    continue;
                }
                let (c, r, s) = ((idx % cols) as f32, (idx / cols) as f32, stride as f32);
                let (cx, cy) = ((c + bbox[idx * 4]) * s, (r + bbox[idx * 4 + 1]) * s);
                let (w, h) = (bbox[idx * 4 + 2].exp() * s, bbox[idx * 4 + 3].exp() * s);
                // network pixels -> full-image normalised coordinates
                let nx = |v: f32| (v / scale + rx as f32) / iw;
                let ny = |v: f32| (v / scale + ry as f32) / ih;
                let eye = |n: usize| (nx((kps[idx * 10 + 2 * n] + c) * s), ny((kps[idx * 10 + 2 * n + 1] + r) * s));
                out.push(Face {
                    x: nx(cx - w / 2.0),
                    y: ny(cy - h / 2.0),
                    w: w / scale / iw,
                    h: h / scale / ih,
                    score,
                    eyes: [eye(0), eye(1)],
                });
            }
        }
        Ok(())
    }

    /// Detects faces in an upright image, best score first. The whole frame is
    /// tried first; if nothing is found, four overlapping quadrants are
    /// searched so small faces in full-body shots are still picked up.
    pub fn detect(&self, img: &RgbImage) -> Result<Vec<Face>> {
        let (w, h) = img.dimensions();
        let mut found = Vec::new();
        self.detect_region(img, (0, 0, w, h), &mut found)?;
        if found.is_empty() && w.max(h) > INPUT as u32 {
            let (tw, th) = (w * 3 / 5, h * 3 / 5);
            for (x, y) in [(0, 0), (w - tw, 0), (0, h - th), (w - tw, h - th)] {
                self.detect_region(img, (x, y, tw, th), &mut found)?;
            }
        }
        found.sort_by(|a, b| b.score.total_cmp(&a.score));
        let mut kept: Vec<Face> = Vec::new();
        for f in found {
            if kept.iter().all(|k| k.iou(&f) < NMS_IOU) {
                kept.push(f);
            }
        }
        Ok(kept)
    }
}

/// Picks the face the photographer most likely cares about: the one under an
/// in-focus AF area if there is one, otherwise the largest.
pub fn subject<'a>(faces: &'a [Face], af: &[AfBox]) -> Option<&'a Face> {
    af.iter()
        .filter(|b| b.focused)
        .find_map(|b| faces.iter().find(|f| f.contains(b.x + b.w / 2.0, b.y + b.h / 2.0)))
        .or_else(|| faces.iter().max_by(|a, b| a.area().total_cmp(&b.area())))
}
