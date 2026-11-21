use sift_core::animal;
use sift_core::face::Detector;
use std::collections::HashMap;
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock, RwLock};
use std::time::UNIX_EPOCH;

pub struct AppState {
    pub cache_dir: PathBuf,
    pub data_dir: PathBuf,
    /// Folders the user opened; nothing outside them is ever read or written.
    pub roots: RwLock<Vec<PathBuf>>,
    pub detector: OnceLock<Option<Detector>>,
    /// Loaded the first time a photo turns out to have no face in it.
    pub animals: OnceLock<Option<animal::Detector>>,
    pub eyes: Mutex<HashMap<String, Option<crate::commands::EyeTarget>>>,
    /// What each file read so far says about its burst or pixel-shift series.
    pub series: Mutex<HashMap<String, sift_core::stack::Hint>>,
    /// Briefs read before, by folder.
    pub briefs: crate::briefs::Kept,
    pub cancel: Arc<AtomicBool>,
    /// Counts the folders thumbnails were made ahead for; a run for an earlier one stops.
    pub warming: std::sync::atomic::AtomicU64,
    /// Bounds concurrent image decodes so a fast scroll cannot saturate the CPU.
    pub image_slots: Arc<tokio::sync::Semaphore>,
    pub transfer: Mutex<Option<crate::transfer::Handle>>,
    /// Imported looks, loaded on first use.
    pub looks: Mutex<HashMap<String, Arc<sift_core::look::Look>>>,
    /// Nikon's own NEF renderer, when the user has installed the SDK.
    pub nikon: RwLock<Option<Arc<sift_core::nikon::Engine>>>,
}

impl AppState {
    pub fn new(cache_dir: PathBuf, data_dir: PathBuf) -> AppState {
        let workers = std::thread::available_parallelism().map_or(4, |n| n.get()).clamp(2, 8);
        // Versions up to 0.1 kept a copy of Nikon's SDK here; it is part of the app now.
        let _ = std::fs::remove_dir_all(data_dir.join("nikon-sdk"));
        AppState {
            cache_dir,
            data_dir,
            roots: RwLock::new(Vec::new()),
            detector: OnceLock::new(),
            animals: OnceLock::new(),
            eyes: Mutex::new(HashMap::new()),
            series: Mutex::new(HashMap::new()),
            briefs: Default::default(),
            cancel: Arc::new(AtomicBool::new(false)),
            warming: Default::default(),
            image_slots: Arc::new(tokio::sync::Semaphore::new(workers)),
            transfer: Mutex::new(None),
            looks: Mutex::new(HashMap::new()),
            nikon: RwLock::new(None),
        }
    }

    /// Nikon's renderer, if this build ships it.
    pub fn nikon(&self) -> Option<Arc<sift_core::nikon::Engine>> {
        use sift_core::nikon::Engine;
        if let Some(engine) = self.nikon.read().unwrap().as_ref() {
            return Some(engine.clone());
        }
        let engine = Arc::new(Engine::new(Engine::bundled()?));
        *self.nikon.write().unwrap() = Some(engine.clone());
        Some(engine)
    }

    /// Whether `path` would be developed by Nikon's renderer.
    pub fn nikon_for(&self, path: &Path) -> Option<Arc<sift_core::nikon::Engine>> {
        sift_core::nikon::handles(path).then(|| self.nikon()).flatten()
    }

    pub fn looks_dir(&self) -> PathBuf {
        self.data_dir.join("looks")
    }

    /// A look from the library by id (its file name).
    pub fn look(&self, id: &str) -> Result<Arc<sift_core::look::Look>, String> {
        if let Some(hit) = self.looks.lock().unwrap().get(id) {
            return Ok(hit.clone());
        }
        let clean = Path::new(id).file_name().is_some_and(|n| n == id);
        if !clean {
            return Err(format!("unknown look: {id}"));
        }
        let file = self.looks_dir().join(id);
        if !file.is_file() {
            return Err(format!("look is no longer in the library: {id}"));
        }
        let look = Arc::new(sift_core::look::load(&file).map_err(|e| e.to_string())?);
        self.looks.lock().unwrap().insert(id.to_string(), look.clone());
        Ok(look)
    }

    pub fn allow(&self, root: &Path) {
        let mut roots = self.roots.write().unwrap();
        if !roots.iter().any(|r| r == root) {
            roots.push(root.to_path_buf());
        }
    }

    /// Returns `path` if it lies inside an opened folder.
    pub fn check(&self, path: &str) -> Result<PathBuf, String> {
        let p = PathBuf::from(path);
        let clean = p.is_absolute() && !p.components().any(|c| matches!(c, Component::ParentDir));
        if clean && self.roots.read().unwrap().iter().any(|r| p.starts_with(r)) {
            Ok(p)
        } else {
            Err(format!("not inside an opened folder: {path}"))
        }
    }

    pub fn detector(&self) -> Option<&Detector> {
        self.detector.get_or_init(|| Detector::new().ok()).as_ref()
    }

    pub fn animals(&self) -> Option<&animal::Detector> {
        self.animals.get_or_init(|| animal::Detector::new().ok()).as_ref()
    }
}

pub fn fnv1a(parts: &[&[u8]]) -> u64 {
    let mut h = 0xcbf29ce484222325u64;
    for part in parts {
        for &b in *part {
            h = (h ^ b as u64).wrapping_mul(0x100000001b3);
        }
        h = (h ^ 0xff).wrapping_mul(0x100000001b3);
    }
    h
}

pub fn path_hash(path: &Path) -> String {
    format!("{:016x}", fnv1a(&[path.to_string_lossy().as_bytes()]))
}

/// Cache location for a rendition of `path`. The key covers size and
/// modification time, so an edited file never serves a stale image.
pub fn cache_file(cache_dir: &Path, kind: &str, path: &Path, edge: u32) -> PathBuf {
    let (len, mtime) = fs::metadata(path)
        .map(|m| {
            let t = m.modified().ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map_or(0, |d| d.as_nanos());
            (m.len(), t)
        })
        .unwrap_or((0, 0));
    let h = fnv1a(&[path.to_string_lossy().as_bytes(), &len.to_le_bytes(), &mtime.to_le_bytes(), &edge.to_le_bytes()]);
    cache_dir.join(kind).join(format!("{:02x}", h & 0xff)).join(format!("{h:016x}.jpg"))
}

/// Returns the cached bytes, or produces, stores and returns them. Everything
/// cached is a JPEG; one cut short (the app stopped while it was written, or
/// the disk lost it) is made again.
pub fn cached(file: &Path, make: impl FnOnce() -> sift_core::Result<Vec<u8>>) -> sift_core::Result<Vec<u8>> {
    if let Ok(data) = fs::read(file) {
        if whole_jpeg(&data) {
            return Ok(data);
        }
    }
    let data = make()?;
    put_whole(file, &data);
    Ok(data)
}

/// A JPEG from its start to its end marker; previews taken from RAW files may
/// be padded with zeros after it.
fn whole_jpeg(data: &[u8]) -> bool {
    let end = data.iter().rposition(|&b| b != 0).map_or(0, |i| i + 1);
    data.starts_with(&[0xFF, 0xD8]) && data[..end].ends_with(&[0xFF, 0xD9])
}

/// Writes `data` to `file` whole or not at all. Each write goes through a
/// temporary file of its own: the grid and the thumbnails made ahead can make
/// the same entry at once, and a shared one would mix their bytes.
pub fn put_whole(file: &Path, data: &[u8]) {
    static WRITES: AtomicU64 = AtomicU64::new(0);
    if let Some(dir) = file.parent() {
        let _ = fs::create_dir_all(dir);
    }
    let n = WRITES.fetch_add(1, Ordering::Relaxed);
    let tmp = file.with_extension(format!("tmp{}-{n}", std::process::id()));
    if fs::write(&tmp, data).is_err() || fs::rename(&tmp, file).is_err() {
        let _ = fs::remove_file(&tmp);
    }
}

pub fn dir_size(dir: &Path) -> u64 {
    fs::read_dir(dir).map_or(0, |rd| {
        rd.flatten()
            .map(|e| match e.metadata() {
                Ok(m) if m.is_dir() => dir_size(&e.path()),
                Ok(m) => m.len(),
                Err(_) => 0,
            })
            .sum()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn jpeg(len: usize) -> Vec<u8> {
        let mut data = vec![0xFF, 0xD8];
        data.resize(len - 2, 0x55);
        data.extend([0xFF, 0xD9]);
        data
    }

    #[test]
    fn a_jpeg_cut_short_is_made_again() {
        let dir = std::env::temp_dir().join(format!("sift-cached-{}", std::process::id()));
        let file = dir.join("a.jpg");
        let whole = jpeg(1000);
        put_whole(&file, &whole[..600]);
        assert_eq!(cached(&file, || Ok(whole.clone())).unwrap(), whole);
        assert_eq!(fs::read(&file).unwrap(), whole);
        // What is whole is taken as it is, padding after the end marker too.
        let padded = [whole.clone(), vec![0; 64]].concat();
        put_whole(&file, &padded);
        assert_eq!(cached(&file, || panic!("made again")).unwrap(), padded);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn entries_made_at_once_are_not_mixed() {
        let dir = std::env::temp_dir().join(format!("sift-cached-mixed-{}", std::process::id()));
        let file = dir.join("b.jpg");
        let (long, short) = (jpeg(4 << 20), jpeg(1 << 10));
        for _ in 0..20 {
            let _ = fs::remove_file(&file);
            std::thread::scope(|s| {
                for k in 0..8 {
                    let (file, data) = (&file, if k % 2 == 0 { &long } else { &short });
                    s.spawn(move || put_whole(file, data));
                }
            });
            let got = fs::read(&file).unwrap();
            assert!(got == long || got == short, "{} bytes, neither entry", got.len());
        }
        assert_eq!(fs::read_dir(&dir).unwrap().count(), 1, "temporary files left behind");
        let _ = fs::remove_dir_all(&dir);
    }
}
