use sift_core::animal;
use sift_core::face::Detector;
use std::collections::HashMap;
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::AtomicBool;
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
    pub cancel: Arc<AtomicBool>,
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
            cancel: Arc::new(AtomicBool::new(false)),
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

fn fnv1a(parts: &[&[u8]]) -> u64 {
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

/// Returns the cached bytes, or produces, stores and returns them.
pub fn cached(file: &Path, make: impl FnOnce() -> sift_core::Result<Vec<u8>>) -> sift_core::Result<Vec<u8>> {
    if let Ok(data) = fs::read(file) {
        if !data.is_empty() {
            return Ok(data);
        }
    }
    let data = make()?;
    if let Some(dir) = file.parent() {
        let _ = fs::create_dir_all(dir);
    }
    let tmp = file.with_extension(format!("tmp{}", std::process::id()));
    if fs::write(&tmp, &data).is_ok() && fs::rename(&tmp, file).is_err() {
        let _ = fs::remove_file(&tmp);
    }
    Ok(data)
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
