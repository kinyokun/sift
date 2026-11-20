//! sift-delta make <old dir> <new dir> <old version> <new version> <out>
//! sift-delta build <delta> <installed dir> <out dir>
//! sift-delta info <delta>
//! sift-delta route <updates.json> <os> <installed version>

use std::path::Path;
use std::process::ExitCode;

fn run(args: &[String]) -> std::io::Result<()> {
    match args {
        [cmd, old, new, from, version, out] if cmd == "make" => {
            let bytes = sift_delta::make(Path::new(old), Path::new(new), from, version)?;
            std::fs::write(out, &bytes)?;
            println!("{out}: {from} -> {version}, {} bytes", bytes.len());
        }
        [cmd, delta, root, out] if cmd == "build" => {
            let delta = sift_delta::Delta::parse(&std::fs::read(delta)?)?;
            delta.build(Path::new(root), Path::new(out))?;
            println!("{out}: {}", delta.manifest.version);
        }
        [cmd, delta] if cmd == "info" => {
            let delta = sift_delta::Delta::parse(&std::fs::read(delta)?)?;
            let m = &delta.manifest;
            let mut counts = [0usize; 3];
            for entry in &m.entries {
                if let sift_delta::Entry::File { source, .. } = entry {
                    counts[match source {
                        sift_delta::Source::Same => 0,
                        sift_delta::Source::Patch { .. } => 1,
                        sift_delta::Source::Whole { .. } => 2,
                    }] += 1;
                }
            }
            println!(
                "{} -> {}: {} entries, {} same, {} patched, {} whole, {} removed, {} bytes of data",
                m.from,
                m.version,
                m.entries.len(),
                counts[0],
                counts[1],
                counts[2],
                m.removed.len(),
                delta.data_len()
            );
        }
        [cmd, catalogue, os, current] if cmd == "route" => {
            let catalogue: sift_delta::catalogue::Catalogue = serde_json::from_slice(&std::fs::read(catalogue)?).map_err(std::io::Error::other)?;
            let hops = catalogue.deltas.get(os).map(Vec::as_slice).unwrap_or_default();
            let full = catalogue.full.get(os).copied();
            let path = sift_delta::catalogue::route(hops, current, &catalogue.version, full);
            if path.is_empty() {
                println!("{current} -> {}: full package, {} bytes", catalogue.version, full.unwrap_or_default());
            }
            for hop in &path {
                println!("{} -> {}: {} bytes, {}", hop.from, hop.to, hop.size, hop.url);
            }
        }
        _ => {
            eprintln!("usage: sift-delta make <old dir> <new dir> <old version> <new version> <out>");
            eprintln!("       sift-delta build <delta> <installed dir> <out dir>");
            eprintln!("       sift-delta info <delta>");
            eprintln!("       sift-delta route <updates.json> <os> <installed version>");
            std::process::exit(2);
        }
    }
    Ok(())
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match run(&args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("sift-delta: {e}");
            ExitCode::FAILURE
        }
    }
}
