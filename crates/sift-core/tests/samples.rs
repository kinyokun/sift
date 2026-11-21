//! Checks against real camera files. Run `samples/fetch.sh` first; without
//! the samples every test here passes trivially.

use sift_core::{formats, meta, preview, rating, render, stack};
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
        let img = match preview::pixels(&f, 320) {
            // Canon files shot with HDR PQ embed HEVC previews instead of JPEG.
            Err(sift_core::Error::NoPreview) if m.low_res => continue,
            img => img.unwrap_or_else(|e| panic!("{}: {e}", name(&f))),
        };
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
fn fujifilm_focus_pixel_matches_exiftool() {
    for f in samples().into_iter().filter(|f| formats::ext_of(f) == "raf") {
        let Some(tags) = exiftool(&f, &["FocusPixel", "ExifImageWidth", "ExifImageHeight", "Orientation"]) else { return };
        let v: Vec<f32> = tags.iter().flat_map(|t| t.split_whitespace()).filter_map(|n| n.parse().ok()).collect();
        let [x, y, w, h, orientation] = v[..] else { panic!("{}: {tags:?}", name(&f)) };
        if orientation != 1.0 {
            continue;
        }
        let af = meta::read(&f).unwrap().af.unwrap_or_else(|| panic!("{}: AF area not decoded", name(&f)));
        let b = af.boxes[0];
        assert_eq!(af.source, "fujifilm");
        let close = |a: f32, e: f32| (a - e).abs() < 0.002;
        assert!(close(b.x + b.w / 2.0, x / w) && close(b.y + b.h / 2.0, y / h), "{}: {b:?} vs {x},{y} of {w}x{h}", name(&f));
    }
}

#[test]
fn panasonic_af_point_matches_exiftool() {
    for f in samples().into_iter().filter(|f| matches!(formats::ext_of(f).as_str(), "rw2" | "rwl")) {
        let Some(tags) = exiftool(&f, &["AFPointPosition", "AFAreaSize", "Orientation"]) else { return };
        let v: Vec<f32> = tags.iter().flat_map(|t| t.split_whitespace()).filter_map(|n| n.parse().ok()).collect();
        let orientation = *v.last().unwrap();
        if orientation != 1.0 || v.len() < 3 || v[0] > 1.0 {
            continue;
        }
        let (x, y) = (v[0], v[1]);
        let size = (v.len() == 5 && v[2] <= 1.0).then(|| (v[2], v[3]));
        let af = meta::read(&f).unwrap().af.unwrap_or_else(|| panic!("{}: AF area not decoded", name(&f)));
        let b = af.boxes[0];
        assert_eq!(af.source, "panasonic");
        let close = |a: f32, e: f32| (a - e).abs() < 0.002;
        assert!(close(b.x + b.w / 2.0, x) && close(b.y + b.h / 2.0, y), "{}: {b:?} vs {x},{y}", name(&f));
        if let Some((w, h)) = size {
            assert!(close(b.w, w) && close(b.h, h), "{}: size {b:?} vs {w}x{h}", name(&f));
        }
    }
}

#[test]
fn pentax_and_ricoh_look_matches_exiftool() {
    for f in samples().into_iter().filter(|f| name(f).starts_with("Pentax__") || name(f).starts_with("Ricoh__")) {
        // The names, not the numbers ExifTool prints with -n.
        let Ok(out) = Command::new("exiftool").args(["-s", "-S", "-ImageTone", "-ImageEffects"]).arg(&f).output() else { return };
        let Some(expected) = String::from_utf8_lossy(&out.stdout).lines().next().map(str::to_string) else { continue };
        let look = meta::read(&f).unwrap().look.unwrap_or_else(|| panic!("{}: no look", name(&f)));
        assert_eq!(look.name, expected, "{}", name(&f));
    }
}

/// Every Pentax and Ricoh sample focused automatically shows where.
#[test]
fn pentax_and_ricoh_af_is_decoded() {
    for f in samples().into_iter().filter(|f| name(f).starts_with("Pentax__") || name(f).starts_with("Ricoh__")) {
        let Some(tags) = exiftool(&f, &["FocusMode"]) else { return };
        let af = meta::read(&f).unwrap().af;
        // Pentax FocusMode 3 is manual focus.
        if name(&f).starts_with("Pentax__") && tags.first().map(String::as_str) == Some("3") {
            assert!(af.is_none(), "{}", name(&f));
            continue;
        }
        let af = af.unwrap_or_else(|| panic!("{}: AF not decoded", name(&f)));
        assert!(matches!(af.source, "pentax" | "ricoh"), "{}", name(&f));
        assert!(!af.boxes.is_empty(), "{}", name(&f));
        for b in &af.boxes {
            assert!(b.x >= 0.0 && b.y >= 0.0 && b.x + b.w <= 1.0 && b.y + b.h <= 1.0, "{}: {b:?}", name(&f));
        }
    }
}
/// An RW2 shot in 16:9, 3:2 or 1:1 records a crop of the sensor while its
/// JPEGs and the sensor data decoded show the whole sensor (ImageIO cuts
/// some and not others); every rendition is cut to the crop.
#[test]
fn rw2_shot_in_another_aspect_ratio_is_shown_as_recorded() {
    for f in samples().into_iter().filter(|f| matches!(formats::ext_of(f).as_str(), "rw2" | "rwl")) {
        let Some(tags) = exiftool(&f, &["CropLeft", "CropTop", "CropRight", "CropBottom", "ImageWidth", "ImageHeight"]) else { return };
        let v: Vec<f64> = tags.iter().filter_map(|t| t.parse().ok()).collect();
        // Older bodies record no crop: their sensor borders are the picture already.
        let [l, t, r, b, w, h] = v[..] else { continue };
        let aspect = (r - l) / (b - t);
        let cropped = ((r - l) - w).abs() > 8.0 || ((b - t) - h).abs() > 8.0;
        let near = |w: u32, h: u32| (w as f64 / h as f64 / aspect - 1.0).abs() < 0.01;
        let m = meta::read(&f).unwrap();
        assert!(near(m.preview_width, m.preview_height), "{}: preview {}x{} vs {aspect}", name(&f), m.preview_width, m.preview_height);
        let full = preview::decode_jpeg(&preview::full(&f).unwrap(), u32::MAX).unwrap();
        assert!(near(full.width(), full.height()), "{}: full {:?}", name(&f), full.dimensions());
        assert_eq!(full.dimensions(), (m.preview_width, m.preview_height), "{}", name(&f));
        let thumb = preview::pixels(&f, 320).unwrap();
        assert!(near(thumb.width(), thumb.height()) && thumb.width().max(thumb.height()) == 320, "{}: thumb {:?}", name(&f), thumb.dimensions());
        let screen = preview::decode_jpeg(&preview::screen(&f, 2048, 88).unwrap(), u32::MAX).unwrap();
        assert!(near(screen.width(), screen.height()), "{}: screen {:?}", name(&f), screen.dimensions());
        if cropped {
            assert!(preview::embedded_screen(&f, 2048).unwrap().is_none(), "{}: uncut embedded preview", name(&f));
            for (what, jpeg) in [("original", render::original(&f, 80)), ("developed", render::matched(&f, 80))] {
                let img = preview::decode_jpeg(&jpeg.unwrap_or_else(|e| panic!("{}: {what}: {e}", name(&f))), 1024).unwrap();
                assert!(near(img.width(), img.height()), "{}: {what} {:?}", name(&f), img.dimensions());
            }
        }
    }
}

/// The DC-GH6 samples were shot from one spot with one AF area in 3:2 and
/// 16:9, which crop the sensor differently: on the sensor the area is the same.
#[test]
fn panasonic_af_area_stays_put_across_aspect_ratios() {
    let shot = |aspect: &str| samples().into_iter().find(|f| name(f).starts_with("Panasonic__DC-GH6__") && name(f).contains(aspect));
    let (Some(a), Some(b)) = (shot("3-2"), shot("16-9")) else { return };
    let on_sensor = |f: &Path| {
        let b = meta::read(f).unwrap().af.unwrap().boxes[0];
        let src = sift_core::io::Src::open(f).unwrap();
        let c = sift_core::container::parse(&src).recorded_crop().expect("a crop");
        let ([sl, st, sr, sb], [cl, ct, cr, cb]) = (c.sensor.map(|v| v as f32), c.crop.map(|v| v as f32));
        let (sw, sh, cw, ch) = (sr - sl, sb - st, cr - cl, cb - ct);
        ((cl - sl + b.x * cw) / sw, (ct - st + b.y * ch) / sh, b.w * cw / sw, b.h * ch / sh)
    };
    let (a, b) = (on_sensor(&a), on_sensor(&b));
    let close = |p: f32, q: f32| (p - q).abs() < 0.002;
    assert!(close(a.0, b.0) && close(a.1, b.1) && close(a.2, b.2) && close(a.3, b.3), "{a:?} vs {b:?}");
}

#[test]
fn drive_mode_and_burst_frame_number_match_exiftool() {
    let tags = [
        "Nikon:ShootingMode",
        "Canon:ContinuousDrive",
        "Sony:ReleaseMode",
        "Sony:SequenceNumber",
        "FujiFilm:DriveMode",
        "FujiFilm:AutoBracketing",
        "FujiFilm:SequenceNumber",
        "Panasonic:BurstMode",
        "Panasonic:SequenceNumber",
        "Olympus:DriveMode",
        "Pentax:DriveMode",
        "Apple:BurstUUID",
        "Ricoh:DriveMode",
    ];
    for f in samples() {
        // One tab-separated row of printed values, `-` for tags the file lacks.
        let Ok(out) = Command::new("exiftool").arg("-T").args(tags.iter().map(|t| format!("-{t}"))).arg(&f).output() else { return };
        let row = String::from_utf8_lossy(&out.stdout).trim_end().to_string();
        let v: Vec<Option<&str>> = row.split('\t').map(|s| (s != "-").then_some(s)).collect();
        assert_eq!(v.len(), tags.len(), "{}", name(&f));
        let number = |s: Option<&str>| s.and_then(|s| s.parse::<u32>().ok()).filter(|n| *n > 0);
        let (continuous, seq) = if let Some(s) = v[0] {
            (Some(s.contains("Continuous")), None)
        } else if let Some(s) = v[1] {
            (Some(s.starts_with("Continuous")), None)
        } else if let Some(s) = v[2] {
            (Some(s != "Normal"), number(v[3]))
        } else if let Some(s) = v[4] {
            (Some(s != "Single" || v[5].is_some_and(|b| b != "Off")), number(v[6]))
        } else if let Some(s) = v[7] {
            (Some(s != "Off"), number(v[8]))
        } else if let Some(s) = v[9] {
            (Some(!s.starts_with("Single Shot")), s.split("Shot ").nth(1).and_then(|n| n.split(|c: char| !c.is_ascii_digit()).next()).and_then(|n| number(Some(n))))
        } else if let Some(s) = v[10].or(v[12]) {
            (Some(!s.starts_with("Single-frame")), None)
        } else {
            (None, None)
        };
        let h = stack::read(&f).unwrap();
        assert_eq!(h.continuous, continuous, "{}: drive mode {row}", name(&f));
        // Nikon numbers its series in a record of its own, checked elsewhere.
        if v[0].is_none() {
            assert_eq!(h.seq, seq, "{}: frame number {row}", name(&f));
            assert_eq!(h.series.is_some(), v[11].is_some(), "{}: series {row}", name(&f));
        }
    }
}
/// The level as ExifTool reads it, roll clockwise and pitch upward.
#[test]
fn level_matches_exiftool() {
    let mut seen = 0;
    for f in samples() {
        let Ok(out) = Command::new("exiftool").args(["-n", "-T", "-RollAngle", "-PitchAngle"]).arg(&f).output() else { return };
        let row = String::from_utf8_lossy(&out.stdout).trim_end().to_string();
        let v: Vec<Option<f32>> = row.split('\t').map(|s| s.parse().ok()).collect();
        let got = meta::read(&f).unwrap().level;
        let close = |a: f32, b: f32| (a - b).abs() < 0.06;
        match (v[0], got) {
            (Some(roll), Some(l)) => {
                seen += 1;
                assert!(close(l.roll, roll), "{}: roll {} vs {row}", name(&f), l.roll);
                assert_eq!(l.pitch.is_some(), v[1].is_some(), "{}: pitch {:?} vs {row}", name(&f), l.pitch);
                if let (Some(p), Some(q)) = (l.pitch, v[1]) {
                    assert!(close(p, q), "{}: pitch {p} vs {row}", name(&f));
                }
            }
            // iPhones only say which way gravity pulled.
            (None, Some(_)) if name(&f).starts_with("Apple__") => {}
            // Ricoh's GR bodies fill the record with zeros and no orientation.
            (Some(0.0), None) if v[1] == Some(0.0) && name(&f).starts_with("Ricoh__") => {}
            (want, got) => assert_eq!(want, got.map(|l| l.roll), "{}: {row}", name(&f)),
        }
    }
    assert!(seen > 0 || samples().is_empty());
}

/// Adobe RGB as ExifTool reads the file: an EXIF ColorSpace other than sRGB,
/// or a MakerNote's ColorSpace.
#[test]
fn adobe_rgb_matches_exiftool() {
    let mut adobe = 0;
    for f in samples() {
        let Ok(out) = Command::new("exiftool").args(["-T", "-EXIF:ColorSpace", "-MakerNotes:ColorSpace"]).arg(&f).output() else { return };
        let row = String::from_utf8_lossy(&out.stdout).trim_end().to_string();
        let v: Vec<&str> = row.split('\t').collect();
        let want = matches!(v[0], "Uncalibrated" | "Adobe RGB") || v[1] == "Adobe RGB";
        let src = sift_core::io::Src::open(&f).unwrap();
        assert_eq!(sift_core::exif::adobe_rgb(&sift_core::container::parse(&src)), want, "{}: {row}", name(&f));
        adobe += want as usize;
    }
    assert!(adobe > 0 || samples().is_empty());
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
        // Samples may carry a rating already (the camera's, or one given while trying the app on them).
        let original = meta::read(&copy).unwrap().rating;
        let rating = if original == 4 { 3 } else { 4 };

        let stored = rating::write(&copy, rating).unwrap_or_else(|e| panic!("{}: {e}", name(&f)));
        assert_eq!(meta::read(&copy).unwrap().rating, rating, "{}", name(&f));
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
                assert_eq!(tags.first().map(String::as_str), Some(rating.to_string().as_str()), "{}: ExifTool disagrees", name(&f));
            }
        }

        rating::write(&copy, original).unwrap();
        assert_eq!(meta::read(&copy).unwrap().rating, original, "{}", name(&f));
        if stored == rating::Stored::Embedded {
            assert_eq!(std::fs::read(&copy).unwrap(), before, "{}: restoring the rating did not restore the file", name(&f));
        } else if original == 0 {
            assert!(!rating::sidecar_path(&copy).is_file(), "{}: sidecar left behind", name(&f));
        }
    }
    let _ = std::fs::remove_dir_all(&dir);
}
