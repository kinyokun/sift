//! Handing photos to other programs: opening them in an editor the user
//! chose, and dragging their files out of the window.

use crate::state::AppState;
use std::path::{Path, PathBuf};
use tauri::{AppHandle, Manager};

/// Opens `paths` (files of opened folders) in the program at `app`: an
/// `.app` bundle on macOS, an `.exe` on Windows.
#[tauri::command]
pub fn open_with(app: AppHandle, paths: Vec<String>, program: String) -> Result<(), String> {
    let state = app.state::<AppState>();
    let files = paths.iter().map(|p| state.check(p)).collect::<Result<Vec<PathBuf>, _>>()?;
    if files.is_empty() {
        return Ok(());
    }
    launch(Path::new(&program), &files).map_err(|e| format!("{program}: {e}"))
}

#[cfg(target_os = "macos")]
fn launch(program: &Path, files: &[PathBuf]) -> std::io::Result<()> {
    if program.extension().is_none_or(|e| e != "app") || !program.is_dir() {
        return Err(std::io::Error::other("not an application"));
    }
    let status = std::process::Command::new("/usr/bin/open").arg("-a").arg(program).arg("--").args(files).status()?;
    if status.success() { Ok(()) } else { Err(std::io::Error::other("could not open the files")) }
}

#[cfg(windows)]
fn launch(program: &Path, files: &[PathBuf]) -> std::io::Result<()> {
    if program.extension().is_none_or(|e| !e.eq_ignore_ascii_case("exe")) || !program.is_file() {
        return Err(std::io::Error::other("not a program"));
    }
    std::process::Command::new(program).args(files).spawn().map(|_| ())
}

#[cfg(not(any(target_os = "macos", windows)))]
fn launch(program: &Path, files: &[PathBuf]) -> std::io::Result<()> {
    if !program.is_file() {
        return Err(std::io::Error::other("not a program"));
    }
    std::process::Command::new(program).args(files).spawn().map(|_| ())
}

/// Long edge of the picture that follows the pointer while files are dragged out.
#[cfg(any(target_os = "macos", windows))]
const DRAG_ICON_EDGE: u32 = 128;

/// Starts dragging `paths` out of the window, shown as a small picture of
/// `icon` turned by `turn` quarter turns. Called while the mouse button is
/// down; the system takes the drag from there.
#[cfg(any(target_os = "macos", windows))]
#[tauri::command]
pub async fn drag_out(window: tauri::WebviewWindow, paths: Vec<String>, icon: String, turn: u8) -> Result<(), String> {
    let (files, picture) = {
        let state = window.state::<AppState>();
        let files = paths.iter().map(|p| state.check(p)).collect::<Result<Vec<PathBuf>, _>>()?;
        let icon = state.check(&icon)?;
        let picture = tauri::async_runtime::spawn_blocking(move || {
            let img = sift_core::preview::pixels(&icon, DRAG_ICON_EDGE).ok()?;
            let img = sift_core::edit::shrink(sift_core::edit::turn(img, turn), DRAG_ICON_EDGE);
            sift_core::preview::encode_jpeg(&img, 85).ok()
        })
        .await
        .ok()
        .flatten();
        (files, picture)
    };
    if files.is_empty() {
        return Ok(());
    }
    // A plain grey square when the photo gives no picture.
    let picture = picture.unwrap_or_else(|| {
        sift_core::preview::encode_jpeg(&image::RgbImage::from_pixel(64, 64, image::Rgb([128, 128, 128])), 85).unwrap_or_default()
    });
    let (tx, rx) = std::sync::mpsc::channel();
    let target = window.clone();
    window
        .run_on_main_thread(move || {
            let started = drag::start_drag(&target, drag::DragItem::Files(files), drag::Image::Raw(picture), |_, _| {}, drag::Options::default());
            let _ = tx.send(started.map_err(|e| e.to_string()));
        })
        .map_err(|e| e.to_string())?;
    tauri::async_runtime::spawn_blocking(move || rx.recv().map_err(|e| e.to_string())?).await.map_err(|e| e.to_string())?
}

/// Elsewhere files cannot be dragged out; the window does not start a drag.
#[cfg(not(any(target_os = "macos", windows)))]
#[tauri::command]
pub async fn drag_out() -> Result<(), String> {
    Err("dragging files out is not available here".into())
}
