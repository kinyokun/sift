//! Prints the metadata Sift reads from each file given on the command line.
//! `probe --preview <dir> files...` also writes the full preview and a thumbnail.

use std::path::{Path, PathBuf};
use std::time::Instant;

fn main() {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let mut out: Option<PathBuf> = None;
    if args.first().map(String::as_str) == Some("--nikon-match") {
        // probe --nikon-match <engine root> <nef> <41 flexible values, comma separated>
        // Renders as shot and with the same Flexible Color values passed explicitly; they should agree.
        use sift_core::nikon::{Base, Engine, Recipe};
        let engine = Engine::new(Path::new(&args[1]).to_path_buf());
        let as_shot = engine.render(Path::new(&args[2]), &Recipe::base(Base::AsShot), 1512).expect("as shot").image;
        let mut recipe = Recipe::base(Base::FlexibleColor);
        for (slot, v) in recipe.flexible.iter_mut().zip(args[3].split(',')) {
            *slot = v.trim().parse().expect("number");
        }
        let explicit = engine.render(Path::new(&args[2]), &recipe, 1512).expect("explicit").image;
        let zero = engine.render(Path::new(&args[2]), &Recipe::base(Base::FlexibleColor), 1512).expect("zero").image;
        let diff = |a: &image::RgbImage, b: &image::RgbImage| {
            let (mut sum, mut max) = (0u64, 0u8);
            for (x, y) in a.as_raw().iter().zip(b.as_raw()) {
                let d = x.abs_diff(*y);
                sum += d as u64;
                max = max.max(d);
            }
            (sum as f64 / a.as_raw().len() as f64, max)
        };
        let mean = |a: &image::RgbImage| {
            let mut m = [0f64; 3];
            for p in a.pixels() {
                for (sum, v) in m.iter_mut().zip(p.0) {
                    *sum += v as f64;
                }
            }
            m.map(|v| (v / (a.width() * a.height()) as f64 * 100.0).round() / 100.0)
        };
        // The same comparison on 8x8 block averages removes sharpening and noise.
        let blocks = |a: &image::RgbImage| image::imageops::resize(a, a.width() / 8, a.height() / 8, image::imageops::FilterType::Triangle);
        println!("means: as shot {:?} explicit {:?} zero {:?}", mean(&as_shot), mean(&explicit), mean(&zero));
        println!("block averages, as shot vs explicit: mean {:.3} max {}", diff(&blocks(&as_shot), &blocks(&explicit)).0, diff(&blocks(&as_shot), &blocks(&explicit)).1);
        std::fs::write("/tmp/nksdk/m-asshot.jpg", sift_core::preview::encode_jpeg(&as_shot, 90).unwrap()).unwrap();
        std::fs::write("/tmp/nksdk/m-explicit.jpg", sift_core::preview::encode_jpeg(&explicit, 90).unwrap()).unwrap();
        println!("as shot vs explicit values: mean {:.3} max {}", diff(&as_shot, &explicit).0, diff(&as_shot, &explicit).1);
        println!("as shot vs all-zero Flexible Color (control): mean {:.3} max {}", diff(&as_shot, &zero).0, diff(&as_shot, &zero).1);
        return;
    }
    if args.first().map(String::as_str) == Some("--camera-look") {
        // probe --camera-look <NEF or camera JPEG> <out.NP3>
        // Saves the Picture Control the photo was shot with as an NP3 file.
        let np3 = sift_core::nikon::recorded_look(&std::fs::read(&args[1]).expect("read")).expect("camera look");
        let look = sift_core::look::parse_np3(&np3).expect("parse");
        println!("{} base {:#06x} curve={} ({} bytes)", look.name, look.base, look.curve.is_some(), np3.len());
        std::fs::write(&args[2], np3).expect("write");
        return;
    }
    if args.first().map(String::as_str) == Some("--nikon") {
        // probe --nikon <engine root> <nef> <outdir> <edge> [looks.NP3...] : develop with Nikon's renderer
        use sift_core::nikon::{Base, Engine, Recipe};
        let engine = Engine::new(Path::new(&args[1]).to_path_buf());
        let (nef, out, edge) = (Path::new(&args[2]), Path::new(&args[3]), args[4].parse::<u32>().unwrap());
        std::fs::create_dir_all(out).unwrap();
        let mut jobs = vec![("as-shot".to_string(), Recipe::base(Base::AsShot), None), ("standard".to_string(), Recipe::base(Base::Standard), None)];
        for a in &args[5..] {
            let np3 = sift_core::look::parse_np3(&std::fs::read(a).unwrap()).unwrap();
            // Every look twice: from settings (plus our curve), and recorded in a copy of the file.
            let mut settings = np3.recipe();
            settings.recorded = None;
            let mut recorded = np3.recipe();
            recorded.recorded = Some(np3.file.clone());
            jobs.push((format!("{}-settings", np3.name.replace(' ', "_")), settings, Some(np3.clone())));
            jobs.push((format!("{}-recorded", np3.name.replace(' ', "_")), recorded, Some(np3)));
        }
        for (name, recipe, np3) in jobs {
            let t = std::time::Instant::now();
            match engine.render(nef, &recipe, edge) {
                Ok(sift_core::nikon::Rendered { image: mut img, recorded }) => {
                    if let Some(np3) = np3.as_ref().filter(|_| !recorded) {
                        np3.apply_curve(&mut img);
                    }
                    println!("{name:28} {}x{} in {:.2}s base {:?} curve={} recorded={recorded}", img.width(), img.height(), t.elapsed().as_secs_f32(), recipe.base, np3.is_some_and(|n| n.curve.is_some()));
                    std::fs::write(out.join(format!("{name}.jpg")), sift_core::preview::encode_jpeg(&img, 88).unwrap()).unwrap();
                }
                Err(e) => println!("{name}: ERROR {e}"),
            }
        }
        return;
    }
    if args.first().map(String::as_str) == Some("--looks") {
        // probe --looks <photo> <outdir> looks... : parse every look and render it onto the photo
        let base = sift_core::preview::pixels(Path::new(&args[1]), 900).expect("pixels");
        std::fs::create_dir_all(&args[2]).unwrap();
        for a in &args[3..] {
            match sift_core::look::load(Path::new(a)) {
                Ok(look) => {
                    let mut img = base.clone();
                    look.lut.apply(&mut img);
                    let out = Path::new(&args[2]).join(format!("{}.jpg", look.name.replace(' ', "_")));
                    std::fs::write(out, sift_core::preview::encode_jpeg(&img, 85).unwrap()).unwrap();
                    if let Ok(n) = sift_core::look::parse_np3(&std::fs::read(a).unwrap()) {
                        println!(
                            "{:18} c{:+.0} h{:+.0} s{:+.0} w{:+.0} b{:+.0} sat{:+.0} curve={}",
                            n.name, n.contrast, n.highlights, n.shadows, n.white_level, n.black_level, n.saturation, n.curve.is_some()
                        );
                    }
                }
                Err(e) => println!("{a}: ERROR {e}"),
            }
        }
        return;
    }
    if args.first().map(String::as_str) == Some("--ratings") {
        let t = Instant::now();
        let sum: u32 = args[1..].iter().map(|a| sift_core::meta::rating(Path::new(a)).unwrap_or(0) as u32).sum();
        println!("{} files, rating sum {sum}, {:.1} ms/file", args.len() - 1, t.elapsed().as_secs_f64() * 1000.0 / (args.len() - 1) as f64);
        return;
    }
    if args.first().map(String::as_str) == Some("--face") {
        let det = sift_core::face::Detector::new().expect("model");
        for a in &args[1..] {
            let img = sift_core::preview::pixels(Path::new(a), 1280).expect("pixels");
            let t = Instant::now();
            let faces = det.detect(&img).expect("detect");
            println!("{a}: {:.1}ms {}", t.elapsed().as_secs_f64() * 1000.0, serde_json::to_string(&faces).unwrap());
        }
        return;
    }
    if args.first().map(String::as_str) == Some("--animal") {
        // probe --animal <file>... : animals and their eyes; ANIMAL_OUT=<dir> saves marked copies
        let t = Instant::now();
        let det = sift_core::animal::Detector::new().expect("model");
        println!("models ready in {:.0}ms", t.elapsed().as_secs_f64() * 1000.0);
        for a in &args[1..] {
            let mut img = sift_core::preview::pixels(Path::new(a), 1280).expect("pixels");
            let t = Instant::now();
            let found = det.all(&img).expect("detect");
            println!("{a}: {:.0}ms {}", t.elapsed().as_secs_f64() * 1000.0, serde_json::to_string(&found).unwrap());
            if let Ok(dir) = std::env::var("ANIMAL_OUT") {
                let (w, h) = (img.width() as f32, img.height() as f32);
                let mut dot = |x: f32, y: f32, r: i32, c: [u8; 3]| {
                    for dy in -r..=r {
                        for dx in -r..=r {
                            let (px, py) = ((x * w) as i32 + dx, (y * h) as i32 + dy);
                            if px >= 0 && py >= 0 && (px as u32) < img.width() && (py as u32) < img.height() {
                                img.put_pixel(px as u32, py as u32, image::Rgb(c));
                            }
                        }
                    }
                };
                for f in &found {
                    for i in 0..=100 {
                        let t = i as f32 / 100.0;
                        for (x, y) in [(f.x + f.w * t, f.y), (f.x + f.w * t, f.y + f.h), (f.x, f.y + f.h * t), (f.x + f.w, f.y + f.h * t)] {
                            dot(x, y, 2, [255, 220, 0]);
                        }
                    }
                    for e in &f.eyes {
                        dot(e.0, e.1, 7, [255, 0, 60]);
                    }
                }
                std::fs::create_dir_all(&dir).unwrap();
                let name = Path::new(a).file_stem().unwrap().to_string_lossy().into_owned();
                std::fs::write(Path::new(&dir).join(format!("{name}.jpg")), sift_core::preview::encode_jpeg(&img, 85).unwrap()).unwrap();
            }
        }
        return;
    }
    if args.first().map(String::as_str) == Some("--frame") {
        // probe --frame <outdir> <style> <file>
        let path = Path::new(&args[3]);
        let m = sift_core::meta::read(path).expect("meta");
        let img = sift_core::preview::pixels(path, 2400).expect("pixels");
        let opt: sift_core::frame::FrameOptions =
            serde_json::from_str(&format!(r#"{{"style":"{}","caption":"{}"}}"#, args[2], args.get(4).cloned().unwrap_or_default())).unwrap();
        let t = Instant::now();
        let out = sift_core::frame::render(&img, &m.exif, &opt);
        println!("frame {:.0}ms {}x{}", t.elapsed().as_secs_f64() * 1000.0, out.width(), out.height());
        std::fs::create_dir_all(&args[1]).unwrap();
        std::fs::write(Path::new(&args[1]).join(format!("frame-{}.jpg", args[2])), sift_core::preview::encode_jpeg(&out, 90).unwrap()).unwrap();
        return;
    }
    if args.first().map(String::as_str) == Some("--develop") {
        for a in &args[2..] {
            let t = Instant::now();
            let jpg = if std::env::var("ORIGINAL").is_ok() {
                sift_core::render::original(Path::new(a), 92).expect("original")
            } else {
                sift_core::render::matched(Path::new(a), 92).expect("develop")
            };
            println!("{a}: {:.0}ms {} bytes", t.elapsed().as_secs_f64() * 1000.0, jpg.len());
            let stem = Path::new(a).file_stem().unwrap().to_string_lossy().to_string();
            std::fs::create_dir_all(&args[1]).unwrap();
            std::fs::write(Path::new(&args[1]).join(format!("{stem}.dev.jpg")), jpg).unwrap();
        }
        return;
    }
    if args.first().map(String::as_str) == Some("--rate") {
        let rating: u8 = args[1].parse().expect("rating 0-5");
        for a in &args[2..] {
            println!("{a}: {:?}", sift_core::rating::write(Path::new(a), rating));
        }
        return;
    }
    if args.first().map(String::as_str) == Some("--preview") {
        out = Some(PathBuf::from(args.remove(1)));
        args.remove(0);
    }
    for a in &args {
        let path = Path::new(a);
        let t = Instant::now();
        match sift_core::meta::read(path) {
            Ok(m) => println!(
                "{}\t{:.1}ms\n{}",
                path.file_name().unwrap().to_string_lossy(),
                t.elapsed().as_secs_f64() * 1000.0,
                serde_json::to_string(&m).unwrap()
            ),
            Err(e) => println!("{a}: ERROR {e}"),
        }
        if let Some(dir) = &out {
            std::fs::create_dir_all(dir).unwrap();
            let stem = path.file_stem().unwrap().to_string_lossy().to_string();
            let t = Instant::now();
            match sift_core::preview::full(path) {
                Ok(b) => {
                    println!("  full {} bytes in {:.1}ms", b.len(), t.elapsed().as_secs_f64() * 1000.0);
                    std::fs::write(dir.join(format!("{stem}.full.jpg")), b).unwrap();
                }
                Err(e) => println!("  full ERROR {e}"),
            }
            let t2 = Instant::now();
            match sift_core::preview::screen(path, 2048, 88) {
                Ok(b) => {
                    println!("  screen {} bytes in {:.1}ms", b.len(), t2.elapsed().as_secs_f64() * 1000.0);
                    std::fs::write(dir.join(format!("{stem}.screen.jpg")), b).unwrap();
                }
                Err(e) => println!("  screen ERROR {e}"),
            }
            let t = Instant::now();
            match sift_core::preview::scaled(path, 480, 82) {
                Ok(b) => {
                    println!("  thumb {} bytes in {:.1}ms", b.len(), t.elapsed().as_secs_f64() * 1000.0);
                    std::fs::write(dir.join(format!("{stem}.thumb.jpg")), b).unwrap();
                }
                Err(e) => println!("  thumb ERROR {e}"),
            }
        }
    }
}
