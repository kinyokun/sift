use crate::protocol;
use crate::state::{path_hash, AppState};
use serde::{Deserialize, Serialize};
use sift_core::af::AfBox;
use sift_core::export::{self, CopyReport, Progress};
use sift_core::frame::FrameOptions;
use sift_core::{face, formats, frame, meta, preview, rating, stack};
use std::cmp::Ordering;
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering as AtomicOrdering;
use std::time::UNIX_EPOCH;
use tauri::{AppHandle, Emitter, Manager, State};

type CmdResult<T> = Result<T, String>;

const EYE_SEARCH_EDGE: u32 = 1600;
const FRAMED_QUALITY: u8 = 93;

fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}

async fn blocking<T: Send + 'static>(f: impl FnOnce() -> CmdResult<T> + Send + 'static) -> CmdResult<T> {
    tauri::async_runtime::spawn_blocking(f).await.map_err(err)?
}

#[derive(Serialize)]
pub struct Item {
    /// Path of the file that is displayed (the JPEG of a RAW+JPEG pair).
    pub id: String,
    pub name: String,
    /// Every file belonging to this shot; ratings and copies apply to all.
    pub paths: Vec<String>,
    pub raw: bool,
    pub paired: bool,
    /// The movie of a Live Photo, when the displayed file has one. It is not
    /// among `paths`: ratings are never written to it.
    pub live: Option<String>,
    pub size: u64,
    /// Modification time in milliseconds, used to version image URLs.
    pub mtime: u64,
}

/// Orders names the way a person would: `IMG_9.jpg` before `IMG_10.jpg`.
fn natural_cmp(a: &str, b: &str) -> Ordering {
    let (a, b) = (a.to_lowercase(), b.to_lowercase());
    let (mut x, mut y) = (a.chars().peekable(), b.chars().peekable());
    loop {
        match (x.peek().copied(), y.peek().copied()) {
            (None, None) => return Ordering::Equal,
            (None, _) => return Ordering::Less,
            (_, None) => return Ordering::Greater,
            (Some(c), Some(d)) if c.is_ascii_digit() && d.is_ascii_digit() => {
                let take = |it: &mut std::iter::Peekable<std::str::Chars>| {
                    let mut s = String::new();
                    while let Some(ch) = it.peek().filter(|ch| ch.is_ascii_digit()) {
                        s.push(*ch);
                        it.next();
                    }
                    s.trim_start_matches('0').to_string()
                };
                let (n, m) = (take(&mut x), take(&mut y));
                let ord = n.len().cmp(&m.len()).then_with(|| n.cmp(&m));
                if ord != Ordering::Equal {
                    return ord;
                }
            }
            (Some(c), Some(d)) => {
                if c != d {
                    return c.cmp(&d);
                }
                x.next();
                y.next();
            }
        }
    }
}

fn stat(path: &Path) -> (u64, u64) {
    fs::metadata(path).map_or((0, 0), |m| {
        let t = m.modified().ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map_or(0, |d| d.as_millis() as u64);
        (m.len(), t)
    })
}

pub fn scan(dir: &Path, pair: bool, prefer_raw: bool) -> CmdResult<Vec<Item>> {
    // Movies are not listed; one named like a photo may be its Live Photo half.
    let mut movies: HashMap<String, PathBuf> = HashMap::new();
    let mut files: Vec<PathBuf> = fs::read_dir(dir)
        .map_err(err)?
        .flatten()
        .filter_map(|e| {
            let p = e.path();
            let hidden = p.file_name().and_then(|n| n.to_str()).is_none_or(|n| n.starts_with('.'));
            if !hidden && p.extension().is_some_and(|x| x.eq_ignore_ascii_case("mov")) {
                movies.insert(formats::stem_key(&p), p);
                return None;
            }
            if hidden || !formats::is_supported(&p) {
                return None;
            }
            // The directory listing usually knows the type already; asking again
            // costs a round trip per file on a network share. Links are followed.
            let file = match e.file_type() {
                Ok(t) if t.is_file() => true,
                Ok(t) if t.is_dir() => false,
                _ => p.is_file(),
            };
            file.then_some(p)
        })
        .collect();
    files.sort_by(|a, b| natural_cmp(&a.to_string_lossy(), &b.to_string_lossy()));

    let mut groups: Vec<Vec<PathBuf>> = Vec::new();
    let mut by_stem: HashMap<String, usize> = HashMap::new();
    for f in files {
        match by_stem.get(&formats::stem_key(&f)).filter(|_| pair) {
            Some(&i) => groups[i].push(f),
            None => {
                by_stem.insert(formats::stem_key(&f), groups.len());
                groups.push(vec![f]);
            }
        }
    }
    Ok(groups
        .into_iter()
        .map(|mut g| {
            // The first file of a pair is the one displayed: the camera's JPEG,
            // or the RAW's embedded preview when the user prefers that.
            g.sort_by_key(|p| if prefer_raw { (!formats::is_raw(p), false) } else { (!formats::is_jpeg(p), formats::is_raw(p)) });
            let primary = &g[0];
            let (size, mtime) = stat(primary);
            Item {
                id: primary.to_string_lossy().into_owned(),
                name: primary.file_name().unwrap_or_default().to_string_lossy().into_owned(),
                raw: g.iter().any(|p| formats::is_raw(p)),
                paired: g.len() > 1,
                live: movies.get(&formats::stem_key(primary)).and_then(|m| sift_core::live::movie_of(primary, m)).map(|m| m.to_string_lossy().into_owned()),
                paths: g.iter().map(|p| p.to_string_lossy().into_owned()).collect(),
                size,
                mtime,
            }
        })
        .collect())
}

#[tauri::command]
pub async fn scan_folder(app: AppHandle, path: String, pair: bool, prefer_raw: bool) -> CmdResult<Vec<Item>> {
    blocking(move || {
        let dir = PathBuf::from(&path);
        if !dir.is_dir() {
            return Err(format!("not a folder: {path}"));
        }
        app.state::<AppState>().allow(&dir);
        scan(&dir, pair, prefer_raw)
    })
    .await
}

#[tauri::command]
pub async fn read_meta(app: AppHandle, path: String) -> CmdResult<meta::Meta> {
    blocking(move || meta::read(&app.state::<AppState>().check(&path)?).map_err(err)).await
}

/// Rating and capture time for many files at once, in input order.
pub fn briefs_of(state: &AppState, paths: &[String]) -> Vec<meta::Brief> {
    let files: Vec<Option<PathBuf>> = paths.iter().map(|p| state.check(p).ok()).collect();
    let workers = std::thread::available_parallelism().map_or(4, |n| n.get()).min(8);
    let chunk = files.len().div_ceil(workers).max(1);
    let mut out = vec![meta::Brief::default(); files.len()];
    std::thread::scope(|s| {
        for (slots, part) in out.chunks_mut(chunk).zip(files.chunks(chunk)) {
            s.spawn(move || {
                for (slot, f) in slots.iter_mut().zip(part) {
                    *slot = f.as_ref().and_then(|f| meta::brief(f).ok()).unwrap_or_default();
                }
            });
        }
    });
    // The series each file belongs to is kept for `stacks_of`, which needs the whole folder at once.
    let mut series = state.series.lock().unwrap();
    for (path, brief) in paths.iter().zip(&out) {
        series.insert(path.clone(), brief.series.clone());
    }
    out
}

/// Shots that belong together, by the path they are displayed by.
#[derive(Serialize)]
pub struct Stack {
    pub kind: stack::Kind,
    /// In shooting order; a merged pixel-shift file comes first.
    pub ids: Vec<String>,
}

/// Bursts and pixel-shift series among `paths` (the displayed file of every
/// shot in a folder). Files `read_briefs` has seen are not read again.
pub fn stacks_of(state: &AppState, paths: &[String]) -> Vec<Stack> {
    let unread: Vec<String> = {
        let series = state.series.lock().unwrap();
        paths.iter().filter(|p| !series.contains_key(*p)).cloned().collect()
    };
    if !unread.is_empty() {
        briefs_of(state, &unread);
    }
    let series = state.series.lock().unwrap();
    let hints: Vec<stack::Hint> = paths.iter().map(|p| series.get(p).cloned().unwrap_or_default()).collect();
    stack::group(&hints).into_iter().map(|s| Stack { kind: s.kind, ids: s.members.into_iter().map(|i| paths[i].clone()).collect() }).collect()
}

#[tauri::command]
pub async fn group_shots(app: AppHandle, paths: Vec<String>) -> CmdResult<Vec<Stack>> {
    blocking(move || Ok(stacks_of(&app.state::<AppState>(), &paths))).await
}

/// Capture times of `frames`, in milliseconds, where the files record them.
fn taken_at(state: &AppState, frames: &[String]) -> Vec<Option<i64>> {
    stacks_of(state, frames);
    let series = state.series.lock().unwrap();
    frames.iter().map(|p| series.get(p).and_then(|h| h.time_ms)).collect()
}

/// Seconds each frame of a burst is shown for when it plays as a Live Photo.
pub fn live_pace(state: &AppState, frames: &[String]) -> f64 {
    let (_, duration) = sift_core::live::timeline(&taken_at(state, frames));
    duration / frames.len().max(1) as f64
}

#[tauri::command]
pub async fn live_photo_pace(app: AppHandle, frames: Vec<String>) -> CmdResult<f64> {
    blocking(move || Ok(live_pace(&app.state::<AppState>(), &frames))).await
}

/// Writes a burst as a Live Photo (a JPEG and a movie that belong together)
/// next to its frames. `frames` are the displayed files in shooting order and
/// `key` is the one that becomes the still. Returns the path of the JPEG.
#[cfg(target_vendor = "apple")]
pub fn live_photo(state: &AppState, frames: &[String], key: usize, on_frame: impl FnMut(usize)) -> CmdResult<String> {
    let files = frames.iter().map(|p| state.check(p)).collect::<Result<Vec<_>, _>>()?;
    let key = key.min(files.len().saturating_sub(1));
    let cover = files.get(key).ok_or("no frames")?;
    let dest = cover.parent().ok_or("no folder")?;
    let base = cover.file_stem().map_or_else(|| "Live".into(), |s| s.to_string_lossy().into_owned());
    // Never over an existing file: the pair gets a name of its own.
    let stem = (1..)
        .map(|n| if n == 1 { format!("{base}_live") } else { format!("{base}_live_{n}") })
        .find(|stem| !dest.join(format!("{stem}.JPG")).exists() && !dest.join(format!("{stem}.MOV")).exists())
        .unwrap_or_default();
    let taken = taken_at(state, frames);
    state.cancel.store(false, AtomicOrdering::Relaxed);
    let (still, _) = sift_core::live::export(&files, &taken, key, dest, &stem, &state.cancel, on_frame).map_err(err)?;
    Ok(still.to_string_lossy().into_owned())
}

#[cfg(not(target_vendor = "apple"))]
pub fn live_photo(_: &AppState, _: &[String], _: usize, _: impl FnMut(usize)) -> CmdResult<String> {
    Err("Live Photos can only be made on Apple platforms".into())
}

#[tauri::command]
pub async fn save_live_photo(app: AppHandle, frames: Vec<String>, key: usize) -> CmdResult<String> {
    blocking(move || {
        let total = frames.len();
        let mut emit = progress(&app);
        live_photo(&app.state::<AppState>(), &frames, key, |done| emit(Progress { done, total, name: String::new() }))
    })
    .await
}

#[tauri::command]
pub async fn read_briefs(app: AppHandle, paths: Vec<String>) -> CmdResult<Vec<meta::Brief>> {
    blocking(move || Ok(briefs_of(&app.state::<AppState>(), &paths))).await
}

/// Writes the rating to every file of a shot; reports where it was stored.
pub fn rate(state: &AppState, paths: &[String], rating: u8) -> CmdResult<rating::Stored> {
    let mut stored = rating::Stored::Embedded;
    for p in paths {
        if rating::write(&state.check(p)?, rating).map_err(err)? == rating::Stored::Sidecar {
            stored = rating::Stored::Sidecar;
        }
    }
    Ok(stored)
}

#[tauri::command]
pub async fn set_rating(app: AppHandle, paths: Vec<String>, rating: u8) -> CmdResult<rating::Stored> {
    blocking(move || rate(&app.state::<AppState>(), &paths, rating)).await
}

#[derive(Serialize, Clone, Copy)]
pub struct EyeTarget {
    /// Point to centre on, in normalised display coordinates.
    pub x: f32,
    pub y: f32,
    /// The face, or the whole animal.
    pub face: [f32; 4],
    /// "face" for a person, otherwise the animal ("cat", "bird", ...).
    pub kind: &'static str,
}

/// Finds the eyes of the main subject: a person's, or failing that an
/// animal's. `af` biases the choice towards what the camera focused on.
pub fn eyes_of(state: &AppState, path: String, af: &[AfBox]) -> CmdResult<Option<EyeTarget>> {
    let file = state.check(&path)?;
    if let Some(hit) = state.eyes.lock().unwrap().get(&path) {
        return Ok(*hit);
    }
    let detector = state.detector().ok_or("face model failed to load")?;
    let img = preview::pixels(&file, EYE_SEARCH_EDGE).map_err(err)?;
    let faces = detector.detect(&img).map_err(err)?;
    let focus: Vec<(f32, f32)> = af.iter().filter(|b| b.focused).map(|b| (b.x + b.w / 2.0, b.y + b.h / 2.0)).collect();
    let inside = |b: [f32; 4], p: &(f32, f32)| p.0 >= b[0] && p.0 <= b[0] + b[2] && p.1 >= b[1] && p.1 <= b[1] + b[3];
    let person = face::subject(&faces, af).map(|f| {
        let (x, y) = f.eye_center();
        EyeTarget { x, y, face: [f.x, f.y, f.w, f.h], kind: "face" }
    });
    // People come first, unless the camera focused somewhere that is no face:
    // then the pet beside them may be the subject.
    let elsewhere = !focus.is_empty() && person.is_some_and(|t| !focus.iter().any(|p| inside(t.face, p)));
    let animal = if person.is_none() || elsewhere {
        let found = state.animals().and_then(|d| d.subject(&img, af).ok().flatten());
        found.and_then(|a| {
            let whole = [a.x, a.y, a.w, a.h];
            let focused = focus.iter().find(|p| inside(whole, p)).copied();
            // Without eyes the network is sure of, a focus point on the animal
            // is the next best thing: subject-detection AF aims for the eye.
            let (x, y) = a.eye_center().or(focused)?;
            (person.is_none() || focused.is_some()).then_some(EyeTarget { x, y, face: whole, kind: a.kind })
        })
    } else {
        None
    };
    let target = animal.or(person);
    state.eyes.lock().unwrap().insert(path, target);
    Ok(target)
}

#[tauri::command]
pub async fn locate_eyes(app: AppHandle, path: String, af: Vec<AfBox>) -> CmdResult<Option<EyeTarget>> {
    blocking(move || eyes_of(&app.state::<AppState>(), path, &af)).await
}

/// Develops a RAW at full resolution so `sift://localhost/dev` can serve it.
#[tauri::command]
pub async fn develop_raw(app: AppHandle, path: String) -> CmdResult<()> {
    blocking(move || {
        let state = app.state::<AppState>();
        let file = state.check(&path)?;
        protocol::developed(&state, &file).map(|_| ()).map_err(err)
    })
    .await
}

fn progress(app: &AppHandle) -> impl FnMut(Progress) + '_ {
    move |p| {
        let _ = app.emit("task-progress", p);
    }
}

#[tauri::command]
pub fn cancel_task(state: State<AppState>) {
    state.cancel.store(true, AtomicOrdering::Relaxed);
}

fn checked_shots(state: &AppState, shots: &[Vec<String>]) -> CmdResult<Vec<Vec<PathBuf>>> {
    shots.iter().map(|shot| shot.iter().map(|f| state.check(f)).collect()).collect()
}

/// Cuts an image down to a normalised `[x, y, width, height]` rectangle.
fn crop_to(img: image::RgbImage, c: &[f32; 4]) -> image::RgbImage {
    let (iw, ih) = (img.width(), img.height());
    let x = ((c[0] * iw as f32).round() as u32).min(iw.saturating_sub(1));
    let y = ((c[1] * ih as f32).round() as u32).min(ih.saturating_sub(1));
    let w = ((c[2] * iw as f32).round() as u32).clamp(1, iw - x);
    let h = ((c[3] * ih as f32).round() as u32).clamp(1, ih - y);
    image::imageops::crop_imm(&img, x, y, w, h).to_image()
}

/// Upright full-size pixels of a shot the way the user set it up while
/// culling: with its look applied and cut to its crop.
fn styled_pixels(state: &AppState, shot: &[PathBuf], look: Option<&String>, crop: Option<&[f32; 4]>) -> CmdResult<image::RgbImage> {
    let img = match look {
        Some(look) => protocol::look_pixels(state, look_source(shot).ok_or("empty shot")?, look, 0)?,
        None => {
            let path = shot.first().ok_or("empty shot")?;
            best_pixels(state, path, &meta::read(path).map_err(err)?).map_err(err)?
        }
    };
    Ok(match crop {
        Some(c) => crop_to(img, c),
        None => img,
    })
}

/// How a set of shots is exported.
#[derive(Deserialize, Default)]
pub struct ExportPlan {
    #[serde(default)]
    pub kind: export::Kind,
    #[serde(default)]
    pub rename: Option<export::Rename>,
    /// First path of a shot -> look id.
    #[serde(default)]
    pub looks: HashMap<String, String>,
    /// First path of a shot -> normalised `[x, y, width, height]` crop.
    #[serde(default)]
    pub crops: HashMap<String, [f32; 4]>,
}

/// The file a look is rendered from: the RAW of a shot if it has one.
fn look_source(shot: &[PathBuf]) -> Option<&PathBuf> {
    shot.iter().find(|p| formats::is_raw(p)).or(shot.first())
}

/// Copies the chosen kind of file of every shot, optionally renamed. Shots
/// with a look or crop assigned (keyed by the first path of the shot) get a
/// JPEG rendered that way instead of a copy of the camera's JPEG.
pub fn export_shots(state: &AppState, shots: &[Vec<String>], dest: &str, plan: &ExportPlan, mut on_progress: impl FnMut(Progress)) -> CmdResult<CopyReport> {
    let ExportPlan { kind, looks, crops, .. } = plan;
    let (kind, rename) = (*kind, plan.rename.as_ref());
    let all = checked_shots(state, shots)?;
    state.cancel.store(false, AtomicOrdering::Relaxed);
    let stems = export::stems(&all, rename);
    let dest = Path::new(dest);
    fs::create_dir_all(dest).map_err(err)?;
    let mut report = CopyReport::default();
    // Files that are copied as they are, with the name their shot was given.
    let (mut plain, mut names) = (Vec::new(), Vec::new());
    for (i, shot) in all.iter().enumerate() {
        let look = shots[i].first().and_then(|p| looks.get(p));
        let crop = shots[i].first().and_then(|p| crops.get(p));
        if kind == export::Kind::Raw || (look.is_none() && crop.is_none()) {
            plain.push(shot.clone());
            names.push(stems[i].clone());
            continue;
        }
        if state.cancel.load(AtomicOrdering::Relaxed) {
            report.cancelled = true;
            return Ok(report);
        }
        on_progress(Progress { done: i, total: all.len(), name: stems[i].clone() });
        let rendered = styled_pixels(state, shot, look, crop)
            .and_then(|img| preview::encode_jpeg(&img, FRAMED_QUALITY).map_err(err))
            .and_then(|jpeg| fs::write(dest.join(format!("{}.jpg", stems[i])), jpeg).map_err(err));
        match rendered {
            Ok(()) => report.copied += 1,
            Err(e) => report.failed.push(format!("{}: {e}", stems[i])),
        }
        // The RAW still travels along unchanged when RAW files are wanted.
        let raws: Vec<PathBuf> = shot.iter().filter(|p| formats::is_raw(p)).cloned().collect();
        if kind == export::Kind::All && !raws.is_empty() {
            plain.push(raws);
            names.push(stems[i].clone());
        }
    }
    let copied = export::export_named(&plain, &names, dest, kind, &state.cancel, on_progress).map_err(err)?;
    report.copied += copied.copied;
    report.skipped += copied.skipped;
    report.failed.extend(copied.failed);
    report.cancelled |= copied.cancelled;
    Ok(report)
}

#[tauri::command]
pub async fn export_files(app: AppHandle, shots: Vec<Vec<String>>, dest: String, plan: ExportPlan) -> CmdResult<CopyReport> {
    blocking(move || export_shots(&app.state::<AppState>(), &shots, &dest, &plan, progress(&app))).await
}

#[derive(Serialize)]
pub struct LookInfo {
    /// File name inside the look library.
    id: String,
    name: String,
    /// True when the look is an approximation (Nikon NP3 slider values).
    approximate: bool,
    /// A Nikon Picture Control: exact on NEF files in builds with Nikon's renderer.
    nikon: bool,
    /// It carries a custom tone curve, which stays an approximation even then.
    curve: bool,
}

/// Histogram of the photo as the viewer shows it: with its look, inside its crop.
pub fn histogram_of(state: &AppState, path: &str, look: Option<&str>, edge: u32, crop: Option<[f32; 4]>) -> CmdResult<sift_core::histogram::Histogram> {
    let path = state.check(path)?;
    let img = protocol::analysis_pixels(state, &path, look, edge, protocol::HISTOGRAM_EDGE).map_err(err)?;
    Ok(sift_core::histogram::of(&img, crop))
}

#[tauri::command]
pub async fn histogram(app: AppHandle, path: String, look: Option<String>, edge: u32, crop: Option<[f32; 4]>) -> CmdResult<sift_core::histogram::Histogram> {
    blocking(move || histogram_of(&app.state::<AppState>(), &path, look.as_deref(), edge, crop)).await
}

/// Looks in the library, by name.
pub fn looks_of(state: &AppState) -> Vec<LookInfo> {
    let mut ids: Vec<String> = fs::read_dir(state.looks_dir())
        .map(|rd| rd.flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect())
        .unwrap_or_default();
    ids.sort_by(|a, b| natural_cmp(a, b));
    ids.into_iter()
        .filter_map(|id| {
            state.look(&id).ok().map(|l| LookInfo { name: l.name.clone(), approximate: l.approximate, nikon: l.np3.is_some(), curve: l.np3.as_ref().is_some_and(|n| n.curve.is_some()), id })
        })
        .collect()
}

/// Adds `.cube` / `.NP3` data to the library after checking that it parses.
pub fn add_look(state: &AppState, file_name: &str, data: &[u8]) -> CmdResult<()> {
    let name = Path::new(file_name).file_name().and_then(|n| n.to_str()).ok_or("bad file name")?;
    if !sift_core::look::is_look_file(Path::new(name)) {
        return Err(format!("{name}: not a .cube, .3dl, HaldCLUT .png or .NP3 file"));
    }
    fs::create_dir_all(state.looks_dir()).map_err(err)?;
    let target = state.looks_dir().join(name);
    fs::write(&target, data).map_err(err)?;
    state.looks.lock().unwrap().remove(name);
    if let Err(e) = state.look(name) {
        let _ = fs::remove_file(&target);
        return Err(format!("{name}: {e}"));
    }
    Ok(())
}

/// Saves the Picture Control a photo was shot with (a recipe that may exist
/// nowhere else) to the library as an NP3 file; returns the look's id.
pub fn keep_camera_look(state: &AppState, path: &str) -> CmdResult<String> {
    let path = state.check(path)?;
    // The MakerNote sits near the start of the file; a RAW on a network share is not read whole.
    let src = sift_core::io::Src::open(&path).map_err(err)?;
    let np3 = sift_core::nikon::recorded_look(&src.read_upto(0, 8 << 20)).or_else(|_| sift_core::nikon::recorded_look(&fs::read(&path)?)).map_err(err)?;
    let name = sift_core::look::parse_np3(&np3).map_err(err)?.name;
    let safe: String = name.chars().map(|c| if c.is_alphanumeric() || " -_.()".contains(c) { c } else { '_' }).collect();
    let safe = safe.trim().trim_matches('.').to_string();
    if safe.is_empty() {
        return Err("the Picture Control has no name".into());
    }
    // The same recipe is kept once; another recipe of the same name gets a number.
    let id = (1..100)
        .map(|n| if n == 1 { format!("{safe}.NP3") } else { format!("{safe} {n}.NP3") })
        .find(|id| fs::read(state.looks_dir().join(id)).map_or(true, |old| old == np3))
        .ok_or("too many looks of this name")?;
    add_look(state, &id, &np3)?;
    Ok(id)
}

#[tauri::command]
pub async fn save_camera_look(app: AppHandle, path: String) -> CmdResult<String> {
    blocking(move || keep_camera_look(&app.state::<AppState>(), &path)).await
}

#[tauri::command]
pub async fn list_looks(app: AppHandle) -> CmdResult<Vec<LookInfo>> {
    blocking(move || Ok(looks_of(&app.state::<AppState>()))).await
}

#[tauri::command]
pub async fn import_looks(app: AppHandle, sources: Vec<String>) -> CmdResult<Vec<LookInfo>> {
    blocking(move || {
        use std::io::Read;
        use tauri_plugin_fs::{FilePath, FsExt, OpenOptions};
        let state = app.state::<AppState>();
        for src in &sources {
            let decoded = percent_encoding::percent_decode_str(src).decode_utf8_lossy().into_owned();
            let name = decoded.rsplit(['/', '\\', ':']).next().unwrap_or("look").to_string();
            let mut opts = OpenOptions::new();
            opts.read(true);
            let mut data = Vec::new();
            app.fs().open(src.parse::<FilePath>().map_err(err)?, opts).map_err(err)?.read_to_end(&mut data).map_err(err)?;
            add_look(&state, &name, &data)?;
        }
        Ok(looks_of(&state))
    })
    .await
}

#[tauri::command]
pub async fn remove_look(app: AppHandle, id: String) -> CmdResult<Vec<LookInfo>> {
    blocking(move || {
        let state = app.state::<AppState>();
        if Path::new(&id).file_name().is_some_and(|n| n.to_str() == Some(id.as_str())) {
            let _ = fs::remove_file(state.looks_dir().join(&id));
            state.looks.lock().unwrap().remove(&id);
        }
        Ok(looks_of(&state))
    })
    .await
}

/// The names a rename rule gives the first shots, for the export sheet's preview.
pub fn names_preview(state: &AppState, shots: &[Vec<String>], rename: &export::Rename) -> CmdResult<Vec<String>> {
    Ok(export::stems(&checked_shots(state, shots)?, Some(rename)))
}

#[tauri::command]
pub async fn export_names(app: AppHandle, shots: Vec<Vec<String>>, rename: export::Rename) -> CmdResult<Vec<String>> {
    blocking(move || names_preview(&app.state::<AppState>(), &shots, &rename)).await
}

#[tauri::command]
pub async fn find_raws(app: AppHandle, picked: String, raws: String, dest: String, loose: bool) -> CmdResult<CopyReport> {
    blocking(move || {
        let state = app.state::<AppState>();
        state.cancel.store(false, AtomicOrdering::Relaxed);
        export::find_raws(Path::new(&picked), Path::new(&raws), Path::new(&dest), loose, true, &state.cancel, progress(&app))
            .map_err(err)
    })
    .await
}

#[derive(Deserialize)]
pub struct FramedRequest {
    files: Vec<String>,
    dest: String,
    options: FrameOptions,
    /// Long edge of the photo inside the frame; 0 keeps the source size.
    max_edge: u32,
    #[serde(default)]
    rename: Option<export::Rename>,
    /// File -> look id for photos that should be rendered with a look.
    #[serde(default)]
    looks: HashMap<String, String>,
    /// File -> the file its look is rendered from (the RAW of a pair).
    #[serde(default)]
    sources: HashMap<String, String>,
    /// File -> normalised `[x, y, width, height]` crop.
    #[serde(default)]
    crops: HashMap<String, [f32; 4]>,
}

/// Full-resolution upright pixels: the embedded preview, or a development
/// when the preview is too small or missing.
fn best_pixels(state: &AppState, path: &Path, m: &meta::Meta) -> sift_core::Result<image::RgbImage> {
    if m.raw && m.low_res {
        return preview::decode_upright(&protocol::developed(state, path)?);
    }
    preview::pixels(path, u32::MAX)
}

/// Renders every requested photo with its info frame into `request.dest`.
pub fn frame_files(state: &AppState, request: &FramedRequest, mut on_progress: impl FnMut(Progress)) -> CmdResult<CopyReport> {
    state.cancel.store(false, AtomicOrdering::Relaxed);
    let dest = PathBuf::from(&request.dest);
    fs::create_dir_all(&dest).map_err(err)?;
    let mut report = CopyReport::default();
    let total = request.files.len();
    let shots: Vec<Vec<String>> = request.files.iter().map(|f| vec![f.clone()]).collect();
    let stems = export::stems(&checked_shots(state, &shots)?, request.rename.as_ref());
    for (i, f) in request.files.iter().enumerate() {
        if state.cancel.load(AtomicOrdering::Relaxed) {
            report.cancelled = true;
            break;
        }
        let path = state.check(f)?;
        let name = path.file_name().unwrap_or_default().to_string_lossy().into_owned();
        on_progress(Progress { done: i, total, name: name.clone() });
        let result = (|| -> sift_core::Result<()> {
            let m = meta::read(&path)?;
            let mut img = match request.looks.get(f) {
                Some(look) => {
                    let source = request.sources.get(f).and_then(|s| state.check(s).ok()).unwrap_or_else(|| path.clone());
                    protocol::look_pixels(state, &source, look, 0).map_err(sift_core::Error::Decode)?
                }
                None => best_pixels(state, &path, &m)?,
            };
            if let Some(c) = request.crops.get(f) {
                img = crop_to(img, c);
            }
            if request.max_edge > 0 && img.width().max(img.height()) > request.max_edge {
                let k = request.max_edge as f32 / img.width().max(img.height()) as f32;
                let (w, h) = ((img.width() as f32 * k).round() as u32, (img.height() as f32 * k).round() as u32);
                img = image::imageops::resize(&img, w.max(1), h.max(1), image::imageops::FilterType::Lanczos3);
            }
            let framed = frame::render(&img, &m.exif, &request.options);
            let name = if request.rename.is_some() { format!("{}.jpg", stems[i]) } else { format!("{}_framed.jpg", stems[i]) };
            fs::write(dest.join(name), preview::encode_jpeg(&framed, FRAMED_QUALITY)?)?;
            Ok(())
        })();
        match result {
            Ok(()) => report.copied += 1,
            Err(e) => report.failed.push(format!("{name}: {e}")),
        }
    }
    on_progress(Progress { done: total, total, name: String::new() });
    Ok(report)
}

#[tauri::command]
pub async fn export_framed(app: AppHandle, request: FramedRequest) -> CmdResult<CopyReport> {
    blocking(move || frame_files(&app.state::<AppState>(), &request, progress(&app))).await
}

/// A small framed rendition for the export sheet's live preview, as JPEG bytes.
pub fn framed_preview(state: &AppState, path: &str, options: &FrameOptions) -> CmdResult<Vec<u8>> {
    let file = state.check(path)?;
    let m = meta::read(&file).map_err(err)?;
    let img = preview::pixels(&file, 1200).map_err(err)?;
    preview::encode_jpeg(&frame::render(&img, &m.exif, options), 88).map_err(err)
}

#[tauri::command]
pub async fn preview_framed(app: AppHandle, path: String, options: FrameOptions) -> CmdResult<tauri::ipc::Response> {
    blocking(move || framed_preview(&app.state::<AppState>(), &path, &options).map(tauri::ipc::Response::new)).await
}

/// Writes a file so that a crash or power cut leaves either the old or the
/// new contents, never a torn file.
fn write_atomic(file: &Path, data: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    if let Some(dir) = file.parent() {
        fs::create_dir_all(dir)?;
    }
    let tmp = file.with_extension("tmp");
    let mut f = fs::File::create(&tmp)?;
    f.write_all(data)?;
    sift_core::io::sync(&f)?;
    fs::rename(&tmp, file)
}

fn session_file(state: &AppState, folder: &str) -> PathBuf {
    state.data_dir.join("sessions").join(format!("{}.json", path_hash(Path::new(folder))))
}

fn last_file(state: &AppState) -> PathBuf {
    state.data_dir.join("last-folder.json")
}

/// Culling progress of a folder (flags, position, filter); `null` when new.
pub fn session_of(state: &AppState, folder: &str) -> serde_json::Value {
    fs::read(session_file(state, folder)).ok().and_then(|d| serde_json::from_slice(&d).ok()).unwrap_or_default()
}

/// Stores a folder's progress and remembers it as the folder being worked on.
pub fn store_session(state: &AppState, folder: &str, session: &serde_json::Value) -> CmdResult<()> {
    write_atomic(&session_file(state, folder), &serde_json::to_vec(session).map_err(err)?).map_err(err)?;
    write_atomic(&last_file(state), &serde_json::to_vec(folder).map_err(err)?).map_err(err)
}

/// The folder that was open when the app last stopped, if it still exists.
pub fn last_folder(state: &AppState) -> Option<String> {
    let folder: String = serde_json::from_slice(&fs::read(last_file(state)).ok()?).ok()?;
    Path::new(&folder).is_dir().then_some(folder)
}

pub fn forget_last(state: &AppState) {
    let _ = fs::remove_file(last_file(state));
}

#[tauri::command]
pub fn load_session(state: State<AppState>, folder: String) -> serde_json::Value {
    session_of(&state, &folder)
}

#[tauri::command]
pub fn save_session(state: State<AppState>, folder: String, session: serde_json::Value) -> CmdResult<()> {
    store_session(&state, &folder, &session)
}

#[tauri::command]
pub fn last_session(state: State<AppState>) -> Option<String> {
    last_folder(&state)
}

#[tauri::command]
pub fn clear_last_session(state: State<AppState>) {
    forget_last(&state)
}

#[derive(Serialize)]
pub struct AppInfo {
    platform: &'static str,
    mobile: bool,
    version: String,
    /// On phones and tablets photos live in the app's own Documents folder.
    library: Option<String>,
    cache_bytes: u64,
    /// Debug builds only: folder (and photo index) to open at launch, taken
    /// from `SIFT_OPEN` / `SIFT_VIEW`, so screens can be checked on simulators.
    dev_open: Option<String>,
    dev_view: Option<usize>,
    /// Whether Nikon's Image SDK can be used on this platform, and is installed.
    nikon_supported: bool,
    nikon_sdk: bool,
    /// Whether a burst can be written as a Live Photo here.
    live_photo: bool,
}

pub fn library_dir(app: &AppHandle) -> Option<PathBuf> {
    if !cfg!(mobile) {
        return None;
    }
    let dir = app.path().document_dir().or_else(|_| app.path().app_data_dir()).ok()?;
    let dir = if cfg!(target_os = "android") { dir.join("Sift") } else { dir };
    fs::create_dir_all(&dir).ok()?;
    Some(dir)
}

#[tauri::command]
pub async fn app_info(app: AppHandle) -> CmdResult<AppInfo> {
    blocking(move || {
        let state = app.state::<AppState>();
        let library = library_dir(&app);
        if let Some(dir) = &library {
            state.allow(dir);
        }
        let dev = |key: &str| std::env::var(key).ok().filter(|_| cfg!(debug_assertions));
        let dev_open = dev("SIFT_OPEN").map(|p| match &library {
            Some(root) if !Path::new(&p).is_absolute() => root.join(p).to_string_lossy().into_owned(),
            _ => p,
        });
        Ok(AppInfo {
            dev_open,
            dev_view: dev("SIFT_VIEW").and_then(|v| v.parse().ok()),
            platform: std::env::consts::OS,
            mobile: cfg!(mobile),
            version: app.package_info().version.to_string(),
            library: library.map(|d| d.to_string_lossy().into_owned()),
            cache_bytes: crate::state::dir_size(&state.cache_dir),
            nikon_supported: sift_core::nikon::SUPPORTED,
            nikon_sdk: state.nikon().is_some(),
            live_photo: cfg!(target_os = "macos"),
        })
    })
    .await
}

#[tauri::command]
pub async fn clear_cache(app: AppHandle) -> CmdResult<()> {
    blocking(move || {
        let state = app.state::<AppState>();
        let _ = fs::remove_dir_all(&state.cache_dir);
        fs::create_dir_all(&state.cache_dir).map_err(err)
    })
    .await
}

#[derive(Serialize)]
pub struct Album {
    path: String,
    name: String,
    count: usize,
    cover: Option<String>,
}

/// Sub-folders of the mobile library that contain photos.
#[tauri::command]
pub async fn list_albums(app: AppHandle) -> CmdResult<Vec<Album>> {
    blocking(move || {
        let Some(root) = library_dir(&app) else { return Ok(Vec::new()) };
        let state = app.state::<AppState>();
        let mut dirs: Vec<PathBuf> = fs::read_dir(&root).map_err(err)?.flatten().map(|e| e.path()).filter(|p| p.is_dir()).collect();
        dirs.insert(0, root.clone());
        let mut albums = Vec::new();
        for dir in dirs {
            let items = scan(&dir, true, false).unwrap_or_default();
            if items.is_empty() {
                continue;
            }
            state.allow(&dir);
            albums.push(Album {
                name: if dir == root { String::new() } else { dir.file_name().unwrap_or_default().to_string_lossy().into_owned() },
                path: dir.to_string_lossy().into_owned(),
                count: items.len(),
                cover: items.first().map(|i| i.id.clone()),
            });
        }
        albums.sort_by(|a, b| natural_cmp(&b.name, &a.name));
        Ok(albums)
    })
    .await
}

/// Copies files chosen in the system picker into a new album of the mobile library.
#[tauri::command]
pub async fn import_files(app: AppHandle, sources: Vec<String>, album: String) -> CmdResult<String> {
    blocking(move || {
        use std::io::{Read, Write};
        use tauri_plugin_fs::{FilePath, FsExt, OpenOptions};
        let root = library_dir(&app).ok_or("import is only used on mobile")?;
        let name: String = album.chars().filter(|c| !matches!(c, '/' | '\\' | ':' | '\0')).collect();
        let dir = root.join(if name.trim().is_empty() { "Imported" } else { name.trim() });
        fs::create_dir_all(&dir).map_err(err)?;
        let total = sources.len();
        for (i, src) in sources.iter().enumerate() {
            let source: FilePath = src.parse().map_err(err)?;
            let decoded = percent_encoding::percent_decode_str(src).decode_utf8_lossy().into_owned();
            let file_name = decoded.rsplit(['/', '\\', ':']).next().unwrap_or("photo").to_string();
            let _ = app.emit("task-progress", Progress { done: i, total, name: file_name.clone() });
            let mut opts = OpenOptions::new();
            opts.read(true);
            let mut input = app.fs().open(source, opts).map_err(err)?;
            let target = dir.join(&file_name);
            let mut output = fs::File::create(&target).map_err(err)?;
            let mut buf = vec![0u8; 1 << 20];
            loop {
                let n = input.read(&mut buf).map_err(err)?;
                if n == 0 {
                    break;
                }
                output.write_all(&buf[..n]).map_err(err)?;
            }
            drop(output);
            // Android pickers return opaque document ids; name the copy by its contents.
            if !formats::is_supported(&target) {
                match formats::sniff_ext(&target) {
                    Some(ext) => fs::rename(&target, dir.join(format!("IMG_{:04}.{ext}", i + 1))).map_err(err)?,
                    None => fs::remove_file(&target).map_err(err)?,
                }
            }
        }
        let _ = app.emit("task-progress", Progress { done: total, total, name: String::new() });
        app.state::<AppState>().allow(&dir);
        Ok(dir.to_string_lossy().into_owned())
    })
    .await
}
