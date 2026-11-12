//! Animals and their eyes, for the eye close-up on photos without people.
//!
//! Two networks, both run through `tract` like the face detector:
//!
//! - YOLOX-Tiny (Ge et al., "YOLOX: Exceeding YOLO Series in 2021";
//!   Apache-2.0, Megvii's own ONNX export) finds the animals among COCO's
//!   classes: birds, cats, dogs, horses, sheep, cows, elephants, bears,
//!   zebras and giraffes.
//! - RTMPose-m trained on AP-10K (Jiang et al., "RTMPose", OpenMMLab,
//!   Apache-2.0; Yu et al., "AP-10K") places 17 keypoints on each animal, of
//!   which the eyes and the nose are used. AP-10K holds mammals only; on
//!   birds the answer is kept when the network is confident and otherwise
//!   the caller falls back to the animal's box.

use crate::af::AfBox;
use crate::{Error, Result};
use image::{imageops, Rgb, RgbImage};
use serde::Serialize;
use tract_onnx::prelude::*;

static FINDER: &[u8] = include_bytes!("../assets/yolox-tiny.onnx");
static POSE: &[u8] = include_bytes!("../assets/rtmpose-ap10k.onnx");

const FIND_INPUT: usize = 416;
const FIND_STRIDES: [usize; 3] = [8, 16, 32];
const FIND_SCORE_MIN: f32 = 0.35;
const NMS_IOU: f32 = 0.45;
/// COCO's animal classes sit together: bird, cat, dog, horse, sheep, cow, elephant, bear, zebra, giraffe.
const FIRST_ANIMAL: usize = 14;
const KINDS: [&str; 10] = ["bird", "cat", "dog", "horse", "sheep", "cow", "elephant", "bear", "zebra", "giraffe"];

const POSE_SIZE: usize = 256;
/// The pose network sees the animal's box grown by this much, as in training.
const POSE_PADDING: f32 = 1.25;
/// SimCC bins per input pixel.
const SIMCC_SPLIT: f32 = 2.0;
/// Keypoints below this confidence are not believed.
const EYE_SCORE_MIN: f32 = 0.35;
const MEAN: [f32; 3] = [123.675, 116.28, 103.53];
const STD: [f32; 3] = [58.395, 57.12, 57.375];

#[derive(Serialize, Clone, Debug)]
pub struct Animal {
    pub kind: &'static str,
    /// Bounding box in normalised image coordinates.
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    pub score: f32,
    /// The eyes the pose network is confident about (none, one or both).
    pub eyes: Vec<(f32, f32)>,
}

impl Animal {
    fn area(&self) -> f32 {
        self.w * self.h
    }

    fn iou(&self, o: &Animal) -> f32 {
        let ix = (self.x + self.w).min(o.x + o.w) - self.x.max(o.x);
        let iy = (self.y + self.h).min(o.y + o.h) - self.y.max(o.y);
        if ix <= 0.0 || iy <= 0.0 {
            return 0.0;
        }
        ix * iy / (self.area() + o.area() - ix * iy)
    }

    fn contains(&self, px: f32, py: f32) -> bool {
        px >= self.x && px <= self.x + self.w && py >= self.y && py <= self.y + self.h
    }

    /// Where to look: between the eyes, or on the only one visible.
    pub fn eye_center(&self) -> Option<(f32, f32)> {
        let n = self.eyes.len() as f32;
        (!self.eyes.is_empty()).then(|| (self.eyes.iter().map(|e| e.0).sum::<f32>() / n, self.eyes.iter().map(|e| e.1).sum::<f32>() / n))
    }
}

type Plan = Arc<TypedRunnableModel>;

pub struct Detector {
    finder: Plan,
    pose: Plan,
}

fn failed(e: impl std::fmt::Display) -> Error {
    Error::Decode(format!("animal model: {e}"))
}

fn plan(model: &[u8], size: usize) -> Result<Plan> {
    // The pose export annotates its inner tensors with a symbolic batch size,
    // which contradicts the fixed input; the annotations are not needed.
    tract_onnx::onnx()
        .with_ignore_value_info(true)
        .model_for_read(&mut std::io::Cursor::new(model))
        .map_err(failed)?
        .with_input_fact(0, f32::fact([1, 3, size, size]).into())
        .map_err(failed)?
        .into_optimized()
        .map_err(failed)?
        .into_runnable()
        .map_err(failed)
}

impl Detector {
    pub fn new() -> Result<Detector> {
        Ok(Detector { finder: plan(FINDER, FIND_INPUT)?, pose: plan(POSE, POSE_SIZE)? })
    }

    /// The animals in an upright image, without their eyes yet.
    fn find(&self, img: &RgbImage) -> Result<Vec<Animal>> {
        let (iw, ih) = (img.width() as f32, img.height() as f32);
        let scale = FIND_INPUT as f32 / iw.max(ih);
        let (sw, sh) = (((iw * scale) as u32).clamp(1, FIND_INPUT as u32), ((ih * scale) as u32).clamp(1, FIND_INPUT as u32));
        let small = imageops::thumbnail(img, sw, sh);
        // Letterboxed from the top left on grey, BGR, values 0..255: YOLOX's own preprocessing.
        let mut input = tract_ndarray::Array4::<f32>::from_elem((1, 3, FIND_INPUT, FIND_INPUT), 114.0);
        for (x, y, px) in small.enumerate_pixels() {
            for c in 0..3 {
                input[[0, c, y as usize, x as usize]] = px[2 - c] as f32;
            }
        }
        let tensor: Tensor = input.into();
        let res = self.finder.run(tvec!(tensor.into())).map_err(failed)?;
        let out = res[0].to_plain_array_view::<f32>().map_err(failed)?;
        let rows: Vec<f32> = out.iter().copied().collect();
        const WIDTH: usize = 85;
        let mut found = Vec::new();
        let mut row = 0;
        for stride in FIND_STRIDES {
            let cells = FIND_INPUT / stride;
            for gy in 0..cells {
                for gx in 0..cells {
                    let Some(v) = rows.get(row * WIDTH..(row + 1) * WIDTH) else { return Err(failed("unexpected output")) };
                    row += 1;
                    let (kind, class) = (0..KINDS.len()).map(|k| (k, v[5 + FIRST_ANIMAL + k])).max_by(|a, b| a.1.total_cmp(&b.1)).unwrap_or((0, 0.0));
                    let score = v[4] * class;
                    if score < FIND_SCORE_MIN {
                        continue;
                    }
                    let s = stride as f32;
                    let (cx, cy) = ((v[0] + gx as f32) * s, (v[1] + gy as f32) * s);
                    let (w, h) = (v[2].exp() * s, v[3].exp() * s);
                    let (x0, y0) = (((cx - w / 2.0) / scale / iw).max(0.0), ((cy - h / 2.0) / scale / ih).max(0.0));
                    let (x1, y1) = (((cx + w / 2.0) / scale / iw).min(1.0), ((cy + h / 2.0) / scale / ih).min(1.0));
                    found.push(Animal { kind: KINDS[kind], x: x0, y: y0, w: x1 - x0, h: y1 - y0, score, eyes: Vec::new() });
                }
            }
        }
        found.sort_by(|a, b| b.score.total_cmp(&a.score));
        // One animal, one box, whatever species the network wavered between.
        let mut kept: Vec<Animal> = Vec::new();
        for a in found {
            if kept.iter().all(|k| k.iou(&a) < NMS_IOU) {
                kept.push(a);
            }
        }
        Ok(kept)
    }

    /// Looks for the eyes of one animal and fills them in.
    fn locate_eyes(&self, img: &RgbImage, animal: &mut Animal) -> Result<()> {
        let (iw, ih) = (img.width() as f32, img.height() as f32);
        // A square around the box, padded; parts outside the photo stay black.
        let side = (animal.w * iw).max(animal.h * ih) * POSE_PADDING;
        let (left, top) = ((animal.x + animal.w / 2.0) * iw - side / 2.0, (animal.y + animal.h / 2.0) * ih - side / 2.0);
        let (x0, y0) = (left.max(0.0).floor(), top.max(0.0).floor());
        let (x1, y1) = ((left + side).min(iw).ceil(), (top + side).min(ih).ceil());
        if x1 - x0 < 8.0 || y1 - y0 < 8.0 {
            return Ok(());
        }
        let k = POSE_SIZE as f32 / side;
        let part = imageops::resize(
            &*imageops::crop_imm(img, x0 as u32, y0 as u32, (x1 - x0) as u32, (y1 - y0) as u32),
            (((x1 - x0) * k).round() as u32).clamp(1, POSE_SIZE as u32),
            (((y1 - y0) * k).round() as u32).clamp(1, POSE_SIZE as u32),
            imageops::FilterType::Triangle,
        );
        let mut canvas = RgbImage::from_pixel(POSE_SIZE as u32, POSE_SIZE as u32, Rgb([0, 0, 0]));
        imageops::replace(&mut canvas, &part, ((x0 - left) * k).round() as i64, ((y0 - top) * k).round() as i64);
        let mut input = tract_ndarray::Array4::<f32>::zeros((1, 3, POSE_SIZE, POSE_SIZE));
        for (x, y, px) in canvas.enumerate_pixels() {
            for c in 0..3 {
                input[[0, c, y as usize, x as usize]] = (px[c] as f32 - MEAN[c]) / STD[c];
            }
        }
        let tensor: Tensor = input.into();
        let res = self.pose.run(tvec!(tensor.into())).map_err(failed)?;
        if res.len() < 2 {
            return Err(failed("unexpected outputs"));
        }
        let xs = res[0].to_plain_array_view::<f32>().map_err(failed)?;
        let ys = res[1].to_plain_array_view::<f32>().map_err(failed)?;
        let (xs, ys): (Vec<f32>, Vec<f32>) = (xs.iter().copied().collect(), ys.iter().copied().collect());
        let bins = (POSE_SIZE as f32 * SIMCC_SPLIT) as usize;
        let peak = |v: &[f32], joint: usize| -> Option<(usize, f32)> {
            v.get(joint * bins..(joint + 1) * bins)?.iter().copied().enumerate().max_by(|a, b| a.1.total_cmp(&b.1))
        };
        // AP-10K keypoints 0 and 1 are the eyes.
        for joint in 0..2 {
            let (Some((bx, sx)), Some((by, sy))) = (peak(&xs, joint), peak(&ys, joint)) else { continue };
            if sx.min(sy) < EYE_SCORE_MIN {
                continue;
            }
            let (px, py) = ((bx as f32 / SIMCC_SPLIT / k + left) / iw, (by as f32 / SIMCC_SPLIT / k + top) / ih);
            if animal.contains(px, py) {
                animal.eyes.push((px, py));
            }
        }
        Ok(())
    }

    /// The main animal of an upright image with its eyes, if there is one:
    /// the one under an in-focus AF area, otherwise the largest.
    pub fn subject(&self, img: &RgbImage, af: &[AfBox]) -> Result<Option<Animal>> {
        let found = self.find(img)?;
        let chosen = af
            .iter()
            .filter(|b| b.focused)
            .find_map(|b| found.iter().filter(|a| a.contains(b.x + b.w / 2.0, b.y + b.h / 2.0)).min_by(|a, b| a.area().total_cmp(&b.area())))
            .or_else(|| found.iter().max_by(|a, b| a.area().total_cmp(&b.area())));
        let Some(mut animal) = chosen.cloned() else { return Ok(None) };
        self.locate_eyes(img, &mut animal)?;
        Ok(Some(animal))
    }

    /// Every animal with its eyes; for inspection (`probe --animal`).
    pub fn all(&self, img: &RgbImage) -> Result<Vec<Animal>> {
        let mut found = self.find(img)?;
        for a in &mut found {
            self.locate_eyes(img, a)?;
        }
        Ok(found)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn both_networks_load_and_see_nothing_in_an_empty_frame() {
        let detector = Detector::new().expect("models load");
        let blank = RgbImage::from_pixel(640, 480, Rgb([128, 128, 128]));
        assert!(detector.subject(&blank, &[]).expect("runs").is_none());
        // The pose network runs on any box without failing.
        let mut whole = Animal { kind: "cat", x: 0.0, y: 0.0, w: 1.0, h: 1.0, score: 1.0, eyes: Vec::new() };
        detector.locate_eyes(&blank, &mut whole).expect("runs");
        assert!(whole.eyes.len() <= 2);
    }
}
