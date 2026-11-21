use crate::protocol;
use crate::state::{path_hash, AppState};
use serde::{Deserialize, Serialize};
use sift_core::af::AfBox;
use sift_core::export::{self, CopyReport, Progress};
use sift_core::frame::FrameOptions;
use sift_core::{edit, face, formats, frame, hdr, mark, meta, preview, rating, remove, stack};
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
    /// The folder the shot is in below the opened one, `/` between the
    /// names; empty for the opened folder itself.
    pub dir: String,
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

fn is_movie(path: &Path) -> bool {
    path.extension().is_some_and(|x| x.eq_ignore_ascii_case("mov"))
}

/// The files of one shot have the same name, apart from the extension, in the same folder.
fn shot_key(path: &Path) -> String {
    path.with_extension("").to_string_lossy().to_ascii_lowercase()
}

/// How far below the opened folder photos are looked for.
const MAX_DEPTH: usize = 8;

/// Folders that hold no photos of the user's: packages of other programs
/// and what systems keep on a volume for themselves.
fn passed_over(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    let package = [".photoslibrary", ".lrdata", ".lrcat-data", ".cocatalog", ".app", ".bundle", ".fcpbundle", ".imovielibrary"];
    package.iter().any(|ext| lower.ends_with(ext)) || matches!(lower.as_str(), "$recycle.bin" | "system volume information" | "@eadir" | "#recycle" | "lost+found")
}

/// What a folder holds that matters here.
#[derive(Default)]
struct Listing {
    photos: Vec<PathBuf>,
    /// Movies are not listed; one named like a photo may be its Live Photo half.
    movies: HashMap<String, PathBuf>,
    /// Live Photo bundles.
    bundles: Vec<PathBuf>,
}

impl Listing {
    /// Reads `dir`, and the folders in it down to `depth` levels. Only the
    /// folder asked for has to be readable.
    fn read(&mut self, dir: &Path, depth: usize) -> std::io::Result<()> {
        for e in fs::read_dir(dir)?.flatten() {
            let p = e.path();
            if p.file_name().and_then(|n| n.to_str()).is_none_or(|n| n.starts_with('.')) {
                continue;
            }
            if is_movie(&p) {
                self.movies.insert(shot_key(&p), p);
            } else if sift_core::live::is_bundle(&p) {
                self.bundles.push(p);
            } else if formats::is_supported(&p) {
                // The directory listing usually knows the type already; asking again
                // costs a round trip per file on a network share. Links are followed.
                let file = match e.file_type() {
                    Ok(t) if t.is_file() => true,
                    Ok(t) if t.is_dir() => false,
                    _ => p.is_file(),
                };
                if file {
                    self.photos.push(p);
                }
            } else if depth > 0 && e.file_type().is_ok_and(|t| t.is_dir()) && !passed_over(&e.file_name().to_string_lossy()) {
                // Links to folders are not followed: they can lead back up.
                let _ = self.read(&p, depth - 1);
            }
        }
        Ok(())
    }
}

/// Changes when a photo, a movie or a Live Photo bundle appears in `dir` or
/// goes away (with `deep`, anywhere below it): a listing, no file is read.
pub fn folder_stamp(dir: &Path, deep: bool) -> CmdResult<String> {
    let mut found = Listing::default();
    found.read(dir, if deep { MAX_DEPTH } else { 0 }).map_err(err)?;
    let mut names: Vec<String> = found.photos.iter().chain(found.movies.values()).chain(&found.bundles).map(|p| p.to_string_lossy().into_owned()).collect();
    names.sort_unstable();
    let parts: Vec<&[u8]> = names.iter().map(|n| n.as_bytes()).collect();
    Ok(format!("{:016x}", crate::state::fnv1a(&parts)))
}

/// The folders between `root` and a file in it, outermost first.
fn folders_of<'a>(root: &Path, file: &'a Path) -> Vec<std::borrow::Cow<'a, str>> {
    // The still of a Live Photo bundle counts as being where the bundle is.
    let dir = file.parent().map(|dir| dir.parent().filter(|_| sift_core::live::is_bundle(dir)).unwrap_or(dir));
    let inside = dir.and_then(|dir| dir.strip_prefix(root).ok());
    inside.map(|dir| dir.components().map(|c| c.as_os_str().to_string_lossy()).collect()).unwrap_or_default()
}

/// The shots in `dir`, in name order; with `deep` also those in the folders
/// inside it, each folder's after the one it is in.
pub fn scan(dir: &Path, pair: bool, prefer_raw: bool, deep: bool) -> CmdResult<Vec<Item>> {
    let mut found = Listing::default();
    found.read(dir, if deep { MAX_DEPTH } else { 0 }).map_err(err)?;
    let Listing { photos: mut files, mut movies, bundles } = found;
    // A Live Photo bundle is listed as its still, with its movie attached like a loose one's.
    for (still, movie) in bundles.iter().filter_map(|b| sift_core::live::bundle_members(b)) {
        movies.insert(shot_key(&movie), movie);
        files.push(still);
    }
    let name_of = |p: &Path| p.file_name().unwrap_or_default().to_string_lossy().into_owned();
    files.sort_by(|a, b| {
        let (x, y) = (folders_of(dir, a), folders_of(dir, b));
        let folders = x.iter().zip(&y).map(|(x, y)| natural_cmp(x, y)).find(|o| o.is_ne()).unwrap_or(x.len().cmp(&y.len()));
        folders.then_with(|| natural_cmp(&name_of(a), &name_of(b)))
    });

    let mut groups: Vec<Vec<PathBuf>> = Vec::new();
    let mut by_stem: HashMap<String, usize> = HashMap::new();
    for f in files {
        match by_stem.get(&shot_key(&f)).filter(|_| pair) {
            Some(&i) => groups[i].push(f),
            None => {
                by_stem.insert(shot_key(&f), groups.len());
                groups.push(vec![f]);
            }
        }
    }
    for g in &mut groups {
        // The first file of a pair is the one displayed: the camera's JPEG,
        // or the RAW's embedded preview when the user prefers that.
        g.sort_by_key(|p| if prefer_raw { (!formats::is_raw(p), false) } else { (!formats::is_jpeg(p), formats::is_raw(p)) });
    }
    // One round trip each on a network share, so all at once.
    let stats = crate::briefs::each(&groups, crate::briefs::READERS, |g| stat(&g[0]));
    Ok(groups
        .into_iter()
        .zip(stats)
        .map(|(g, (size, mtime))| {
            let primary = &g[0];
            Item {
                id: primary.to_string_lossy().into_owned(),
                name: primary.file_name().unwrap_or_default().to_string_lossy().into_owned(),
                raw: g.iter().any(|p| formats::is_raw(p)),
                paired: g.len() > 1,
                dir: folders_of(dir, primary).join("/"),
                live: movies.get(&shot_key(primary)).and_then(|m| sift_core::live::movie_of(primary, m)).map(|m| m.to_string_lossy().into_owned()),
                paths: g.iter().map(|p| p.to_string_lossy().into_owned()).collect(),
                size,
                mtime,
            }
        })
        .collect())
}

#[tauri::command]
pub async fn scan_folder(app: AppHandle, path: String, pair: bool, prefer_raw: bool, deep: bool) -> CmdResult<Vec<Item>> {
    blocking(move || {
        let dir = PathBuf::from(&path);
        if !dir.is_dir() {
            return Err(format!("not a folder: {path}"));
        }
        app.state::<AppState>().allow(&dir);
        scan(&dir, pair, prefer_raw, deep)
    })
    .await
}

#[tauri::command]
pub async fn folder_changes(app: AppHandle, path: String, deep: bool) -> CmdResult<String> {
    blocking(move || folder_stamp(&app.state::<AppState>().check(&path)?, deep)).await
}

#[tauri::command]
pub async fn read_meta(app: AppHandle, path: String) -> CmdResult<meta::Meta> {
    blocking(move || meta::read(&app.state::<AppState>().check(&path)?).map_err(err)).await
}

pub use crate::briefs::{briefs_of, kept_of};

/// Shots that belong together, by the path they are displayed by.
#[derive(Serialize)]
pub struct Stack {
    pub kind: stack::Kind,
    /// In shooting order; a merged pixel-shift file comes first.
    pub ids: Vec<String>,
    /// Members that are another copy of an earlier exposure (the same file
    /// saved twice), each with the member it repeats.
    pub copies: HashMap<String, String>,
}

/// The copies among the members of a stack: files of the same exposure.
/// They were taken at the same instant (and have the same number in the
/// series, where the camera gives one) and are either the same size or named
/// after the original ("DSC_0080-1.JPG" beside "DSC_0080.JPG"; a rating
/// written into one of them changes its size). Frames of a very fast burst
/// share the instant, but neither the size nor the name.
fn copies_of(ids: &[String], hints: &HashMap<String, stack::Hint>) -> HashMap<String, String> {
    let mut seen: HashMap<(i64, Option<u32>), Vec<&String>> = HashMap::new();
    for id in ids {
        if let Some(time) = hints.get(id).and_then(|h| h.time_ms) {
            seen.entry((time, hints[id].seq)).or_default().push(id);
        }
    }
    let stem = |id: &str| Path::new(id).file_stem().map(|s| s.to_string_lossy().to_lowercase()).unwrap_or_default();
    let mut copies = HashMap::new();
    for mut same in seen.into_values().filter(|s| s.len() > 1) {
        // The plainest name first: that one is the original.
        same.sort_by_key(|id| (id.len(), (*id).clone()));
        let mut originals: Vec<(&String, u64, String)> = Vec::new();
        for id in same {
            let (size, name) = (stat(Path::new(id)).0, stem(id));
            match originals.iter().find(|(_, s, n)| *s == size || name.starts_with(n.as_str())) {
                Some((original, ..)) => {
                    copies.insert(id.clone(), (*original).clone());
                }
                None => originals.push((id, size, name)),
            }
        }
    }
    copies
}

/// Bursts and pixel-shift series among `paths` (the displayed file of every
/// shot in a folder). Files `read_briefs` has seen are not read again.
pub fn stacks_of(state: &AppState, paths: &[String]) -> Vec<Stack> {
    let unread: Vec<String> = {
        let series = state.series.lock().unwrap();
        paths.iter().filter(|p| !series.contains_key(*p)).cloned().collect()
    };
    if !unread.is_empty() {
        briefs_of(state, &unread, false);
    }
    let series = state.series.lock().unwrap();
    let hints: Vec<stack::Hint> = paths.iter().map(|p| series.get(p).cloned().unwrap_or_default()).collect();
    stack::group(&hints)
        .into_iter()
        .map(|s| {
            let ids: Vec<String> = s.members.into_iter().map(|i| paths[i].clone()).collect();
            Stack { kind: s.kind, copies: copies_of(&ids, &series), ids }
        })
        .collect()
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

/// Writes a burst as a Live Photo (a still and a movie of the same name that
/// belong together) next to its frames. `frames` are the displayed files in
/// shooting order and `key` is the one that becomes the still, a HEIC with
/// `heif` and a JPEG without; `turn` is the quarter turns the photos were
/// given while culling. Returns the path of the still.
pub fn live_photo(state: &AppState, frames: &[String], key: usize, turn: u8, heif: bool, on_frame: impl FnMut(usize)) -> CmdResult<String> {
    use sift_core::live::Still;
    let files = frames.iter().map(|p| state.check(p)).collect::<Result<Vec<_>, _>>()?;
    let key = key.min(files.len().saturating_sub(1));
    let cover = files.get(key).ok_or("no frames")?;
    let dest = cover.parent().ok_or("no folder")?;
    let base = cover.file_stem().map_or_else(|| "Live".into(), |s| s.to_string_lossy().into_owned());
    // Never over an existing file: the pair gets a name of its own, also
    // clear of an older Live Photo saved as a `.pvt` bundle.
    let taken_name = |stem: &str| Still::EXTS.iter().chain(&["MOV", sift_core::live::BUNDLE_EXT]).any(|ext| dest.join(format!("{stem}.{ext}")).exists());
    let stem = (1..)
        .map(|n| if n == 1 { format!("{base}_live") } else { format!("{base}_live_{n}") })
        .find(|stem| !taken_name(stem))
        .unwrap_or_default();
    let taken = taken_at(state, frames);
    state.cancel.store(false, AtomicOrdering::Relaxed);
    let format = if heif { Still::Heic } else { Still::Jpeg };
    let (still, _) = sift_core::live::export(&files, &taken, key, turn, format, dest, &stem, &state.cancel, on_frame).map_err(err)?;
    Ok(still.to_string_lossy().into_owned())
}

#[tauri::command]
pub async fn save_live_photo(app: AppHandle, frames: Vec<String>, key: usize, turn: Option<u8>, heif: bool) -> CmdResult<String> {
    blocking(move || {
        let total = frames.len();
        let mut emit = progress(&app);
        live_photo(&app.state::<AppState>(), &frames, key, turn.unwrap_or(0), heif, |done| emit(Progress { done, total, name: String::new() }))
    })
    .await
}

#[tauri::command]
pub async fn read_briefs(app: AppHandle, paths: Vec<String>, fresh: Option<bool>) -> CmdResult<Vec<meta::Brief>> {
    blocking(move || Ok(briefs_of(&app.state::<AppState>(), &paths, fresh.unwrap_or(false)))).await
}

/// Starts making the grid's thumbnails of `paths` ahead, in the background;
/// a run started before stops (none is started for no paths).
#[tauri::command]
pub fn warm_thumbs(app: AppHandle, paths: Vec<String>, edge: u32) {
    use std::sync::atomic::Ordering::Relaxed;
    let run = app.state::<AppState>().warming.fetch_add(1, Relaxed) + 1;
    if paths.is_empty() {
        return;
    }
    std::thread::spawn(move || {
        let state = app.state::<AppState>();
        protocol::warm_thumbs(&state, &paths, edge, || state.warming.load(Relaxed) != run);
    });
}

#[tauri::command]
pub async fn kept_briefs(app: AppHandle, paths: Vec<String>) -> CmdResult<Vec<Option<meta::Brief>>> {
    blocking(move || Ok(kept_of(&app.state::<AppState>(), &paths))).await
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

/// Deletes shots, each given as its files (a Live Photo's movie among them):
/// into the system's trash, or with `for_good` at once and without a way back.
pub fn delete_shots(state: &AppState, shots: &[Vec<String>], for_good: bool) -> CmdResult<remove::Outcome> {
    let shots = checked_shots(state, shots)?;
    // Windows does not move a file that is open, and Nikon's helper keeps the photo it developed last.
    if cfg!(windows) {
        if let Some(engine) = state.nikon() {
            engine.release(&shots.concat());
        }
    }
    Ok(remove::remove(&shots, for_good))
}

#[tauri::command]
pub async fn delete_photos(app: AppHandle, shots: Vec<Vec<String>>, for_good: bool) -> CmdResult<remove::Outcome> {
    blocking(move || delete_shots(&app.state::<AppState>(), &shots, for_good)).await
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
    let img = protocol::pixels(state, &file, EYE_SEARCH_EDGE).map_err(err)?;
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

/// A crop as it comes from the webview: normalised `[x, y, width, height]`
/// of the photo as turned, then the angle in degrees (clockwise) it is
/// straightened by, when it is.
pub type Crop = Vec<f32>;

fn crop_parts(c: &Crop) -> Option<([f32; 4], f32)> {
    let rect: [f32; 4] = c.get(..4)?.try_into().ok()?;
    Some((rect, c.get(4).copied().unwrap_or(0.0)))
}

#[derive(Deserialize, Clone, Copy, Default, PartialEq, Eq, Debug)]
#[serde(rename_all = "lowercase")]
pub enum MarkKind {
    #[default]
    Picture,
    Text,
}

/// The watermark on exported pictures: a picture the app keeps (`picture`,
/// see [keep_watermark]; its plain ground taken out with `knockout`) or
/// text with fields in Inter at `weight`, and how it is laid.
#[derive(Deserialize, Clone, Debug)]
#[serde(default)]
pub struct Mark {
    pub kind: MarkKind,
    pub picture: String,
    pub knockout: bool,
    pub text: String,
    pub weight: f32,
    #[serde(flatten)]
    pub layout: mark::Layout,
}

impl Default for Mark {
    fn default() -> Mark {
        Mark { kind: MarkKind::Picture, picture: String::new(), knockout: false, text: String::new(), weight: 450.0, layout: mark::Layout::default() }
    }
}

/// How exported pictures come out.
#[derive(Deserialize, Clone)]
#[serde(default)]
pub struct Output {
    /// Long edge in pixels; 0 keeps the size.
    pub size: u32,
    pub quality: u8,
    /// Leave out what the camera recorded (EXIF, XMP, place).
    pub strip: bool,
    pub watermark: Option<Mark>,
}

impl Default for Output {
    fn default() -> Self {
        Output { size: 0, quality: FRAMED_QUALITY, strip: false, watermark: None }
    }
}

/// Where the watermark picture `id` is kept; the empty id is the one
/// picture earlier versions kept.
fn watermark_file(state: &AppState, id: &str) -> CmdResult<PathBuf> {
    if id.is_empty() {
        return Ok(state.data_dir.join("watermark.png"));
    }
    if id.len() != 16 || !id.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(format!("watermark: no picture {id}"));
    }
    Ok(state.data_dir.join("watermarks").join(format!("{id}.png")))
}

/// The watermark the output asks for, ready to be laid.
fn mark_of(state: &AppState, output: &Output) -> CmdResult<Option<mark::Watermark>> {
    let Some(m) = &output.watermark else { return Ok(None) };
    Ok(match m.kind {
        MarkKind::Text => mark::Watermark::text(&m.text, m.weight, m.layout.clone()),
        MarkKind::Picture => {
            let img = image::open(watermark_file(state, &m.picture)?).map_err(|e| format!("watermark: {e}"))?.to_rgba8();
            let img = if m.knockout { mark::knocked_out(img) } else { img };
            Some(mark::Watermark::picture(&img, m.layout.clone()))
        }
    })
}

/// Upright full-size pixels of a shot with its look applied.
fn looked_pixels(state: &AppState, shot: &[PathBuf], look: Option<&String>) -> CmdResult<image::RgbImage> {
    match look {
        Some(look) => protocol::look_pixels(state, look_source(shot).ok_or("empty shot")?, look, 0),
        None => {
            let path = shot.first().ok_or("empty shot")?;
            best_pixels(state, path, &meta::read(path).map_err(err)?).map_err(err)
        }
    }
}

/// Upright pixels the way the user set them up while culling: turned, then
/// straightened and cut to the crop.
fn shaped(img: image::RgbImage, turn: u8, crop: Option<([f32; 4], f32)>) -> image::RgbImage {
    let img = edit::turn(img, turn);
    match crop {
        Some((rect, angle)) => edit::cut(&img, rect, angle),
        None => img,
    }
}

/// Made the size the output asks for, and watermarked; text on the mark is
/// filled in from `source`, the photo's file.
fn finished(img: image::RgbImage, output: &Output, mark: Option<&mark::Watermark>, source: &Path) -> image::RgbImage {
    let mut img = edit::shrink(img, output.size);
    if let Some(mark) = mark {
        marked(&mut img, mark, source);
    }
    img
}

fn marked(img: &mut image::RgbImage, mark: &mark::Watermark, source: &Path) {
    let exif = if mark.needs_exif() { meta::read(source).map(|m| m.exif).unwrap_or_default() } else { Default::default() };
    mark.apply(img, &exif, &source.file_stem().unwrap_or_default().to_string_lossy());
}

/// An HDR photo exported as one: turned, cut to its crop, made the size the
/// output asks for and watermarked as [`shaped`] and [`finished`] do, then
/// encoded as a JPEG with a gain map. `None` when `path` is no HDR photo
/// this build reads.
fn finished_hdr(path: &Path, turn: u8, crop: Option<([f32; 4], f32)>, output: &Output, mark: Option<&mark::Watermark>) -> CmdResult<Option<Vec<u8>>> {
    let Some(picture) = hdr::picture(path, u32::MAX).map_err(err)? else { return Ok(None) };
    let mut picture = picture.map(|img| shaped(img, turn, crop)).shrink(output.size);
    if let Some(mark) = mark {
        let before = picture.base.clone();
        marked(&mut picture.base, mark, path);
        picture.settle(&before);
    }
    let quality = output.quality.clamp(1, 100);
    picture.encode(quality, |jpeg| if output.strip { jpeg } else { sift_core::live::described(jpeg, path) }).map(Some).map_err(err)
}

/// A JPEG of `img` at the output's quality, carrying what `source` records
/// about the shot unless the output leaves that out.
fn encoded(img: &image::RgbImage, output: &Output, source: Option<&Path>) -> CmdResult<Vec<u8>> {
    let jpeg = preview::encode_jpeg(img, output.quality.clamp(1, 100)).map_err(err)?;
    Ok(match source {
        Some(source) if !output.strip => sift_core::live::described(jpeg, source),
        _ => jpeg,
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
    /// First path of a shot -> crop.
    #[serde(default)]
    pub crops: HashMap<String, Crop>,
    /// First path of a shot -> quarter turns clockwise.
    #[serde(default)]
    pub turns: HashMap<String, u8>,
    #[serde(default)]
    pub output: Output,
    /// Sorted by tag: one folder for each of these inside the destination.
    #[serde(default)]
    pub folders: Vec<export::TagFolder>,
}

/// What becomes of a shot on export.
enum Job {
    /// Its files are copied as they are.
    Plain,
    /// Its JPEG is copied with another orientation or without its metadata;
    /// the picture is not encoded again.
    Rewrite(PathBuf),
    /// A JPEG is rendered of it.
    Render,
}

/// A copy of a JPEG turned on by `turn` quarter turns (0 to 3) and, with
/// `strip`, without its metadata; `None` when that cannot be done without
/// encoding it again.
fn rewritten(data: &[u8], turn: u8, strip: bool) -> Option<Vec<u8>> {
    let o = edit::turned_orientation(edit::orientation_of(data), turn);
    if strip {
        // A gain map is no metadata: it is the photo's HDR.
        edit::stripped(data, o).map(|bare| hdr::kept(bare, data))
    } else if turn != 0 {
        edit::reoriented(data, o)
    } else {
        Some(data.to_vec())
    }
}

/// A shot of an export that is made anew rather than copied.
struct Made<'a> {
    /// Where it is among the shots of its folder, and among all.
    n: usize,
    i: usize,
    shot: &'a [PathBuf],
    look: Option<&'a String>,
    crop: Option<([f32; 4], f32)>,
    turn: u8,
    job: Job,
}

/// Photos rendered at once on export. Each holds a few copies of the
/// full-size picture, and the work on its pixels runs on every core already;
/// reading, decoding, encoding and writing them gains from overlapping.
fn renderers() -> usize {
    std::thread::available_parallelism().map_or(2, |n| n.get() / 2).clamp(1, 4)
}

/// Progress of one of several folders as progress of the whole export.
fn overall(p: Progress, part: usize, parts: usize) -> Progress {
    const STEPS: usize = 1000;
    if parts < 2 {
        return p;
    }
    Progress { done: part * STEPS + p.done * STEPS / p.total.max(1), total: parts * STEPS, name: p.name }
}

/// The file a look is rendered from: the RAW of a shot if it has one.
fn look_source(shot: &[PathBuf]) -> Option<&PathBuf> {
    shot.iter().find(|p| formats::is_raw(p)).or(shot.first())
}

/// Copies the chosen kind of file of every shot, optionally renamed. Shots
/// with a look or crop assigned (keyed by the first path of the shot) get a
/// JPEG rendered that way instead of a copy of the camera's JPEG.
pub fn export_shots(state: &AppState, shots: &[Vec<String>], dest: &str, plan: &ExportPlan, mut on_progress: impl FnMut(Progress) + Send) -> CmdResult<CopyReport> {
    let ExportPlan { kind, looks, crops, turns, output, .. } = plan;
    let kind = *kind;
    let all = checked_shots(state, shots)?;
    let mark = if kind == export::Kind::Raw { None } else { mark_of(state, output)? };
    state.cancel.store(false, AtomicOrdering::Relaxed);
    let parts = export::parts(Path::new(dest), &plan.folders, plan.rename.as_ref(), all.len());
    let mut report = CopyReport::default();
    // A shot that goes into several folders is rendered once; the others get a copy of that file.
    let mut rendered: HashMap<usize, PathBuf> = HashMap::new();
    for (k, part) in parts.iter().enumerate() {
        let picked: Vec<Vec<PathBuf>> = part.shots.iter().map(|i| all[*i].clone()).collect();
        let stems = export::stems(&picked, part.rename.as_ref());
        fs::create_dir_all(&part.dir).map_err(err)?;
        let mut step = |p: Progress| on_progress(overall(p, k, parts.len()));
        // Files that are copied as they are, with the name their shot was given.
        let (mut plain, mut names) = (Vec::new(), Vec::new());
        // The shots made anew, rendered or rewritten.
        let mut work = Vec::new();
        for (n, (&i, shot)) in part.shots.iter().zip(&picked).enumerate() {
            let first = shots[i].first();
            let look = first.and_then(|p| looks.get(p));
            let crop = first.and_then(|p| crops.get(p)).and_then(crop_parts);
            let turn = first.and_then(|p| turns.get(p)).map_or(0, |t| t % 4);
            // RAW files are never changed; anything else that cannot be done to the JPEG's bytes is rendered.
            let picture = shot.iter().find(|p| !formats::is_raw(p) && formats::is_supported(p));
            let job = if kind == export::Kind::Raw {
                Job::Plain
            } else if look.is_some() || crop.is_some() || output.size > 0 || mark.is_some() {
                Job::Render
            } else if turn == 0 && !output.strip {
                Job::Plain
            } else {
                match picture {
                    Some(p) if formats::is_jpeg(p) => Job::Rewrite(p.clone()),
                    Some(_) => Job::Render,
                    None if turn != 0 => Job::Render,
                    None => Job::Plain,
                }
            };
            if matches!(job, Job::Plain) {
                plain.push(shot.clone());
                names.push(stems[n].clone());
            } else {
                work.push(Made { n, i, shot, look, crop, turn, job });
            }
        }
        let started = std::sync::Mutex::new((0, &mut step));
        let made = crate::briefs::each(&work, renderers(), |w| -> Option<(PathBuf, CmdResult<()>)> {
            if state.cancel.load(AtomicOrdering::Relaxed) {
                return None;
            }
            {
                let mut started = started.lock().unwrap();
                let done = started.0;
                (started.1)(Progress { done, total: picked.len(), name: stems[w.n].clone() });
                started.0 += 1;
            }
            let Made { n, i, shot, look, crop, turn, ref job } = *w;
            let ext = |p: &Path| p.extension().and_then(|e| e.to_str()).unwrap_or("jpg").to_string();
            let (target, made) = match job {
                Job::Rewrite(jpeg) => {
                    let target = part.dir.join(format!("{}.{}", stems[n], ext(jpeg)));
                    let bytes = fs::read(jpeg).map_err(err).map(|data| rewritten(&data, turn, output.strip));
                    match bytes {
                        Ok(Some(bytes)) => (target, Ok(bytes)),
                        // An EXIF block without an orientation in it: rendered instead.
                        Ok(None) => (part.dir.join(format!("{}.jpg", stems[n])), Err(None)),
                        Err(e) => (target, Err(Some(e))),
                    }
                }
                _ => (part.dir.join(format!("{}.jpg", stems[n])), Err(None)),
            };
            let written = match (rendered.get(&i), made) {
                (Some(first), _) => export::copy_file(first, &target, &state.cancel).map_err(err),
                (None, Ok(bytes)) => fs::write(&target, bytes).map_err(err),
                (None, Err(Some(e))) => Err(e),
                (None, Err(None)) => {
                    // A look is made for ordinary pictures: a shot with one comes out as one.
                    let hdr = match (look, shot.first()) {
                        (None, Some(first)) if hdr::kind(first).is_some() => finished_hdr(first, turn, crop, output, mark.as_ref()),
                        _ => Ok(None),
                    };
                    match hdr {
                        Ok(Some(jpeg)) => Ok(jpeg),
                        Ok(None) => looked_pixels(state, shot, look)
                            .map(|img| finished(shaped(img, turn, crop), output, mark.as_ref(), shot.first().map_or(Path::new(""), PathBuf::as_path)))
                            .and_then(|img| encoded(&img, output, look_source(shot).map(PathBuf::as_path))),
                        Err(e) => Err(e),
                    }
                    .and_then(|jpeg| fs::write(&target, jpeg).map_err(err))
                }
            };
            Some((target, written))
        });
        drop(started);
        if state.cancel.load(AtomicOrdering::Relaxed) {
            report.cancelled = true;
            return Ok(report);
        }
        for (w, made) in work.iter().zip(made) {
            let Some((target, written)) = made else { continue };
            match written {
                Ok(()) => {
                    report.copied += 1;
                    rendered.entry(w.i).or_insert(target);
                }
                Err(e) => report.failed.push(format!("{}: {e}", stems[w.n])),
            }
            // The other files travel along unchanged: the RAW when RAW files are wanted, and next to
            // a JPEG copied as it was, the movie of a Live Photo.
            let rest: Vec<PathBuf> = w
                .shot
                .iter()
                .filter(|p| match &w.job {
                    Job::Rewrite(jpeg) => *p != jpeg,
                    _ => formats::is_raw(p) && kind == export::Kind::All,
                })
                .cloned()
                .collect();
            if !rest.is_empty() {
                plain.push(rest);
                names.push(stems[w.n].clone());
            }
        }
        let copied = export::export_named(&plain, &names, &part.dir, kind, &state.cancel, step).map_err(err)?;
        report.copied += copied.copied;
        report.skipped += copied.skipped;
        report.failed.extend(copied.failed);
        report.cancelled |= copied.cancelled;
        if report.cancelled {
            break;
        }
    }
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
    /// File -> crop.
    #[serde(default)]
    crops: HashMap<String, Crop>,
    /// File -> quarter turns clockwise.
    #[serde(default)]
    turns: HashMap<String, u8>,
    /// Sorted by tag: one folder for each of these inside the destination.
    #[serde(default)]
    folders: Vec<export::TagFolder>,
    /// Quality, metadata and watermark; a size here outranks `max_edge`.
    #[serde(default)]
    output: Output,
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
pub fn frame_files(state: &AppState, request: &FramedRequest, on_progress: impl FnMut(Progress) + Send) -> CmdResult<CopyReport> {
    state.cancel.store(false, AtomicOrdering::Relaxed);
    let mut report = CopyReport::default();
    let shots: Vec<Vec<String>> = request.files.iter().map(|f| vec![f.clone()]).collect();
    let all = checked_shots(state, &shots)?;
    let parts = export::parts(Path::new(&request.dest), &request.folders, request.rename.as_ref(), all.len());
    let total: usize = parts.iter().map(|part| part.shots.len()).sum();
    let output = Output { size: if request.output.size > 0 { request.output.size } else { request.max_edge }, ..request.output.clone() };
    let mark = mark_of(state, &output)?;
    let started = std::sync::Mutex::new((0, on_progress));
    // A photo that goes into several folders is framed once; the others get a copy of that file.
    let mut framed: HashMap<usize, PathBuf> = HashMap::new();
    for part in &parts {
        fs::create_dir_all(&part.dir).map_err(err)?;
        let picked: Vec<Vec<PathBuf>> = part.shots.iter().map(|i| all[*i].clone()).collect();
        let stems = export::stems(&picked, part.rename.as_ref());
        let jobs: Vec<(usize, usize)> = part.shots.iter().copied().enumerate().collect();
        let made = crate::briefs::each(&jobs, renderers(), |&(n, i)| -> Option<(PathBuf, sift_core::Result<()>)> {
            if state.cancel.load(AtomicOrdering::Relaxed) {
                return None;
            }
            let (f, path) = (&request.files[i], &all[i][0]);
            {
                let mut started = started.lock().unwrap();
                let done = started.0;
                (started.1)(Progress { done, total, name: path.file_name().unwrap_or_default().to_string_lossy().into_owned() });
                started.0 += 1;
            }
            let target = part.dir.join(if part.rename.is_some() { format!("{}.jpg", stems[n]) } else { format!("{}_framed.jpg", stems[n]) });
            let result = match framed.get(&i) {
                Some(first) => export::copy_file(first, &target, &state.cancel),
                None => (|| -> sift_core::Result<()> {
                    let m = meta::read(path)?;
                    let mut img = match request.looks.get(f) {
                        Some(look) => {
                            let source = request.sources.get(f).and_then(|s| state.check(s).ok()).unwrap_or_else(|| path.clone());
                            protocol::look_pixels(state, &source, look, 0).map_err(sift_core::Error::Decode)?
                        }
                        None => best_pixels(state, path, &m)?,
                    };
                    let turn = request.turns.get(f).map_or(0, |t| t % 4);
                    img = finished(shaped(img, turn, request.crops.get(f).and_then(crop_parts)), &output, mark.as_ref(), path);
                    let picture = frame::render(&img, &m.exif, &request.options);
                    let jpeg = encoded(&picture, &output, Some(path)).map_err(sift_core::Error::Decode)?;
                    fs::write(&target, jpeg)?;
                    Ok(())
                })(),
            };
            Some((target, result))
        });
        for (&(_, i), made) in jobs.iter().zip(made) {
            let Some((target, result)) = made else { continue };
            match result {
                Ok(()) => {
                    report.copied += 1;
                    framed.entry(i).or_insert(target);
                }
                Err(e) => report.failed.push(format!("{}: {e}", all[i][0].file_name().unwrap_or_default().to_string_lossy())),
            }
        }
        if state.cancel.load(AtomicOrdering::Relaxed) {
            report.cancelled = true;
            break;
        }
    }
    let (_, mut on_progress) = started.into_inner().unwrap();
    on_progress(Progress { done: total, total, name: String::new() });
    Ok(report)
}

#[tauri::command]
pub async fn export_framed(app: AppHandle, request: FramedRequest) -> CmdResult<CopyReport> {
    blocking(move || frame_files(&app.state::<AppState>(), &request, progress(&app))).await
}

/// Long edge of the export sheet's preview.
const PREVIEW_EDGE: u32 = 1200;

/// A small rendition of a photo as it will be exported, for the export
/// sheet's preview, as JPEG bytes: turned, cut to its crop, watermarked
/// and, given frame options, framed.
pub fn export_preview(state: &AppState, path: &str, options: Option<&FrameOptions>, output: &Output, turn: u8, crop: Option<&Crop>) -> CmdResult<Vec<u8>> {
    let file = state.check(path)?;
    let img = shaped(protocol::pixels(state, &file, PREVIEW_EDGE).map_err(err)?, turn % 4, crop.and_then(crop_parts));
    let shown = Output { size: PREVIEW_EDGE, ..output.clone() };
    let mut img = finished(img, &shown, mark_of(state, output)?.as_ref(), &file);
    if let Some(options) = options {
        img = frame::render(&img, &meta::read(&file).map_err(err)?.exif, options);
    }
    preview::encode_jpeg(&img, 88).map_err(err)
}

#[tauri::command]
pub async fn preview_export(app: AppHandle, path: String, options: Option<FrameOptions>, output: Output, turn: u8, crop: Option<Crop>) -> CmdResult<tauri::ipc::Response> {
    blocking(move || export_preview(&app.state::<AppState>(), &path, options.as_ref(), &output, turn, crop.as_ref()).map(tauri::ipc::Response::new)).await
}

/// A watermark picture the app keeps: its id, and whether it has no
/// transparency but a plain ground that can be taken out.
#[derive(Serialize)]
pub struct KeptMark {
    id: String,
    backdrop: bool,
}

/// Longest edge a watermark picture is kept at: more than a mark across a
/// whole large photo needs.
const MARK_EDGE: u32 = 4096;

/// Keeps a picture (PNG, JPEG, WebP, TIFF, BMP or GIF) as a watermark, as a
/// PNG named after what it holds, so that choosing it again keeps one copy.
pub fn keep_watermark(state: &AppState, data: &[u8]) -> CmdResult<KeptMark> {
    let img = image::load_from_memory(data).map_err(|e| format!("watermark: {e}"))?;
    let img = if img.width().max(img.height()) > MARK_EDGE { img.resize(MARK_EDGE, MARK_EDGE, image::imageops::FilterType::Lanczos3) } else { img };
    let img = img.to_rgba8();
    let mut png = Vec::new();
    img.write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png).map_err(err)?;
    // FNV-1a: a name that stays the same for the same picture.
    let hash = png.iter().fold(0xcbf2_9ce4_8422_2325u64, |h, &b| (h ^ b as u64).wrapping_mul(0x100_0000_01b3));
    let id = format!("{hash:016x}");
    let file = watermark_file(state, &id)?;
    if !file.exists() {
        write_atomic(&file, &png).map_err(err)?;
    }
    Ok(KeptMark { id, backdrop: mark::backdrop(&img).is_some() })
}

/// The watermark picture `id` as PNG bytes; none when there is none.
pub fn watermark_png(state: &AppState, id: &str) -> Option<Vec<u8>> {
    fs::read(watermark_file(state, id).ok()?).ok()
}

/// What is in a file the file dialog gave (a path or a URL).
fn picked_bytes(app: &AppHandle, source: &str) -> CmdResult<Vec<u8>> {
    use std::io::Read;
    use tauri_plugin_fs::{FilePath, FsExt, OpenOptions};
    let mut opts = OpenOptions::new();
    opts.read(true);
    let mut data = Vec::new();
    app.fs().open(source.parse::<FilePath>().map_err(err)?, opts).map_err(err)?.read_to_end(&mut data).map_err(err)?;
    Ok(data)
}

/// Keeps the picture at `source` (from the file dialog) as a watermark, or
/// `data`, a PNG the webview drew (of an SVG).
#[tauri::command]
pub async fn set_watermark(app: AppHandle, source: Option<String>, data: Option<Vec<u8>>) -> CmdResult<KeptMark> {
    blocking(move || {
        let data = match (data, source) {
            (Some(data), _) => data,
            (None, Some(source)) => picked_bytes(&app, &source)?,
            (None, None) => return Err("watermark: nothing chosen".into()),
        };
        keep_watermark(&app.state::<AppState>(), &data)
    })
    .await
}

/// The text of an SVG from the file dialog, for the webview to draw.
#[tauri::command]
pub async fn read_svg(app: AppHandle, source: String) -> CmdResult<String> {
    blocking(move || {
        if !source.to_ascii_lowercase().ends_with(".svg") {
            return Err("not an SVG".into());
        }
        String::from_utf8(picked_bytes(&app, &source)?).map_err(err)
    })
    .await
}

/// The watermark picture `id` as PNG bytes, none when there is none.
#[tauri::command]
pub async fn watermark(app: AppHandle, id: String) -> CmdResult<tauri::ipc::Response> {
    blocking(move || Ok(tauri::ipc::Response::new(watermark_png(&app.state::<AppState>(), &id).unwrap_or_default()))).await
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
    /// Whether photos can be saved to the system photo library (iOS).
    save_to_photos: bool,
    /// Whether this build updates itself (official desktop builds).
    updater: bool,
    /// Whether deleted photos go into the system's trash; without one they are deleted for good.
    trash: bool,
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
            live_photo: cfg!(desktop) && sift_core::live::EXPORT,
            save_to_photos: crate::photos::SUPPORTED,
            updater: crate::update::ENABLED,
            trash: cfg!(desktop) && remove::TRASH,
        })
    })
    .await
}

#[tauri::command]
pub async fn clear_cache(app: AppHandle) -> CmdResult<()> {
    blocking(move || {
        let state = app.state::<AppState>();
        state.briefs.clear();
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
            let items = scan(&dir, true, false, false).unwrap_or_default();
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

/// A photo to save to the system photo library, with its Live Photo movie.
#[derive(Deserialize)]
pub struct ToPhotos {
    still: String,
    movie: Option<String>,
}

/// Saves photos to the system photo library, each with its movie as one Live
/// Photo. Returns how many were saved; stops at the first failure, reported
/// as `denied` when the user has not allowed it.
#[tauri::command]
pub async fn save_to_photos(app: AppHandle, items: Vec<ToPhotos>) -> CmdResult<usize> {
    blocking(move || {
        let state = app.state::<AppState>();
        let total = items.len();
        let mut emit = progress(&app);
        for (done, item) in items.iter().enumerate() {
            let still = state.check(&item.still)?;
            let movie = item.movie.as_deref().map(|m| state.check(m)).transpose()?;
            emit(Progress { done, total, name: still.file_name().unwrap_or_default().to_string_lossy().into_owned() });
            crate::photos::save(&still, movie.as_deref()).map_err(|e| match e {
                crate::photos::Failure::Denied => "denied".to_string(),
                crate::photos::Failure::Other(message) => message,
            })?;
        }
        emit(Progress { done: total, total, name: String::new() });
        Ok(total)
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
            // Android pickers return opaque document ids; name the copy by its
            // contents. A movie stays: it may be a Live Photo's other half.
            if !formats::is_supported(&target) && !is_movie(&target) {
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

#[cfg(test)]
mod tests {
    use super::*;

    fn touch(root: &Path, names: &[&str]) {
        for name in names {
            let file = root.join(name);
            fs::create_dir_all(file.parent().unwrap()).unwrap();
            fs::write(file, b"x").unwrap();
        }
    }

    #[test]
    fn folders_inside_are_listed_only_when_asked_for() {
        let root = std::env::temp_dir().join(format!("sift-scan-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        touch(&root, &["b.jpg", "a/IMG_10.JPG", "a/IMG_9.JPG", "a/IMG_9.CR3", "a/deep/x.png", "a 2/y.jpg", ".hidden/z.jpg", "Photos.photoslibrary/m.jpg", "notes.txt"]);
        let names = |deep| -> Vec<String> {
            let shots = scan(&root, true, false, deep).unwrap();
            shots.into_iter().map(|i| if i.dir.is_empty() { i.name } else { format!("{}/{}", i.dir, i.name) }).collect()
        };
        assert_eq!(names(false), ["b.jpg"]);
        // The opened folder first, then each folder with the ones inside it after its own files.
        assert_eq!(names(true), ["b.jpg", "a/IMG_9.JPG", "a/IMG_10.JPG", "a/deep/x.png", "a 2/y.jpg"]);
        let pair = scan(&root, true, false, true).unwrap().remove(1);
        assert!(pair.paired && pair.raw && pair.paths.len() == 2);
        let inside = root.join("a").join("IMG_1.pvt").join("IMG_1.JPG");
        assert_eq!(folders_of(&root, &inside), ["a"], "a bundle is not a folder of photos");

        let (flat, deep) = (folder_stamp(&root, false).unwrap(), folder_stamp(&root, true).unwrap());
        touch(&root, &["a/new.jpg", "more.txt"]);
        assert_eq!(folder_stamp(&root, false).unwrap(), flat, "nothing new in the folder itself");
        assert_ne!(folder_stamp(&root, true).unwrap(), deep);
        fs::remove_file(root.join("b.jpg")).unwrap();
        assert_ne!(folder_stamp(&root, false).unwrap(), flat);
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn a_watermark_set_up_before_still_reads() {
        let old: Mark = serde_json::from_str(r#"{"place":"sw","size":0.3,"opacity":0.5}"#).unwrap();
        assert_eq!((old.kind, old.picture.as_str(), old.layout.place, old.layout.size, old.layout.opacity), (MarkKind::Picture, "", mark::Place::Sw, 0.3, 0.5));
        assert_eq!(old.layout.margin, mark::Layout::default().margin);
        let text: Mark = serde_json::from_str(r##"{"kind":"text","text":"© {year}","weight":700,"place":"c","at":[0.2,0.8],"tint":"#ffffff","tile":true,"angle":-30}"##).unwrap();
        assert_eq!((text.kind, text.weight, text.layout.at, text.layout.tint, text.layout.tile), (MarkKind::Text, 700.0, Some([0.2, 0.8]), mark::Tint::Rgb([255; 3]), true));
    }
}
