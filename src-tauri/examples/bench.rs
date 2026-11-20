//! Timings of what opening a folder costs, on real files.
//!
//! cargo run -p sift --release --example bench -- <folder> [threads...]

use sift_core::{meta, preview};
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;

fn pool<T: Send>(files: &[PathBuf], threads: usize, f: impl Fn(&PathBuf) -> T + Sync) -> Vec<Option<T>> {
    let next = AtomicUsize::new(0);
    let mut out: Vec<Option<T>> = (0..files.len()).map(|_| None).collect();
    let slots: Vec<std::sync::Mutex<&mut Option<T>>> = out.iter_mut().map(std::sync::Mutex::new).collect();
    std::thread::scope(|s| {
        for _ in 0..threads {
            s.spawn(|| loop {
                let i = next.fetch_add(1, Ordering::Relaxed);
                let Some(file) = files.get(i) else { break };
                **slots[i].lock().unwrap() = Some(f(file));
            });
        }
    });
    drop(slots);
    out
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let dir = PathBuf::from(&args[0]);
    let what = std::env::var("WHAT").unwrap_or_else(|_| "briefs".into());
    let threads: Vec<usize> = args[1..].iter().filter_map(|t| t.parse().ok()).collect();
    let mut files: Vec<PathBuf> = std::fs::read_dir(&dir).unwrap().flatten().map(|e| e.path()).filter(|p| sift_core::formats::is_supported(p)).collect();
    files.sort();
    if let Ok(only) = std::env::var("EXT") {
        files.retain(|p| p.extension().is_some_and(|x| x.eq_ignore_ascii_case(&only)));
    }
    if let Some(n) = std::env::var("LIMIT").ok().and_then(|n| n.parse().ok()) {
        files.truncate(n);
    }
    println!("{} files", files.len());
    for &n in &threads {
        let t = Instant::now();
        let done = match what.as_str() {
            "briefs" => pool(&files, n, |f| meta::brief(f).is_ok()),
            "meta" => pool(&files, n, |f| meta::read(f).is_ok()),
            "thumb" => {
                let edge: u32 = std::env::var("EDGE").ok().and_then(|e| e.parse().ok()).unwrap_or(512);
                pool(&files, n, move |f| preview::scaled(f, edge, 80).is_ok())
            }
            "stat" => pool(&files, n, |f| std::fs::metadata(f).is_ok()),
            // What the app does, every file read whole (`n` is ignored).
            "app" => {
                let state = sift_lib::state::AppState::new(std::env::temp_dir().join("sift-bench-cache"), std::env::temp_dir().join("sift-bench-data"));
                state.allow(&dir);
                let paths: Vec<String> = files.iter().map(|f| f.to_string_lossy().into_owned()).collect();
                sift_lib::briefs::briefs_of(&state, &paths, true).into_iter().map(|_| Some(true)).collect()
            }
            "parts" => {
                let mut sums = [0f64; 5];
                for f in &files {
                    let t = Instant::now();
                    let src = sift_core::io::Src::open(f).unwrap();
                    let p = sift_core::container::parse_metadata(&src);
                    sums[0] += t.elapsed().as_secs_f64();
                    let t = Instant::now();
                    let _ = sift_core::rating::read(&src, &p, f);
                    sums[1] += t.elapsed().as_secs_f64();
                    let t = Instant::now();
                    let _ = sift_core::exif::read(&p);
                    sums[2] += t.elapsed().as_secs_f64();
                    let t = Instant::now();
                    let _ = sift_core::hdr::of(&src, &p);
                    sums[3] += t.elapsed().as_secs_f64();
                    let t = Instant::now();
                    let _ = sift_core::stack::hint(&p, f);
                    sums[4] += t.elapsed().as_secs_f64();
                }
                println!("parse {:.1} rating {:.1} exif {:.1} hdr {:.1} stack {:.1} ms a file", sums[0] * 1e3 / files.len() as f64, sums[1] * 1e3 / files.len() as f64, sums[2] * 1e3 / files.len() as f64, sums[3] * 1e3 / files.len() as f64, sums[4] * 1e3 / files.len() as f64);
                vec![]
            }
            other => panic!("unknown {other}"),
        };
        let ok = done.iter().flatten().filter(|&&ok| ok).count();
        let s = t.elapsed().as_secs_f64();
        println!("{what} threads {n:>3}: {s:.2} s, {:.1} ms a file, {ok} ok", s * 1000.0 / files.len() as f64);
    }
}
