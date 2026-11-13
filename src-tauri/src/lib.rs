pub mod commands;
pub mod photos;
pub mod protocol;
pub mod state;
pub mod transfer;

use state::AppState;
use std::path::PathBuf;
use tauri::{AppHandle, Emitter, Manager, State};

/// Starts sharing `files` (and an inbox inside `folder`) on the local network.
#[tauri::command]
async fn transfer_start(app: AppHandle, folder: String, files: Vec<String>) -> Result<transfer::Info, String> {
    let (root, list) = {
        let state = app.state::<AppState>();
        let root = state.check(&folder)?;
        (root, files.iter().map(|f| state.check(f)).collect::<Result<Vec<PathBuf>, _>>()?)
    };
    let cache_dir = app.state::<AppState>().cache_dir.clone();
    let notify = app.clone();
    let on_received = Box::new(move |path: String| {
        let _ = notify.emit("transfer-received", path);
    });
    let (handle, info) = transfer::start(cache_dir, on_received, root, list).await?;
    if let Some(old) = app.state::<AppState>().transfer.lock().unwrap().replace(handle) {
        old.stop();
    }
    Ok(info)
}

#[tauri::command]
fn transfer_stop(state: State<AppState>) {
    if let Some(handle) = state.transfer.lock().unwrap().take() {
        handle.stop();
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_fs::init())
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            let cache = app.path().app_cache_dir()?;
            let data = app.path().app_data_dir()?;
            std::fs::create_dir_all(&cache)?;
            std::fs::create_dir_all(&data)?;
            app.manage(AppState::new(cache, data));
            Ok(())
        })
        .register_asynchronous_uri_scheme_protocol("sift", |ctx, request, responder| {
            let app = ctx.app_handle().clone();
            tauri::async_runtime::spawn(async move {
                let slots = app.state::<AppState>().image_slots.clone();
                let _permit = slots.acquire_owned().await;
                let uri = request.uri().clone();
                let worker = app.clone();
                let response = tauri::async_runtime::spawn_blocking(move || protocol::respond(&worker.state::<AppState>(), &uri)).await;
                if let Ok(response) = response {
                    responder.respond(response);
                }
            });
        })
        .invoke_handler(tauri::generate_handler![
            commands::scan_folder,
            commands::read_meta,
            commands::read_briefs,
            commands::group_shots,
            commands::live_photo_pace,
            commands::save_live_photo,
            commands::set_rating,
            commands::locate_eyes,
            commands::develop_raw,
            commands::cancel_task,
            commands::export_files,
            commands::export_names,
            commands::list_looks,
            commands::save_camera_look,
            commands::import_looks,
            commands::remove_look,
            commands::histogram,
            commands::export_framed,
            commands::preview_framed,
            commands::find_raws,
            commands::load_session,
            commands::save_session,
            commands::last_session,
            commands::clear_last_session,
            commands::app_info,
            commands::clear_cache,
            commands::list_albums,
            commands::import_files,
            commands::save_to_photos,
            transfer_start,
            transfer_stop,
        ])
        .run(tauri::generate_context!())
        .expect("error while running Sift");
}
