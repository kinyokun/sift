//! Deleting photos: into the system's trash where there is one.
//!
//! A shot goes with what is only there because of it. The caller names its
//! files (both of a RAW+JPEG pair, a Live Photo's movie); the `.xmp` sidecar
//! goes too once no photo of that name is left next to it, and a Live Photo
//! bundle goes whole. The sidecars of Nikon's software (`nksc.rs`) stay where
//! they are: they hold the adjustments made there, which a photo taken back
//! out of the trash should still have.

use crate::{formats, live};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

/// Whether this platform has a trash that [`remove`] can move files into.
pub const TRASH: bool = cfg!(any(target_os = "macos", windows));

#[derive(serde::Serialize)]
pub struct Outcome {
    /// The shots, by their place in the list, that are still there, whole or in part.
    pub failed: Vec<usize>,
    /// Why the first of them could not be removed.
    pub reason: Option<String>,
}

/// A folder's files by name without the extension, in lower case.
fn names(dir: &Path) -> HashMap<String, Vec<PathBuf>> {
    let mut found: HashMap<String, Vec<PathBuf>> = HashMap::new();
    for path in fs::read_dir(dir).into_iter().flatten().flatten().map(|e| e.path()) {
        if let Some(stem) = path.file_stem() {
            found.entry(stem.to_string_lossy().to_lowercase()).or_default().push(path);
        }
    }
    found
}

/// What has to go for each shot to be gone; nothing is named twice.
fn targets(shots: &[Vec<PathBuf>]) -> Vec<Vec<PathBuf>> {
    let going: HashSet<&Path> = shots.iter().flatten().map(|p| p.as_path()).collect();
    // One listing per folder, however many of its photos go: a share answers each question slowly.
    let mut folders: HashMap<&Path, HashMap<String, Vec<PathBuf>>> = HashMap::new();
    let mut named: HashSet<PathBuf> = HashSet::new();
    shots
        .iter()
        .map(|shot| {
            let mut out = Vec::new();
            let mut add = |path: &Path| named.insert(path.to_path_buf()).then(|| out.push(path.to_path_buf()));
            for file in shot {
                let Some(dir) = file.parent() else { continue };
                if live::is_bundle(dir) {
                    add(dir);
                    continue;
                }
                add(file);
                let Some(stem) = file.file_stem().map(|s| s.to_string_lossy().to_lowercase()) else { continue };
                let alike = folders.entry(dir).or_insert_with(|| names(dir)).get(&stem).map_or(&[][..], |v| v.as_slice());
                // In a folder shown unpaired, the RAW of the same name keeps the sidecar.
                if alike.iter().any(|p| !going.contains(p.as_path()) && formats::is_supported(p)) {
                    continue;
                }
                for side in alike.iter().filter(|p| p.extension().is_some_and(|x| x.eq_ignore_ascii_case("xmp"))) {
                    add(side);
                }
            }
            out
        })
        .collect()
}

fn exists(path: &Path) -> bool {
    path.symlink_metadata().is_ok()
}

/// Only files go, and Live Photo bundles: never a folder of photos.
fn refused(path: &Path) -> bool {
    path.symlink_metadata().is_ok_and(|m| m.is_dir()) && !live::is_bundle(path)
}

fn erase(paths: &[PathBuf]) -> Vec<(PathBuf, String)> {
    let erase = |path: &Path| match if path.is_dir() { fs::remove_dir_all(path) } else { fs::remove_file(path) } {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e.to_string()),
        _ => Ok(()),
    };
    paths.iter().filter_map(|p| erase(p).err().map(|why| (p.clone(), why))).collect()
}

#[cfg(any(target_os = "macos", windows))]
fn describe(e: trash::Error) -> String {
    match e {
        // The system's own words follow the name of the call that failed.
        trash::Error::Unknown { description } | trash::Error::Os { description, .. } => description.rsplit_once("failed: ").map_or(description.as_str(), |(_, why)| why).to_string(),
        other => other.to_string(),
    }
}

#[cfg(target_os = "macos")]
fn trash(paths: &[PathBuf]) -> Vec<(PathBuf, String)> {
    use trash::macos::{DeleteMethod, TrashContextExtMacos};
    let mut context = trash::TrashContext::new();
    // The other way is to script Finder, which is slower and has macOS ask
    // whether the app may control it.
    context.set_delete_method(DeleteMethod::NsFileManager);
    paths.iter().filter(|p| exists(p)).filter_map(|p| context.delete(p).err().map(|e| (p.clone(), describe(e)))).collect()
}

#[cfg(windows)]
fn trash(paths: &[PathBuf]) -> Vec<(PathBuf, String)> {
    let present: Vec<&PathBuf> = paths.iter().filter(|p| exists(p)).collect();
    if present.is_empty() {
        return Vec::new();
    }
    // One operation for all of them, as Explorer does it. A file the shell
    // refuses (a drive without a Recycle Bin) fails the call and stays; the
    // others have gone all the same.
    match trash::delete_all(&present) {
        Ok(()) => Vec::new(),
        Err(e) => {
            let why = describe(e);
            present.into_iter().filter(|p| exists(p)).map(|p| (p.clone(), why.clone())).collect()
        }
    }
}

#[cfg(not(any(target_os = "macos", windows)))]
fn trash(paths: &[PathBuf]) -> Vec<(PathBuf, String)> {
    paths.iter().map(|p| (p.clone(), "there is no trash here".to_string())).collect()
}

/// Deletes shots, each given as its files: into the trash, or with
/// `for_good` at once and without a way back. A file that is already gone
/// counts as removed. Nothing is deleted for good unless asked: what the
/// trash does not take (a network share has none) stays and is reported.
pub fn remove(shots: &[Vec<PathBuf>], for_good: bool) -> Outcome {
    let plan = targets(shots);
    let (kept, all): (Vec<PathBuf>, Vec<PathBuf>) = plan.iter().flatten().cloned().partition(|p| refused(p));
    let mut failures: Vec<(PathBuf, String)> = kept.into_iter().map(|p| (p, "not a photo".to_string())).collect();
    failures.extend(if for_good { erase(&all) } else { trash(&all) });
    let stuck: HashSet<&Path> = failures.iter().map(|(p, _)| p.as_path()).collect();
    let failed: Vec<usize> = plan.iter().enumerate().filter(|(_, files)| files.iter().any(|p| stuck.contains(p.as_path()))).map(|(i, _)| i).collect();
    Outcome { failed, reason: failures.into_iter().next().map(|(_, why)| why) }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn folder(name: &str, files: &[&str]) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("sift-remove-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        for file in files {
            let path = dir.join(file);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, b"x").unwrap();
        }
        dir
    }

    fn left(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(dir).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
        names.sort();
        names
    }

    #[test]
    fn a_shot_takes_its_sidecar_unless_another_photo_has_the_name() {
        let dir = folder("sidecar", &["a.NEF", "a.JPG", "a.xmp", "b.NEF", "b.JPG", "b.XMP", "c.NEF", "c.xmp", "c.NEF.nksc", "d.JPG"]);
        let shot = |names: &[&str]| names.iter().map(|n| dir.join(n)).collect::<Vec<_>>();
        // The pair goes with its sidecar; of the unpaired b only the JPEG goes, and the RAW keeps its sidecar.
        let plan = targets(&[shot(&["a.JPG", "a.NEF"]), shot(&["b.JPG"]), shot(&["d.JPG"])]);
        assert_eq!(plan, [shot(&["a.JPG", "a.xmp", "a.NEF"]), shot(&["b.JPG"]), shot(&["d.JPG"])]);
        let done = remove(&[shot(&["a.JPG", "a.NEF"]), shot(&["b.JPG"]), shot(&["c.NEF"])], true);
        assert!(done.failed.is_empty() && done.reason.is_none());
        assert_eq!(left(&dir), ["b.NEF", "b.XMP", "c.NEF.nksc", "d.JPG"]);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_live_photo_bundle_goes_whole_and_a_folder_never() {
        let dir = folder("bundle", &["IMG_1.pvt/IMG_1.HEIC", "IMG_1.pvt/IMG_1.MOV", "IMG_1.pvt/metadata.plist", "IMG_2.JPG", "IMG_2.MOV", "day/IMG_3.JPG"]);
        let bundle = vec![dir.join("IMG_1.pvt/IMG_1.HEIC"), dir.join("IMG_1.pvt/IMG_1.MOV")];
        assert_eq!(targets(std::slice::from_ref(&bundle)), [[dir.join("IMG_1.pvt")]]);
        // The second shot names a folder, the third a file that is no longer there.
        let done = remove(&[bundle, vec![dir.join("day")], vec![dir.join("IMG_2.JPG"), dir.join("IMG_2.MOV")], vec![dir.join("gone.JPG")]], true);
        assert_eq!(done.failed, [1]);
        assert!(done.reason.is_some());
        assert_eq!(left(&dir), ["day"]);
        assert!(dir.join("day/IMG_3.JPG").is_file());
        fs::remove_dir_all(dir).unwrap();
    }

    /// Leaves two small files in the trash of whoever runs it, so on a Mac it
    /// is run by hand (`cargo test -p sift-core remove -- --ignored`).
    #[cfg(any(target_os = "macos", windows))]
    #[test]
    #[cfg_attr(target_os = "macos", ignore)]
    fn the_trash_takes_a_shot() {
        let dir = folder("trash", &["sift-trash-check.NEF", "sift-trash-check.JPG", "sift-trash-check.xmp", "kept.JPG"]);
        let done = remove(&[vec![dir.join("sift-trash-check.JPG"), dir.join("sift-trash-check.NEF")], vec![dir.join("gone.JPG")]], false);
        assert!(done.failed.is_empty(), "{:?}", done.reason);
        assert_eq!(left(&dir), ["kept.JPG"]);
        fs::remove_dir_all(dir).unwrap();
    }
}
