//! Checks against real camera files. Run `samples/fetch.sh` first; without
//! the samples every test here passes trivially.

use sift_core::{formats, meta, preview, rating};
use std::path::{Path, PathBuf};
use std::process::Command;

fn samples() -> Vec<PathBuf> {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../samples");
    let mut files: Vec<PathBuf> = std::fs::read_dir(dir)
        .map(|rd| rd.flatten().map(|e| e.path()).filter(|p| formats::is_raw(p)).collect())
        .unwrap_or_default();
    files.sort();
    files
}

fn name(p: &Path) -> String {
    p.file_name().unwrap().to_string_lossy().into_owned()
}

/// Values of the given ExifTool tags, or `None` when ExifTool is not installed.
fn exiftool(path: &Path, tags: &[&str]) -> Option<Vec<String>> {
    let out = Command::new("exiftool").arg("-s").arg("-S").arg("-n").args(tags.iter().map(|t| format!("-{t}"))).arg(path).output().ok()?;
    out.status.success().then(|| String::from_utf8_lossy(&out.stdout).lines().map(str::to_string).collect())
}

#[test]
fn every_sample_has_metadata_and_a_full_size_preview() {
    for f in samples() {
        let m = meta::read(&f).unwrap_or_else(|e| panic!("{}: {e}", name(&f)));
        assert!(m.exif.model.is_some(), "{}: no camera model", name(&f));
        assert!(m.exif.shutter.is_some(), "{}: no shutter speed", name(&f));
        match preview::full(&f) {
            Ok(jpeg) => {
                assert_eq!(&jpeg[..2], &[0xFF, 0xD8], "{}", name(&f));
                let (w, h) = (m.preview_width.max(m.preview_height), m.preview_width.min(m.preview_height));
                assert!(w >= 1000 && h >= 600, "{}: preview only {w}x{h}", name(&f));
            }
            // Canon files shot with HDR PQ embed HEVC previews instead of JPEG.
            Err(e) => assert!(m.low_res, "{}: {e}", name(&f)),
        }
    }
}

#[test]
fn thumbnails_decode_upright() {
    for f in samples() {
        let m = meta::read(&f).unwrap();
        if m.preview_width == 0 {
            continue;
        }
        let img = preview::pixels(&f, 320).unwrap_or_else(|e| panic!("{}: {e}", name(&f)));
        assert_eq!(img.width().max(img.height()), 320, "{}", name(&f));
        let landscape = (m.preview_width >= m.preview_height) != (m.orientation >= 5);
        assert_eq!(img.width() >= img.height(), landscape, "{}: orientation {}", name(&f), m.orientation);
    }
}

#[test]
fn nikon_z_focus_area_matches_exiftool() {
    for f in samples().into_iter().filter(|f| formats::ext_of(f) == "nef") {
        let m = meta::read(&f).unwrap();
        let Some(tags) = exiftool(&f, &["AFImageWidth", "AFImageHeight", "AFAreaXPosition", "AFAreaYPosition", "AFAreaWidth", "AFAreaHeight"]) else {
            return;
        };
        if tags.len() < 6 {
            // No coordinates recorded: the camera stored a point bitmap or nothing.
            continue;
        }
        let v: Vec<f32> = tags.iter().map(|t| t.parse().unwrap()).collect();
        // In-camera crops (DX, 1:1, 16:9) record positions in the full AF frame
        // while the image is its centre section.
        let crop: Vec<f32> = exiftool(&f, &["CropArea"])
            .and_then(|t| t.first().map(|l| l.split_whitespace().filter_map(|n| n.parse().ok()).collect()))
            .unwrap_or_default();
        let (cw, ch) = match crop.as_slice() {
            [_, _, w, h] if *w < v[0] * 0.98 => (*w, *h),
            _ => (v[0], v[1]),
        };
        let af = m.af.unwrap_or_else(|| panic!("{}: AF area not decoded", name(&f)));
        assert_eq!(af.boxes.len(), 1, "{}", name(&f));
        let b = af.boxes[0];
        let close = |a: f32, e: f32| (a - e).abs() < 0.002;
        let (ex, ey) = ((v[2] - (v[0] - cw) / 2.0) / cw, (v[3] - (v[1] - ch) / 2.0) / ch);
        assert!(close(b.x + b.w / 2.0, ex), "{}: x {} vs {ex}", name(&f), b.x + b.w / 2.0);
        assert!(close(b.y + b.h / 2.0, ey), "{}: y {} vs {ey}", name(&f), b.y + b.h / 2.0);
        assert!(close(b.w, v[4] / cw) && close(b.h, v[5] / ch), "{}: size", name(&f));
    }
}

#[test]
fn nikon_point_bitmap_matches_exiftool_count() {
    for f in samples().into_iter().filter(|f| formats::ext_of(f) == "nef") {
        let Some(tags) = exiftool(&f, &["AFCoordinatesAvailable", "AFPointsUsed"]) else { return };
        if tags.first().map(String::as_str) != Some("0") || tags.len() < 2 {
            continue;
        }
        // ExifTool prints the raw bitmap bytes in hex; count the set bits.
        let used: u32 = tags[1].split_whitespace().filter_map(|b| u8::from_str_radix(b, 16).ok()).map(u8::count_ones).sum();
        let boxes = meta::read(&f).unwrap().af.map_or(0, |a| a.boxes.len());
        assert_eq!(boxes as u32, used, "{}", name(&f));
    }
}

#[test]
fn rating_round_trips_without_moving_image_data() {
    let dir = std::env::temp_dir().join(format!("sift-rating-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let mut done = Vec::new();
    for f in samples() {
        let ext = formats::ext_of(&f);
        if done.contains(&ext) {
            continue;
        }
        done.push(ext.clone());
        let copy = dir.join(format!("sample.{ext}"));
        std::fs::copy(&f, &copy).unwrap();
        let before = std::fs::read(&copy).unwrap();

        let stored = rating::write(&copy, 4).unwrap_or_else(|e| panic!("{}: {e}", name(&f)));
        assert_eq!(meta::read(&copy).unwrap().rating, 4, "{}", name(&f));
        let after = std::fs::read(&copy).unwrap();
        assert_eq!(before.len(), after.len(), "{}: RAW size changed", name(&f));
        let changed = before.iter().zip(&after).filter(|(a, b)| a != b).count();
        match stored {
            rating::Stored::Embedded => assert!((1..=2).contains(&changed), "{}: {changed} bytes changed", name(&f)),
            rating::Stored::Sidecar => {
                assert_eq!(changed, 0, "{}", name(&f));
                assert!(rating::sidecar_path(&copy).is_file());
            }
        }
        if let Some(tags) = exiftool(&copy, &["Rating"]) {
            if stored == rating::Stored::Embedded {
                assert_eq!(tags.first().map(String::as_str), Some("4"), "{}: ExifTool disagrees", name(&f));
            }
        }

        rating::write(&copy, 0).unwrap();
        assert_eq!(meta::read(&copy).unwrap().rating, 0, "{}", name(&f));
        if stored == rating::Stored::Embedded {
            assert_eq!(std::fs::read(&copy).unwrap(), before, "{}: clearing did not restore the file", name(&f));
        } else {
            assert!(!rating::sidecar_path(&copy).is_file(), "{}: sidecar left behind", name(&f));
        }
    }
    let _ = std::fs::remove_dir_all(&dir);
}
