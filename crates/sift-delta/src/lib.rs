//! Updates that carry only what changed between two releases.
//!
//! A delta turns one installed release into the next. Its manifest lists every
//! file of the new release with its SHA-256; the files that differ come as zstd
//! frames made with the installed file as their reference prefix (as
//! `zstd --patch-from` does), and new ones whole. The delta is a tar of
//! `manifest.json` and a `data/<n>` for each of those; whoever reads one checks
//! its signature first.
//!
//! [`Delta::build`] writes the whole new release beside the installed one, and
//! deltas can follow one another, each built on what the last one made, so a
//! release several versions on is reached with a few small downloads. Every
//! file is checked against the manifests on the way, before the installed
//! release is touched. Then either the new folder takes the place of the old
//! one in a single step (a macOS app, see [`swap`]), or [`Staged`] moves in
//! just the files that differ, setting aside the ones they replace, which a
//! running program may still be using (Windows).
//!
//! Which deltas there are is told by a [`catalogue::Catalogue`].

pub mod catalogue;
#[cfg(feature = "make")]
mod make;
#[cfg(feature = "make")]
pub use make::make;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::fs;
use std::io::{self, Read, Write};
use std::path::{Component, Path, PathBuf};

pub const MANIFEST: &str = "manifest.json";
/// Where [`Staged::prepare`] records what [`Staged::commit`] has to do.
const STAGED: &str = "staged.json";
/// Added to the name of a file [`Staged::commit`] moves out of the way.
pub const OLD_SUFFIX: &str = ".sift-old";
/// Decoders accept windows this large (2 GiB), as far as zstd's long mode goes.
const WINDOW_LOG_MAX: u32 = 31;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Manifest {
    /// The release the delta makes.
    pub version: String,
    /// The release it applies to.
    pub from: String,
    pub entries: Vec<Entry>,
    /// Files and links of `from` that `version` no longer has.
    #[serde(default)]
    pub removed: Vec<String>,
}

/// One item of the new release; paths are relative, with `/` between parts.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Entry {
    Dir { path: String, mode: u32 },
    Symlink { path: String, target: String },
    File { path: String, mode: u32, size: u64, sha256: String, source: Source },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "from", rename_all = "lowercase")]
pub enum Source {
    /// The installed file, unchanged.
    Same,
    /// `data/<data>` is made against the installed file, whose SHA-256 is `base`.
    Patch { data: usize, base: String },
    /// `data/<data>` is the file itself, compressed.
    Whole { data: usize },
}

pub struct Delta {
    pub manifest: Manifest,
    data: HashMap<usize, Vec<u8>>,
}

fn bad(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.into())
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes).iter().map(|b| format!("{b:02x}")).collect()
}

/// `rel` under `root`; `rel` has been checked by [`checked`].
fn local(root: &Path, rel: &str) -> PathBuf {
    rel.split('/').fold(root.to_path_buf(), |path, part| path.join(part))
}

/// A path that stays inside the folder it is relative to.
fn checked(rel: &str) -> io::Result<&str> {
    let parts_ok = !rel.is_empty()
        && !rel.contains('\\')
        && rel.split('/').all(|part| !part.is_empty() && part != "." && part != "..")
        && Path::new(rel).components().all(|c| matches!(c, Component::Normal(_)));
    if parts_ok {
        Ok(rel)
    } else {
        Err(bad(format!("unsafe path in delta: {rel:?}")))
    }
}

#[cfg(unix)]
fn set_mode(path: &Path, mode: u32) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(mode & 0o7777))
}

#[cfg(not(unix))]
fn set_mode(_path: &Path, _mode: u32) -> io::Result<()> {
    Ok(())
}

#[cfg(unix)]
fn symlink(target: &str, link: &Path) -> io::Result<()> {
    std::os::unix::fs::symlink(target, link)
}

#[cfg(not(unix))]
fn symlink(_target: &str, link: &Path) -> io::Result<()> {
    Err(io::Error::new(io::ErrorKind::Unsupported, format!("cannot make the link {}", link.display())))
}

fn parent_dirs(path: &Path) -> io::Result<()> {
    match path.parent() {
        Some(parent) => fs::create_dir_all(parent),
        None => Ok(()),
    }
}

impl Delta {
    /// Reads a delta whose signature has been checked.
    pub fn parse(bytes: &[u8]) -> io::Result<Self> {
        let mut manifest: Option<Manifest> = None;
        let mut data = HashMap::new();
        for entry in tar::Archive::new(bytes).entries()? {
            let mut entry = entry?;
            let name = entry.path()?.to_string_lossy().into_owned();
            let mut content = Vec::with_capacity(entry.size() as usize);
            entry.read_to_end(&mut content)?;
            if name == MANIFEST {
                manifest = Some(serde_json::from_slice(&content).map_err(|e| bad(format!("bad manifest: {e}")))?);
            } else if let Some(n) = name.strip_prefix("data/").and_then(|n| n.parse().ok()) {
                data.insert(n, content);
            } else {
                return Err(bad(format!("unexpected {name:?} in delta")));
            }
        }
        let manifest = manifest.ok_or_else(|| bad("delta has no manifest"))?;
        for entry in &manifest.entries {
            match entry {
                Entry::Dir { path, .. } => {
                    checked(path)?;
                }
                Entry::Symlink { path, target } => {
                    checked(path)?;
                    if target.is_empty() || Path::new(target).is_absolute() || target.starts_with('/') {
                        return Err(bad(format!("unsafe link in delta: {path} -> {target}")));
                    }
                }
                Entry::File { path, source, .. } => {
                    checked(path)?;
                    if let Source::Patch { data: n, .. } | Source::Whole { data: n } = source {
                        if !data.contains_key(n) {
                            return Err(bad(format!("delta lacks the data of {path}")));
                        }
                    }
                }
            }
        }
        for path in &manifest.removed {
            checked(path)?;
        }
        Ok(Self { manifest, data })
    }

    /// Bytes this delta downloads beyond its manifest; for reports.
    pub fn data_len(&self) -> u64 {
        self.data.values().map(|d| d.len() as u64).sum()
    }

    /// Makes the new release's file at `path` from the one installed under
    /// `root`, checking it as it goes, and writes it to `dest` (or only checks
    /// it). A file that does not come out right is removed again.
    fn make_file(&self, root: &Path, path: &str, size: u64, sha256: &str, source: &Source, dest: Option<&Path>) -> io::Result<()> {
        let mut out = Hashing { inner: match dest {
            Some(dest) => Box::new(io::BufWriter::new(fs::File::create(dest)?)) as Box<dyn Write>,
            None => Box::new(io::sink()),
        }, hash: Sha256::new(), len: 0 };
        let made = (|| -> io::Result<()> {
            match source {
                Source::Same => {
                    io::copy(&mut fs::File::open(local(root, path))?, &mut out)?;
                }
                Source::Patch { data, base } => {
                    let installed = fs::read(local(root, path))?;
                    if sha256_hex(&installed) != *base {
                        return Err(bad(format!("{path} is not the one of {}", self.manifest.from)));
                    }
                    let mut decoder = zstd::stream::read::Decoder::with_ref_prefix(self.data[data].as_slice(), &installed)?;
                    decoder.window_log_max(WINDOW_LOG_MAX)?;
                    io::copy(&mut decoder.take(size + 1), &mut out)?;
                }
                Source::Whole { data } => {
                    let mut decoder = zstd::stream::read::Decoder::with_buffer(self.data[data].as_slice())?;
                    decoder.window_log_max(WINDOW_LOG_MAX)?;
                    io::copy(&mut decoder.take(size + 1), &mut out)?;
                }
            }
            out.inner.flush()?;
            let digest: String = out.hash.clone().finalize().iter().map(|b| format!("{b:02x}")).collect();
            if out.len != size || digest != sha256 {
                return Err(bad(format!("{path} does not come out as in {}", self.manifest.version)));
            }
            Ok(())
        })();
        drop(out);
        if made.is_err() {
            if let Some(dest) = dest {
                let _ = fs::remove_file(dest);
            }
        }
        made
    }
    /// Writes the whole new release to `out`, which must not exist yet, from
    /// the release installed at `root`.
    pub fn build(&self, root: &Path, out: &Path) -> io::Result<()> {
        fs::create_dir(out)?;
        let mut dirs = Vec::new();
        for entry in &self.manifest.entries {
            match entry {
                Entry::Dir { path, mode } => {
                    let dir = local(out, path);
                    fs::create_dir_all(&dir)?;
                    dirs.push((dir, *mode));
                }
                Entry::Symlink { path, target } => {
                    let link = local(out, path);
                    parent_dirs(&link)?;
                    symlink(target, &link)?;
                }
                Entry::File { path, mode, size, sha256, source } => {
                    let file = local(out, path);
                    parent_dirs(&file)?;
                    self.make_file(root, path, *size, sha256, source, Some(&file))?;
                    set_mode(&file, *mode)?;
                }
            }
        }
        // Last, deepest first, in case a mode takes away the right to write.
        for (dir, mode) in dirs.iter().rev() {
            set_mode(dir, *mode)?;
        }
        set_mode(out, 0o755)
    }
}

/// Passes bytes on, counting and hashing them.
struct Hashing<W> {
    inner: W,
    hash: Sha256,
    len: u64,
}

impl<W: Write> Write for Hashing<W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let n = self.inner.write(buf)?;
        self.hash.update(&buf[..n]);
        self.len += n as u64;
        Ok(n)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

#[derive(Serialize, Deserialize)]
struct Record {
    version: String,
    changed: Vec<String>,
    removed: Vec<String>,
}

/// A release laid out by [`Delta::build`], waiting for the files that differ to be moved in.
#[derive(Debug)]
pub struct Staged {
    pub version: String,
    root: PathBuf,
    stage: PathBuf,
    changed: Vec<String>,
    removed: Vec<String>,
}

/// `path` with [`OLD_SUFFIX`] added, or a numbered variant when an earlier
/// one is still there and cannot be deleted (it may be running).
fn set_aside(path: &Path) -> PathBuf {
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    (0..)
        .map(|n| path.with_file_name(if n == 0 { format!("{name}{OLD_SUFFIX}") } else { format!("{name}.{n}{OLD_SUFFIX}") }))
        .find(|old| old.symlink_metadata().is_err() || fs::remove_file(old).is_ok())
        .expect("an unused name")
}

impl Staged {
    /// Compares the release laid out at `tree` (whose last delta had
    /// `manifest`) with the one installed at `root`: what differs is moved in
    /// by [`Self::commit`], and of `removed` (what the deltas on the way
    /// dropped) what `root` has and the new release does not is set aside.
    pub fn prepare(root: &Path, tree: &Path, manifest: &Manifest, removed: &[String]) -> io::Result<Self> {
        let mut changed = Vec::new();
        let mut kept = std::collections::HashSet::new();
        for entry in &manifest.entries {
            match entry {
                Entry::Dir { path, .. } => {
                    kept.insert(path.as_str());
                }
                Entry::Symlink { path, target } => {
                    kept.insert(path.as_str());
                    if fs::read_link(local(root, path)).ok().as_deref() != Some(Path::new(target)) {
                        changed.push(path.clone());
                    }
                }
                Entry::File { path, size, sha256, .. } => {
                    kept.insert(path.as_str());
                    let installed = local(root, path);
                    let same = installed.metadata().is_ok_and(|m| m.is_file() && m.len() == *size)
                        && fs::read(&installed).is_ok_and(|bytes| sha256_hex(&bytes) == *sha256);
                    if !same {
                        changed.push(path.clone());
                    }
                }
            }
        }
        let mut removed: Vec<String> = removed.iter().filter(|p| !kept.contains(p.as_str()) && local(root, p).symlink_metadata().is_ok()).cloned().collect();
        removed.sort();
        removed.dedup();
        let record = Record { version: manifest.version.clone(), changed, removed };
        fs::write(tree.join(STAGED), serde_json::to_vec(&record).map_err(io::Error::other)?)?;
        Ok(Self { version: record.version, root: root.to_path_buf(), stage: tree.to_path_buf(), changed: record.changed, removed: record.removed })
    }

    /// What an earlier [`Self::prepare`] left in `stage` for the release under `root`.
    pub fn load(root: &Path, stage: &Path) -> io::Result<Self> {
        let record: Record = serde_json::from_slice(&fs::read(stage.join(STAGED))?).map_err(|e| bad(e.to_string()))?;
        for path in record.changed.iter().chain(&record.removed) {
            checked(path)?;
        }
        Ok(Self { version: record.version, root: root.to_path_buf(), stage: stage.to_path_buf(), changed: record.changed, removed: record.removed })
    }

    /// Moves the staged files in. Each one replaced or removed is renamed with
    /// [`OLD_SUFFIX`] rather than deleted, which works while it is in use; on
    /// failure everything done is undone. [`sweep`] deletes them later.
    pub fn commit(&self) -> io::Result<()> {
        // (installed path, where its previous content went)
        let mut done: Vec<(PathBuf, Option<PathBuf>)> = Vec::new();
        let result = (|| -> io::Result<()> {
            for path in &self.changed {
                let target = local(&self.root, path);
                parent_dirs(&target)?;
                let old = match target.symlink_metadata() {
                    Ok(_) => {
                        let old = set_aside(&target);
                        fs::rename(&target, &old)?;
                        Some(old)
                    }
                    Err(_) => None,
                };
                done.push((target.clone(), old));
                fs::rename(local(&self.stage, path), &target)?;
            }
            for path in &self.removed {
                let target = local(&self.root, path);
                if target.symlink_metadata().is_ok() {
                    let old = set_aside(&target);
                    fs::rename(&target, &old)?;
                    done.push((target, Some(old)));
                }
            }
            Ok(())
        })();
        if let Err(e) = result {
            for (target, old) in done.into_iter().rev() {
                if target.symlink_metadata().is_ok() && old.as_ref().is_none_or(|old| old.symlink_metadata().is_ok()) {
                    let _ = fs::remove_file(&target);
                }
                if let Some(old) = old {
                    let _ = fs::rename(&old, &target);
                }
            }
            return Err(e);
        }
        let _ = fs::remove_dir_all(&self.stage);
        Ok(())
    }

    /// Gives up on the staged files.
    pub fn discard(self) {
        let _ = fs::remove_dir_all(&self.stage);
    }
}

/// Deletes what [`Staged::commit`] set aside under `root`, as far as nothing
/// still uses it.
pub fn sweep(root: &Path) {
    let Ok(dir) = fs::read_dir(root) else { return };
    for entry in dir.flatten() {
        let path = entry.path();
        let Ok(kind) = entry.file_type() else { continue };
        if kind.is_dir() {
            sweep(&path);
        } else if entry.file_name().to_string_lossy().ends_with(OLD_SUFFIX) {
            let _ = fs::remove_file(&path);
        }
    }
}

/// Exchanges two folders in one step where the file system can (APFS), and
/// with two renames otherwise, `a` never being left missing on failure.
pub fn swap(a: &Path, b: &Path) -> io::Result<()> {
    #[cfg(target_os = "macos")]
    {
        use std::ffi::CString;
        use std::os::unix::ffi::OsStrExt;
        let from = CString::new(a.as_os_str().as_bytes()).map_err(io::Error::other)?;
        let to = CString::new(b.as_os_str().as_bytes()).map_err(io::Error::other)?;
        // SAFETY: both are valid NUL-terminated paths that outlive the call.
        if unsafe { libc::renamex_np(from.as_ptr(), to.as_ptr(), libc::RENAME_SWAP) } == 0 {
            return Ok(());
        }
        let err = io::Error::last_os_error();
        if err.kind() == io::ErrorKind::PermissionDenied {
            return Err(err);
        }
    }
    let aside = b.with_file_name(format!("{}{OLD_SUFFIX}", b.file_name().map(|n| n.to_string_lossy()).unwrap_or_default()));
    fs::rename(a, &aside)?;
    if let Err(e) = fs::rename(b, a) {
        let _ = fs::rename(&aside, a);
        return Err(e);
    }
    fs::rename(&aside, b)
}

#[cfg(test)]
mod tests;
