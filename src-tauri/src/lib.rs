pub mod briefs;
pub mod commands;
pub mod handoff;
pub mod menu;
pub mod photos;
pub mod protocol;
pub mod state;
pub mod transfer;
pub mod update;

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

/// Gives the keyboard back to the page whenever the window may have taken it.
/// Leaving full screen makes tao hand it to its own view, which wry has taken
/// out of the window, so no key reaches the page until a click (tao 0.37 still
/// does). Tao queues that on the main queue before it reports the resize; this
/// goes in the queue behind it, as run straight away it loses now and then.
#[cfg(target_os = "macos")]
fn keep_keys_on_webview(window: &tauri::WebviewWindow) {
    let webview: tauri::Webview = window.as_ref().clone();
    window.on_window_event(move |event| {
        if matches!(event, tauri::WindowEvent::Focused(true) | tauri::WindowEvent::Resized(_)) {
            let webview = webview.clone();
            dispatch2::DispatchQueue::main().exec_async(move || {
                let _ = webview.set_focus();
            });
        }
    });
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let builder = tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_fs::init())
        .plugin(tauri_plugin_opener::init());
    #[cfg(desktop)]
    let builder = update::attach(builder);
    builder
        .setup(|app| {
            let cache = app.path().app_cache_dir()?;
            let data = app.path().app_data_dir()?;
            std::fs::create_dir_all(&cache)?;
            std::fs::create_dir_all(&data)?;
            app.manage(AppState::new(cache, data));
            menu::restore(app.handle());
            #[cfg(target_os = "macos")]
            if let Some(window) = app.get_webview_window("main") {
                keep_keys_on_webview(&window);
            }
            #[cfg(desktop)]
            update::startup(app.handle());
            #[cfg(all(desktop, debug_assertions))]
            update::unattended(app.handle());
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
            commands::folder_changes,
            commands::read_meta,
            commands::read_briefs,
            commands::kept_briefs,
            commands::warm_thumbs,
            commands::group_shots,
            commands::live_photo_pace,
            commands::save_live_photo,
            commands::set_rating,
            commands::delete_photos,
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
            commands::preview_export,
            commands::set_watermark,
            commands::watermark,
            commands::read_svg,
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
            update::update_check,
            update::update_download,
            update::update_install,
            update::update_cancel,
            update::update_discard,
            transfer_start,
            transfer_stop,
            menu::set_menu_language,
            handoff::open_with,
            handoff::drag_out,
        ])
        .build(tauri::generate_context!())
        .expect("error while building Sift")
        .run(|app, event| {
            #[cfg(desktop)]
            if let tauri::RunEvent::Exit = event {
                update::on_exit(app);
            }
            let _ = (app, event);
        });
}
