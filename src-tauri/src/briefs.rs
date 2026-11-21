//! What the grid sorts, filters and stacks by ([`meta::Brief`]), for many
//! files at once. On a network share reading a file costs round trips, not
//! work, so files are read by many threads at once; and what was read is
//! kept on disk per folder, so a folder opened again is laid out at once,
//! and then only the stars are read again, from where they were found.

use crate::state::{path_hash, AppState};
use serde::{Deserialize, Serialize};
use sift_core::hdr::Hdr;
use sift_core::io::Src;
use sift_core::{meta, nksc, rating, stack};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;
use std::time::UNIX_EPOCH;

/// Files read at once. Each read waits on the disk or the network far longer
/// than it computes, so this is many more than there are cores: on a share
/// over Wi-Fi 8 threads took 10.4 s for 1,600 photos, 32 took 5.0 s, 64 3.7 s.
pub const READERS: usize = 64;

/// Bump when what is kept changes, so what was kept earlier is read again.
const KIND: &str = "briefs1";

/// Size and modification time (nanoseconds) of a file.
type Stamp = (u64, u64);

fn stamp(m: &fs::Metadata) -> Stamp {
    let t = m.modified().ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map_or(0, |d| d.as_nanos() as u64);
    (m.len(), t)
}

/// A brief as kept. Stars are written keeping the file's time and often its
/// size, so they are read again from `places` every time; `rating` is what
/// they were then, to lay the folder out by until that is done.
#[derive(Serialize, Deserialize, Clone)]
struct Entry {
    file: Stamp,
    rating: u8,
    date: Option<String>,
    hdr: Option<Hdr>,
    series: stack::Hint,
    places: rating::Places,
}

impl Entry {
    fn brief(&self) -> meta::Brief {
        meta::Brief { rating: self.rating, date: self.date.clone(), hdr: self.hdr, series: self.series.clone(), places: self.places }
    }
}

/// Briefs kept by folder, then by file name; loaded from disk the first time a folder is asked about.
#[derive(Default)]
pub struct Kept(Mutex<HashMap<PathBuf, HashMap<String, Entry>>>);

impl Kept {
    pub fn clear(&self) {
        self.0.lock().unwrap().clear();
    }
}

fn kept_file(cache_dir: &Path, dir: &Path) -> PathBuf {
    cache_dir.join(KIND).join(format!("{}.json", path_hash(dir)))
}

/// What sits beside the photos of one folder: Nikon's sidecars and `.xmp` files, from one listing each.
struct Beside {
    nksc: Option<nksc::Sidecars>,
    /// `.xmp` files by name.
    xmp: HashMap<String, PathBuf>,
}

impl Beside {
    fn of(dir: &Path) -> Beside {
        let xmp = fs::read_dir(dir)
            .into_iter()
            .flatten()
            .flatten()
            .filter(|e| Path::new(&e.file_name()).extension().is_some_and(|x| x.eq_ignore_ascii_case("xmp")))
            .map(|e| (e.file_name().to_string_lossy().into_owned(), e.path()))
            .collect();
        Beside { nksc: nksc::Sidecars::of(dir), xmp }
    }

    /// The `.xmp` sidecar of `photo`, as `rating::sidecar_path` finds it.
    fn sidecar(&self, photo: &Path) -> Option<&Path> {
        let stem = photo.file_stem()?.to_string_lossy();
        ["xmp", "XMP"].iter().find_map(|ext| self.xmp.get(&format!("{stem}.{ext}"))).map(PathBuf::as_path)
    }
}

/// Runs `f` on every item on up to `threads` threads, each taking the next
/// item when it is done, so a slow file holds up no others. Results in input order.
pub fn each<I: Sync, T: Send + Default>(items: &[I], threads: usize, f: impl Fn(&I) -> T + Sync) -> Vec<T> {
    let next = AtomicUsize::new(0);
    let done = Mutex::new(Vec::with_capacity(items.len()));
    std::thread::scope(|s| {
        for _ in 0..threads.clamp(1, items.len().max(1)) {
            s.spawn(|| loop {
                let i = next.fetch_add(1, Ordering::Relaxed);
                let Some(item) = items.get(i) else { break };
                let out = f(item);
                done.lock().unwrap().push((i, out));
            });
        }
    });
    let mut out: Vec<T> = (0..items.len()).map(|_| T::default()).collect();
    for (i, v) in done.into_inner().unwrap() {
        out[i] = v;
    }
    out
}

/// The folders `files` are in, each once.
fn dirs_of(files: &[Option<PathBuf>]) -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = files.iter().flatten().filter_map(|f| f.parent().map(Path::to_path_buf)).collect();
    dirs.sort();
    dirs.dedup();
    dirs
}

/// What is kept about the files of `dirs`, by folder.
fn known_of(state: &AppState, dirs: &[PathBuf]) -> HashMap<PathBuf, HashMap<String, Entry>> {
    let mut kept = state.briefs.0.lock().unwrap();
    dirs.iter()
        .map(|dir| {
            let folder = kept.entry(dir.clone()).or_insert_with(|| fs::read(kept_file(&state.cache_dir, dir)).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default());
            (dir.clone(), folder.clone())
        })
        .collect()
}

fn split(f: &Path) -> Option<(&Path, String)> {
    Some((f.parent()?, f.file_name()?.to_string_lossy().into_owned()))
}

/// The series each file belongs to is kept for `stacks_of`, which needs the whole folder at once.
fn note_series<'a>(state: &AppState, read: impl Iterator<Item = (&'a String, &'a meta::Brief)>) {
    let mut series = state.series.lock().unwrap();
    for (path, brief) in read {
        series.insert(path.clone(), brief.series.clone());
    }
}

/// Briefs as they were read before, of the files that have not changed
/// since, without opening any: what a folder opened again is laid out by
/// at once, before [`briefs_of`] reads the stars again. `None` for a file
/// that has to be read.
pub fn kept_of(state: &AppState, paths: &[String]) -> Vec<Option<meta::Brief>> {
    let files: Vec<Option<PathBuf>> = paths.iter().map(|p| state.check(p).ok()).collect();
    let known = known_of(state, &dirs_of(&files));
    let out = each(&files, READERS, |f| {
        let f = f.as_ref()?;
        let (dir, name) = split(f)?;
        let e = known.get(dir)?.get(&name)?;
        (stamp(&fs::metadata(f).ok()?) == e.file).then(|| e.brief())
    });
    note_series(state, paths.iter().zip(&out).filter_map(|(p, b)| Some((p, b.as_ref()?))));
    out
}

/// Rating and capture time for many files at once, in input order. `fresh`
/// reads every file whole again, as a refresh that was asked for does.
pub fn briefs_of(state: &AppState, paths: &[String], fresh: bool) -> Vec<meta::Brief> {
    let files: Vec<Option<PathBuf>> = paths.iter().map(|p| state.check(p).ok()).collect();
    let dirs = dirs_of(&files);
    let beside: HashMap<&Path, Beside> = dirs.iter().map(PathBuf::as_path).zip(each(&dirs, READERS, |d| Some(Beside::of(d)))).filter_map(|(d, b)| Some((d, b?))).collect();
    let known = known_of(state, &dirs);
    let read = each(&files, READERS, |f| -> Option<(meta::Brief, Option<Entry>)> {
        let f = f.as_ref()?;
        let (dir, name) = split(f)?;
        let around = beside.get(dir)?;
        // The size and time come with opening the file, where asking for them costs a round trip of their own.
        let open = fs::File::open(f).ok()?;
        let file = stamp(&open.metadata().ok()?);
        let src = Src::file(open, file.0);
        let sidecar = around.sidecar(f);
        let was = known.get(dir).and_then(|k| k.get(&name)).filter(|e| !fresh && e.file == file);
        let again = was.and_then(|e| Some((e, rating::read_at(&src, &e.places, sidecar)?)));
        let mut brief = match again {
            Some((e, rating)) => meta::Brief { rating, ..e.brief() },
            None => meta::brief_beside(&src, f, sidecar),
        };
        // Stars given in Nikon's software are only in its sidecar, so they are the newer word.
        if let Some(given) = around.nksc.as_ref().and_then(|n| n.rating(f)) {
            brief.rating = given;
        }
        let entry = match again {
            Some((e, _)) if e.rating == brief.rating => None,
            _ => Some(Entry { file, rating: brief.rating, date: brief.date.clone(), hdr: brief.hdr, series: brief.series.clone(), places: brief.places }),
        };
        Some((brief, entry))
    });
    let mut out = Vec::with_capacity(read.len());
    let mut changed: HashMap<PathBuf, Vec<(String, Entry)>> = HashMap::new();
    for (f, r) in files.iter().zip(read) {
        let (brief, entry) = r.unwrap_or_default();
        if let (Some((dir, name)), Some(e)) = (f.as_deref().and_then(split), entry) {
            changed.entry(dir.to_path_buf()).or_default().push((name, e));
        }
        out.push(brief);
    }
    if !changed.is_empty() {
        let mut kept = state.briefs.0.lock().unwrap();
        for (dir, entries) in changed {
            let folder = kept.entry(dir.clone()).or_default();
            folder.extend(entries);
            store(&kept_file(&state.cache_dir, &dir), folder);
        }
    }
    note_series(state, paths.iter().zip(&out));
    out
}

fn store(file: &Path, folder: &HashMap<String, Entry>) {
    let Ok(data) = serde_json::to_vec(folder) else { return };
    crate::state::put_whole(file, &data);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn brief(state: &AppState, file: &Path, fresh: bool) -> meta::Brief {
        briefs_of(state, &[file.to_string_lossy().into_owned()], fresh).remove(0)
    }

    #[test]
    fn what_is_kept_is_read_again_only_for_the_stars() {
        let root = std::env::temp_dir().join(format!("sift-briefs-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let (dir, cache) = (root.join("photos"), root.join("cache"));
        fs::create_dir_all(&dir).unwrap();
        let jpg = dir.join("a.jpg");
        let mut data = Vec::new();
        image::codecs::jpeg::JpegEncoder::new(&mut data).encode(&[128u8; 16 * 16 * 3], 16, 16, image::ExtendedColorType::Rgb8).unwrap();
        fs::write(&jpg, data).unwrap();
        let heic = dir.join("b.heic");
        fs::copy(Path::new(env!("CARGO_MANIFEST_DIR")).join("../crates/sift-core/assets/pq.heic"), &heic).unwrap();
        rating::write(&jpg, 2).unwrap();
        let opened = || {
            let state = AppState::new(cache.clone(), root.join("data"));
            state.allow(&dir);
            state
        };

        let state = opened();
        assert_eq!((brief(&state, &jpg, false).rating, brief(&state, &heic, false).hdr), (2, Some(Hdr::Pq)));
        let kept = kept_file(&cache, &dir);
        assert!(kept.is_file());

        // What is kept is taken as it is: here it says something the file does not.
        let text = fs::read_to_string(&kept).unwrap().replace("\"pq\"", "\"hlg\"");
        fs::write(&kept, text).unwrap();
        // Stars written in place keep the file's size and time, and are still seen.
        let size = fs::metadata(&jpg).unwrap().len();
        rating::write(&jpg, 5).unwrap();
        rating::write(&heic, 3).unwrap();
        assert_eq!(fs::metadata(&jpg).unwrap().len(), size);
        let state = opened();
        let (a, b) = (brief(&state, &jpg, false), brief(&state, &heic, false));
        assert_eq!((a.rating, b.rating, b.hdr), (5, 3, Some(Hdr::Hlg)));
        let state = opened();
        let paths = [&jpg, &heic, &dir.join("c.jpg")].map(|p| p.to_string_lossy().into_owned());
        let kept: Vec<_> = kept_of(&state, &paths).into_iter().map(|b| b.map(|b| (b.rating, b.hdr))).collect();
        assert_eq!(kept, [Some((5, None)), Some((3, Some(Hdr::Hlg))), None], "as last read, without opening the files");
        assert_eq!(brief(&state, &heic, true).hdr, Some(Hdr::Pq), "a refresh that was asked for reads the file whole");
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn each_keeps_the_order() {
        let items: Vec<u64> = (0..500).collect();
        let out = each(&items, 16, |&i| {
            std::thread::sleep(std::time::Duration::from_micros((500 - i) % 7 * 50));
            i * 2
        });
        assert_eq!(out, items.iter().map(|i| i * 2).collect::<Vec<_>>());
    }
}
