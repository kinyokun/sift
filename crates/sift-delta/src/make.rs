//! Writing deltas, for the release pipeline.

use crate::{sha256_hex, Entry, Manifest, Source, MANIFEST};
use std::collections::BTreeMap;
use std::fs;
use std::io::{self, Write};
use std::path::Path;

const LEVEL: i32 = 19;

enum Node {
    Dir(u32),
    Symlink(String),
    File(u32),
}

#[cfg(unix)]
fn mode(meta: &fs::Metadata) -> u32 {
    use std::os::unix::fs::PermissionsExt;
    meta.permissions().mode() & 0o7777
}

#[cfg(not(unix))]
fn mode(_meta: &fs::Metadata) -> u32 {
    0
}

/// Everything under `root`, by path relative to it, parents before children.
fn walk(root: &Path) -> io::Result<BTreeMap<String, Node>> {
    fn visit(root: &Path, rel: &str, into: &mut BTreeMap<String, Node>) -> io::Result<()> {
        let dir = if rel.is_empty() { root.to_path_buf() } else { crate::local(root, rel) };
        let mut names: Vec<_> = fs::read_dir(dir)?.collect::<Result<_, _>>()?;
        names.sort_by_key(|e| e.file_name());
        for entry in names {
            let name = entry.file_name().to_string_lossy().into_owned();
            let path = if rel.is_empty() { name } else { format!("{rel}/{name}") };
            let meta = entry.path().symlink_metadata()?;
            if meta.file_type().is_symlink() {
                let target = fs::read_link(entry.path())?.to_string_lossy().replace('\\', "/");
                into.insert(path, Node::Symlink(target));
            } else if meta.is_dir() {
                into.insert(path.clone(), Node::Dir(mode(&meta)));
                visit(root, &path, into)?;
            } else {
                into.insert(path, Node::File(mode(&meta)));
            }
        }
        Ok(())
    }
    let mut tree = BTreeMap::new();
    visit(root, "", &mut tree)?;
    Ok(tree)
}

/// zstd's window must reach back over the reference and what is written so far.
fn window_log(span: usize) -> u32 {
    (usize::BITS - span.max(1).leading_zeros()).clamp(20, super::WINDOW_LOG_MAX)
}

fn compress(data: &[u8], base: Option<&[u8]>) -> io::Result<Vec<u8>> {
    let mut out = Vec::new();
    let mut encoder = match base {
        Some(base) => zstd::stream::write::Encoder::with_ref_prefix(&mut out, LEVEL, base)?,
        None => zstd::stream::write::Encoder::new(&mut out, LEVEL)?,
    };
    encoder.include_checksum(true)?;
    encoder.set_pledged_src_size(Some(data.len() as u64))?;
    encoder.window_log(window_log(data.len() + base.map_or(0, <[u8]>::len)))?;
    encoder.long_distance_matching(true)?;
    let threads = std::thread::available_parallelism().map_or(1, |n| n.get() as u32);
    encoder.multithread(threads)?;
    encoder.write_all(data)?;
    encoder.finish()?;
    Ok(out)
}

/// The delta that turns the release laid out under `old` (version `from`)
/// into the one under `new` (version `version`).
pub fn make(old: &Path, new: &Path, from: &str, version: &str) -> io::Result<Vec<u8>> {
    let before = walk(old)?;
    let after = walk(new)?;
    let mut entries = Vec::new();
    let mut data: Vec<Vec<u8>> = Vec::new();
    for (path, node) in &after {
        entries.push(match node {
            Node::Dir(mode) => Entry::Dir { path: path.clone(), mode: *mode },
            Node::Symlink(target) => Entry::Symlink { path: path.clone(), target: target.clone() },
            Node::File(mode) => {
                let bytes = fs::read(crate::local(new, path))?;
                let base = match before.get(path) {
                    Some(Node::File(_)) => Some(fs::read(crate::local(old, path))?),
                    _ => None,
                };
                let source = match base {
                    Some(base) if base == bytes => Source::Same,
                    Some(base) => {
                        let patch = compress(&bytes, Some(&base))?;
                        // A patch can lose to the file on its own when little is shared.
                        let whole = if patch.len() > bytes.len() / 4 { Some(compress(&bytes, None)?) } else { None };
                        match whole {
                            Some(whole) if whole.len() < patch.len() => {
                                data.push(whole);
                                Source::Whole { data: data.len() - 1 }
                            }
                            _ => {
                                data.push(patch);
                                Source::Patch { data: data.len() - 1, base: sha256_hex(&base) }
                            }
                        }
                    }
                    None => {
                        data.push(compress(&bytes, None)?);
                        Source::Whole { data: data.len() - 1 }
                    }
                };
                Entry::File { path: path.clone(), mode: *mode, size: bytes.len() as u64, sha256: sha256_hex(&bytes), source }
            }
        });
    }
    let removed = before
        .iter()
        .filter(|(path, node)| !matches!(node, Node::Dir(_)) && !after.contains_key(*path))
        .map(|(path, _)| path.clone())
        .collect();
    let manifest = Manifest { version: version.into(), from: from.into(), entries, removed };

    let mut out = tar::Builder::new(Vec::new());
    let mut add = |name: &str, content: &[u8]| {
        let mut header = tar::Header::new_gnu();
        header.set_size(content.len() as u64);
        header.set_mode(0o644);
        header.set_cksum();
        out.append_data(&mut header, name, content)
    };
    add(MANIFEST, &serde_json::to_vec_pretty(&manifest).map_err(io::Error::other)?)?;
    for (n, content) in data.iter().enumerate() {
        add(&format!("data/{n}"), content)?;
    }
    out.into_inner()
}
