//! File operations after culling: copy the keepers out, and pull the RAW
//! files that belong to a set of selected JPEGs.

use crate::{formats, rating, Error, Result};
use serde::Serialize;
use std::collections::HashMap;
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

/// Large sequential blocks keep spinning disks from seeking between source and target.
const COPY_BLOCK: usize = 4 * 1024 * 1024;

#[derive(Serialize, Clone, Debug, Default)]
pub struct CopyReport {
    pub copied: usize,
    pub skipped: usize,
    pub failed: Vec<String>,
    /// Selected files for which no RAW was found (find-RAW only).
    pub missing: Vec<String>,
    pub cancelled: bool,
}

#[derive(Serialize, Clone, Debug)]
pub struct Progress {
    pub done: usize,
    pub total: usize,
    pub name: String,
}

/// Copies one file in large blocks, preserving its modification time. A
/// partially written target is removed on failure or cancellation.
pub fn copy_file(from: &Path, to: &Path, cancel: &AtomicBool) -> Result<()> {
    let result = (|| -> Result<()> {
        let mut src = fs::File::open(from)?;
        let mut dst = fs::File::create(to)?;
        let mut buf = vec![0u8; COPY_BLOCK];
        loop {
            if cancel.load(Ordering::Relaxed) {
                return Err(Error::Cancelled);
            }
            let n = src.read(&mut buf)?;
            if n == 0 {
                break;
            }
            dst.write_all(&buf[..n])?;
        }
        crate::io::sync(&dst)?;
        Ok(())
    })();
    match result {
        Ok(()) => {
            if let Ok(m) = fs::metadata(from) {
                let _ = filetime::set_file_mtime(to, filetime::FileTime::from_last_modification_time(&m));
            }
            Ok(())
        }
        Err(e) => {
            let _ = fs::remove_file(to);
            Err(e)
        }
    }
}

fn run_copies(
    jobs: Vec<(PathBuf, PathBuf)>,
    skip_existing: bool,
    cancel: &AtomicBool,
    mut on_progress: impl FnMut(Progress),
    report: &mut CopyReport,
) {
    let total = jobs.len();
    for (i, (from, to)) in jobs.into_iter().enumerate() {
        if cancel.load(Ordering::Relaxed) {
            report.cancelled = true;
            break;
        }
        let name = from.file_name().and_then(|n| n.to_str()).unwrap_or("").to_string();
        on_progress(Progress { done: i, total, name: name.clone() });
        if skip_existing && to.exists() {
            report.skipped += 1;
            continue;
        }
        match copy_file(&from, &to, cancel) {
            Ok(()) => report.copied += 1,
            Err(Error::Cancelled) => {
                report.cancelled = true;
                break;
            }
            Err(e) => report.failed.push(format!("{name}: {e}")),
        }
    }
    on_progress(Progress { done: total, total, name: String::new() });
}

/// Which files of each shot to export.
#[derive(serde::Deserialize, Clone, Copy, Debug, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    #[default]
    All,
    Raw,
    /// Everything that is not RAW (JPEG, PNG, ...).
    Jpeg,
}

/// Batch rename rule. Tokens: `{name}` original name, `{seq}` running
/// number, `{date}` and `{time}` of capture, `{camera}` model, `{tag}` the
/// tag the shots are exported under.
#[derive(serde::Deserialize, Clone, Debug)]
pub struct Rename {
    pub template: String,
    #[serde(default = "one")]
    pub start: u32,
    /// What `{tag}` stands for; empty when the export is not sorted by tag.
    #[serde(default)]
    pub tag: String,
}

fn one() -> u32 {
    1
}

fn clean(name: &str) -> String {
    let s: String = name.chars().filter(|c| !c.is_control() && !matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|')).collect();
    s.trim().trim_matches('.').to_string()
}

/// The folders an export sorted by tag puts its shots in, one per tag in the
/// order given: the tag's name without the characters a file name cannot
/// have, and no two alike (a file system that ignores case would merge them).
pub fn folder_names<'a>(tags: impl IntoIterator<Item = &'a str>) -> Vec<String> {
    let mut used = std::collections::HashSet::new();
    tags.into_iter()
        .map(|tag| {
            let base = Some(clean(tag)).filter(|name| !name.is_empty()).unwrap_or_else(|| "_".into());
            (1..).map(|n| if n == 1 { base.clone() } else { format!("{base}_{n}") }).find(|name| used.insert(name.to_lowercase())).unwrap_or(base)
        })
        .collect()
}

/// One folder of an export sorted by tag: the tag, and the shots that carry
/// it, by their place in the list of shots.
#[derive(serde::Deserialize, Clone, Debug)]
pub struct TagFolder {
    pub tag: String,
    pub shots: Vec<usize>,
}

/// One destination of an export: a folder, the rule its files are named by,
/// and the shots that go into it.
#[derive(Clone, Debug)]
pub struct Part {
    pub dir: PathBuf,
    pub rename: Option<Rename>,
    pub shots: Vec<usize>,
}

/// Where the `count` shots of an export go. Without `folders` they all go
/// into `dest`. Sorted by tag, every tag gets a folder inside `dest` for the
/// shots that carry it (a shot with two tags is in both), numbered from the
/// rule's start in each folder, with `{tag}` standing for the tag.
pub fn parts(dest: &Path, folders: &[TagFolder], rename: Option<&Rename>, count: usize) -> Vec<Part> {
    if folders.is_empty() {
        return vec![Part { dir: dest.to_path_buf(), rename: rename.cloned(), shots: (0..count).collect() }];
    }
    let names = folder_names(folders.iter().map(|f| f.tag.as_str()));
    folders
        .iter()
        .zip(names)
        .map(|(folder, name)| Part {
            dir: dest.join(name),
            rename: rename.map(|rule| Rename { tag: folder.tag.clone(), ..rule.clone() }),
            shots: folder.shots.iter().copied().filter(|i| *i < count).collect(),
        })
        .collect()
}

fn stem_of(path: &Path) -> String {
    path.file_stem().and_then(|s| s.to_str()).unwrap_or("").to_string()
}

/// The exported file-name stem of every shot (a shot is the list of files
/// that belong together, such as a RAW and its JPEG). Stems are unique.
pub fn stems(shots: &[Vec<PathBuf>], rename: Option<&Rename>) -> Vec<String> {
    let Some(rule) = rename.filter(|r| !r.template.trim().is_empty()) else {
        // Shots from different folders may be named alike. A name belongs to
        // the folder that used it first (a rendered JPEG may join the copies,
        // so the file type does not tell them apart); later ones get a number.
        let mut owners: HashMap<String, Option<&Path>> = HashMap::new();
        return shots
            .iter()
            .map(|shot| {
                let first = shot.first();
                let (stem, dir) = (first.map(|p| stem_of(p)).unwrap_or_default(), first.and_then(|p| p.parent()));
                let free = |s: &String| owners.get(&s.to_lowercase()).is_none_or(|owner| *owner == dir);
                let stem = (1..).map(|n| if n == 1 { stem.clone() } else { format!("{stem}_{n}") }).find(free).unwrap_or(stem);
                owners.insert(stem.to_lowercase(), dir);
                stem
            })
            .collect();
    };
    let last = rule.start as usize + shots.len().saturating_sub(1);
    let digits = last.to_string().len().max(3);
    let needs_meta = ["{date}", "{time}", "{camera}"].iter().any(|t| rule.template.contains(t));
    let mut used = std::collections::HashSet::new();
    shots
        .iter()
        .enumerate()
        .map(|(i, shot)| {
            let first = shot.first();
            let seq = format!("{:0digits$}", rule.start as usize + i);
            let exif = first.filter(|_| needs_meta).and_then(|p| crate::meta::read(p).ok()).map(|m| m.exif).unwrap_or_default();
            // "2026-10-22 21:14:03" -> "20261022" and "211403"
            let stamp = exif.datetime.unwrap_or_default();
            let (date, time) = stamp.split_once(' ').unwrap_or((&stamp, ""));
            let mut stem = clean(
                &rule
                    .template
                    .replace("{name}", &first.map(|p| stem_of(p)).unwrap_or_default())
                    .replace("{seq}", &seq)
                    .replace("{date}", &date.replace('-', ""))
                    .replace("{time}", &time.replace(':', ""))
                    .replace("{camera}", exif.model.as_deref().unwrap_or(""))
                    .replace("{tag}", &rule.tag),
            );
            // A rule without a unique part still must not overwrite earlier files.
            if stem.is_empty() || !used.insert(stem.to_lowercase()) {
                stem = clean(&format!("{stem}_{seq}"));
                used.insert(stem.to_lowercase());
            }
            stem
        })
        .collect()
}

/// Copies the chosen kind of file of every shot into `dest`, renaming on the
/// way if asked. A RAW file's `.xmp` sidecar travels with it, and the halves
/// of a Live Photo bundle come out as a still and a movie of the shot's name.
pub fn export(
    shots: &[Vec<PathBuf>],
    dest: &Path,
    kind: Kind,
    rename: Option<&Rename>,
    cancel: &AtomicBool,
    on_progress: impl FnMut(Progress),
) -> Result<CopyReport> {
    export_named(shots, &stems(shots, rename), dest, kind, cancel, on_progress)
}

/// Like [`export`], with the file-name stem of every shot already decided.
pub fn export_named(
    shots: &[Vec<PathBuf>],
    stems: &[String],
    dest: &Path,
    kind: Kind,
    cancel: &AtomicBool,
    on_progress: impl FnMut(Progress),
) -> Result<CopyReport> {
    fs::create_dir_all(dest)?;
    let mut jobs = Vec::new();
    for (shot, stem) in shots.iter().zip(stems) {
        for f in shot {
            let raw = formats::is_raw(f);
            if (kind == Kind::Raw && !raw) || (kind == Kind::Jpeg && raw) {
                continue;
            }
            let ext = |p: &Path| p.extension().and_then(|e| e.to_str()).unwrap_or("").to_string();
            jobs.push((f.clone(), dest.join(format!("{stem}.{}", ext(f)))));
            let side = rating::sidecar_path(f);
            if raw && side.is_file() {
                jobs.push((side.clone(), dest.join(format!("{stem}.{}", ext(&side)))));
            }
        }
    }
    jobs.dedup();
    let mut report = CopyReport::default();
    run_copies(jobs, true, cancel, on_progress, &mut report);
    Ok(report)
}

fn list_files(dir: &Path) -> Result<Vec<PathBuf>> {
    let mut out: Vec<PathBuf> = fs::read_dir(dir)?.flatten().map(|e| e.path()).filter(|p| p.is_file()).collect();
    out.sort();
    Ok(out)
}

/// For every image in `picked_dir`, copies the RAW with the same file name
/// from `raw_dir` into `dest`. With `loose`, colour-space name prefixes are
/// ignored (`IMG_` vs `_MG_`).
pub fn find_raws(
    picked_dir: &Path,
    raw_dir: &Path,
    dest: &Path,
    loose: bool,
    skip_existing: bool,
    cancel: &AtomicBool,
    on_progress: impl FnMut(Progress),
) -> Result<CopyReport> {
    let key = |p: &Path| if loose { formats::loose_stem_key(p) } else { formats::stem_key(p) };
    let mut raws: HashMap<String, PathBuf> = HashMap::new();
    for f in list_files(raw_dir)?.into_iter().filter(|f| formats::is_raw(f)) {
        raws.entry(key(&f)).or_insert(f);
    }
    let mut report = CopyReport::default();
    let mut jobs = Vec::new();
    for f in list_files(picked_dir)?.into_iter().filter(|f| formats::is_supported(f) && !formats::is_raw(f)) {
        match raws.get(&key(&f)) {
            Some(raw) => {
                if let Some(name) = raw.file_name() {
                    jobs.push((raw.clone(), dest.join(name)));
                }
                let side = rating::sidecar_path(raw);
                if side.is_file() {
                    if let Some(n) = side.file_name() {
                        jobs.push((side.clone(), dest.join(n)));
                    }
                }
            }
            None => report.missing.push(f.file_name().and_then(|n| n.to_str()).unwrap_or("").to_string()),
        }
    }
    jobs.dedup();
    fs::create_dir_all(dest)?;
    run_copies(jobs, skip_existing, cancel, on_progress, &mut report);
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shots(names: &[&[&str]]) -> Vec<Vec<PathBuf>> {
        names.iter().map(|s| s.iter().map(PathBuf::from).collect()).collect()
    }

    #[test]
    fn rename_numbers_shots_and_keeps_pairs_together() {
        let list = shots(&[&["/a/DSC_0001.JPG", "/a/DSC_0001.NEF"], &["/a/DSC_0002.JPG"]]);
        let rule = Rename { template: "Trip_{seq}".into(), start: 9, tag: String::new() };
        assert_eq!(stems(&list, Some(&rule)), ["Trip_009", "Trip_010"]);
        assert_eq!(stems(&list, None), ["DSC_0001", "DSC_0002"]);
    }

    #[test]
    fn shots_named_alike_in_different_folders_do_not_overwrite_each_other() {
        let list = shots(&[&["/a/DSC_0001.JPG", "/a/DSC_0001.NEF"], &["/b/dsc_0001.jpg"], &["/c/DSC_0001.NEF"], &["/b/DSC_0002.JPG"], &["/c/DSC_0001.PNG"]]);
        // Within one folder nothing changes: files named alike there stay named alike.
        assert_eq!(stems(&list, None), ["DSC_0001", "dsc_0001_2", "DSC_0001_3", "DSC_0002", "DSC_0001_3"]);
    }

    #[test]
    fn rename_never_produces_duplicates_or_paths() {
        let list = shots(&[&["/a/x.jpg"], &["/a/y.jpg"], &["/a/z.jpg"]]);
        let fixed = Rename { template: "same".into(), start: 1, tag: String::new() };
        assert_eq!(stems(&list, Some(&fixed)), ["same", "same_002", "same_003"]);
        let sneaky = Rename { template: "../{name}".into(), start: 1, tag: String::new() };
        assert_eq!(stems(&list, Some(&sneaky)), ["x", "y", "z"]);
    }

    #[test]
    fn a_tag_names_its_folder_and_its_files() {
        assert_eq!(folder_names(["Ann", "a/b", "ann", "..", "A:B"]), ["Ann", "ab", "ann_2", "_", "AB_2"]);
        let list = shots(&[&["/a/x.jpg", "/a/x.nef"], &["/a/y.jpg"]]);
        let rule = Rename { template: "{tag}_{seq}".into(), start: 1, tag: "Ann".into() };
        assert_eq!(stems(&list, Some(&rule)), ["Ann_001", "Ann_002"]);
        // Outside an export sorted by tag the token stands for nothing.
        let plain = Rename { tag: String::new(), ..rule };
        assert_eq!(stems(&list, Some(&plain)), ["_001", "_002"]);
    }

    #[test]
    fn an_export_sorted_by_tag_has_a_folder_and_a_count_for_each_tag() {
        let rule = Rename { template: "{tag}_{seq}".into(), start: 1, tag: String::new() };
        let all = parts(Path::new("/out"), &[], Some(&rule), 3);
        assert_eq!((all.len(), all[0].dir.as_path(), all[0].shots.as_slice()), (1, Path::new("/out"), &[0, 1, 2][..]));
        let folders = [TagFolder { tag: "Ann".into(), shots: vec![0, 2] }, TagFolder { tag: "Group: all".into(), shots: vec![0, 7] }];
        let sorted = parts(Path::new("/out"), &folders, Some(&rule), 3);
        assert_eq!(sorted.iter().map(|p| p.dir.clone()).collect::<Vec<_>>(), [PathBuf::from("/out/Ann"), PathBuf::from("/out/Group all")]);
        // A shot that is not in the list is left out rather than failing the export.
        assert_eq!(sorted.iter().map(|p| p.shots.clone()).collect::<Vec<_>>(), [vec![0, 2], vec![0]]);
        let list = shots(&[&["/a/x.jpg"], &["/a/y.jpg"], &["/a/z.jpg"]]);
        let names: Vec<Vec<String>> = sorted.iter().map(|p| stems(&p.shots.iter().map(|i| list[*i].clone()).collect::<Vec<_>>(), p.rename.as_ref())).collect();
        assert_eq!(names, [vec!["Ann_001", "Ann_002"], vec!["Group all_001"]]);
        // Without a rule the files keep their names in every folder.
        assert!(parts(Path::new("/out"), &folders, None, 3).iter().all(|p| p.rename.is_none()));
    }

    #[test]
    fn export_filters_by_kind_and_renames() {
        let dir = std::env::temp_dir().join(format!("sift-export-{}", std::process::id()));
        let (src, out) = (dir.join("src"), dir.join("out"));
        fs::create_dir_all(&src).unwrap();
        for name in ["A.JPG", "A.NEF", "A.xmp", "B.JPG"] {
            fs::write(src.join(name), name).unwrap();
        }
        let list = vec![vec![src.join("A.JPG"), src.join("A.NEF")], vec![src.join("B.JPG")]];
        let rule = Rename { template: "P{seq}".into(), start: 1, tag: String::new() };
        let cancel = AtomicBool::new(false);
        let report = export(&list, &out, Kind::Raw, Some(&rule), &cancel, |_| {}).unwrap();
        assert_eq!(report.copied, 2);
        let mut names: Vec<String> = fs::read_dir(&out).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
        names.sort();
        assert_eq!(names, ["P001.NEF", "P001.xmp"]);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_live_photo_bundle_is_exported_as_a_still_and_a_movie() {
        let dir = std::env::temp_dir().join(format!("sift-export-pvt-{}", std::process::id()));
        let (bundle, out) = (dir.join("src/A_live.pvt"), dir.join("out"));
        fs::create_dir_all(&bundle).unwrap();
        for name in ["A_live.JPG", "A_live.MOV"] {
            fs::write(bundle.join(name), name).unwrap();
        }
        let list = vec![vec![bundle.join("A_live.JPG"), bundle.join("A_live.MOV")]];
        let rule = Rename { template: "Trip_{seq}".into(), start: 1, tag: String::new() };
        let report = export(&list, &out, Kind::All, Some(&rule), &AtomicBool::new(false), |_| {}).unwrap();
        assert_eq!(report.copied, 2);
        let mut names: Vec<String> = fs::read_dir(&out).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
        names.sort();
        assert_eq!(names, ["Trip_001.JPG", "Trip_001.MOV"]);
        assert_eq!(fs::read_to_string(out.join("Trip_001.MOV")).unwrap(), "A_live.MOV");
        let _ = fs::remove_dir_all(&dir);
    }
}
