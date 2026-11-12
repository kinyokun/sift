//! Development bridge: serves the app's commands and images over HTTP so the
//! web UI can run in an ordinary browser (see `src/dev/mock.ts`). It binds to
//! loopback only and is never part of a release build.
//!
//! cargo run -p sift --example bridge

use axum::body::Bytes;
use axum::extract::{Path as UrlPath, State};
use axum::http::{header, HeaderValue, Method, Request, StatusCode, Uri};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::Router;
use serde_json::{json, Value};
use sift_lib::commands;
use sift_lib::protocol;
use sift_lib::state::AppState;
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::sync::Arc;

type Ctx = State<Arc<AppState>>;

fn arg<T: serde::de::DeserializeOwned>(args: &Value, key: &str) -> Result<T, String> {
    serde_json::from_value(args.get(key).cloned().unwrap_or(Value::Null)).map_err(|e| format!("{key}: {e}"))
}

fn run(state: &AppState, cmd: &str, args: &Value) -> Result<Response, String> {
    let ok = |v: Value| Ok(([(header::CONTENT_TYPE, "application/json")], v.to_string()).into_response());
    let json = |v: &dyn erased::Json| ok(v.to_value());
    match cmd {
        "app_info" => ok(json!({
            "platform": std::env::consts::OS, "mobile": false, "version": env!("CARGO_PKG_VERSION"),
            "library": null, "cache_bytes": sift_lib::state::dir_size(&state.cache_dir),
            "nikon_supported": sift_core::nikon::SUPPORTED, "nikon_sdk": state.nikon().is_some(),
        })),
        "scan_folder" => {
            let dir = PathBuf::from(arg::<String>(args, "path")?);
            if !dir.is_dir() {
                return Err(format!("not a folder: {}", dir.display()));
            }
            state.allow(&dir);
            json(&commands::scan(&dir, arg(args, "pair")?, arg(args, "preferRaw")?)?)
        }
        "read_meta" => json(&sift_core::meta::read(&state.check(&arg::<String>(args, "path")?)?).map_err(|e| e.to_string())?),
        "read_briefs" => json(&commands::briefs_of(state, &arg::<Vec<String>>(args, "paths")?)),
        "set_rating" => json(&commands::rate(state, &arg::<Vec<String>>(args, "paths")?, arg(args, "rating")?)?),
        "locate_eyes" => json(&commands::eyes_of(state, arg(args, "path")?, &arg::<Vec<sift_core::af::AfBox>>(args, "af")?)?),
        "load_session" => ok(commands::session_of(state, &arg::<String>(args, "folder")?)),
        "save_session" => {
            commands::store_session(state, &arg::<String>(args, "folder")?, &arg(args, "session")?)?;
            ok(Value::Null)
        }
        "last_session" => json(&commands::last_folder(state)),
        "clear_last_session" => {
            commands::forget_last(state);
            ok(Value::Null)
        }
        "preview_framed" => {
            let jpeg = commands::framed_preview(state, &arg::<String>(args, "path")?, &arg(args, "options")?)?;
            Ok(([(header::CONTENT_TYPE, "image/jpeg")], jpeg).into_response())
        }
        "export_files" => {
            let plan = arg(args, "plan")?;
            json(&commands::export_shots(state, &arg::<Vec<Vec<String>>>(args, "shots")?, &arg::<String>(args, "dest")?, &plan, |_| {})?)
        }
        "list_looks" => json(&commands::looks_of(state)),
        "save_camera_look" => json(&commands::keep_camera_look(state, &arg::<String>(args, "path")?)?),
        "histogram" => json(&commands::histogram_of(
            state,
            &arg::<String>(args, "path")?,
            arg::<Option<String>>(args, "look")?.as_deref(),
            arg(args, "edge")?,
            arg(args, "crop")?,
        )?),
        "import_looks" => {
            for src in arg::<Vec<String>>(args, "sources")? {
                let data = std::fs::read(&src).map_err(|e| e.to_string())?;
                commands::add_look(state, &src, &data)?;
            }
            json(&commands::looks_of(state))
        }
        "export_names" => json(&commands::names_preview(state, &arg::<Vec<Vec<String>>>(args, "shots")?, &arg(args, "rename")?)?),
        "find_raws" => {
            state.cancel.store(false, Ordering::Relaxed);
            let (picked, raws, dest) = (arg::<String>(args, "picked")?, arg::<String>(args, "raws")?, arg::<String>(args, "dest")?);
            let report = sift_core::export::find_raws(Path::new(&picked), Path::new(&raws), Path::new(&dest), arg(args, "loose")?, true, &state.cancel, |_| {});
            json(&report.map_err(|e| e.to_string())?)
        }
        // Forget all saved progress, so scripted runs start from a clean slate.
        "dev_reset" => {
            for entry in std::fs::read_dir(&state.data_dir).into_iter().flatten().flatten() {
                let _ = std::fs::remove_dir_all(entry.path()).or_else(|_| std::fs::remove_file(entry.path()));
            }
            state.looks.lock().unwrap().clear();
            ok(Value::Null)
        }
        "clear_cache" | "cancel_task" | "transfer_stop" | "develop_raw" => ok(Value::Null),
        "list_albums" => ok(json!([])),
        "export_framed" => json(&commands::frame_files(state, &arg(args, "request")?, |_| {})?),
        "transfer_start" => {
            let folder = state.check(&arg::<String>(args, "folder")?)?;
            let files = arg::<Vec<String>>(args, "files")?.iter().map(|f| state.check(f)).collect::<Result<Vec<_>, _>>()?;
            let on_received = Box::new(|path: String| println!("received {path}"));
            let started = tokio::runtime::Handle::current().block_on(sift_lib::transfer::start(state.cache_dir.clone(), on_received, folder, files))?;
            // The bridge keeps sharing until it exits.
            std::mem::forget(started.0);
            json(&started.1)
        }
        other => Err(format!("command not available in the bridge: {other}")),
    }
}

mod erased {
    pub trait Json {
        fn to_value(&self) -> serde_json::Value;
    }
    impl<T: serde::Serialize> Json for T {
        fn to_value(&self) -> serde_json::Value {
            serde_json::to_value(self).unwrap_or(serde_json::Value::Null)
        }
    }
}

async fn invoke(State(state): Ctx, UrlPath(cmd): UrlPath<String>, body: Bytes) -> Response {
    let args: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
    let done = tokio::task::spawn_blocking(move || run(&state, &cmd, &args)).await;
    match done {
        Ok(Ok(response)) => response,
        Ok(Err(message)) => (StatusCode::BAD_REQUEST, message).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

async fn image(State(state): Ctx, uri: Uri) -> Response {
    let made = tokio::task::spawn_blocking(move || protocol::respond(&state, &uri)).await;
    match made {
        Ok(response) => response.map(axum::body::Body::from),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

async fn cors(request: Request<axum::body::Body>, next: Next) -> Response {
    let mut response = if request.method() == Method::OPTIONS { StatusCode::NO_CONTENT.into_response() } else { next.run(request).await };
    let headers = response.headers_mut();
    headers.insert(header::ACCESS_CONTROL_ALLOW_ORIGIN, HeaderValue::from_static("*"));
    headers.insert(header::ACCESS_CONTROL_ALLOW_HEADERS, HeaderValue::from_static("*"));
    response
}

fn main() {
    let port: u16 = std::env::args().nth(1).and_then(|p| p.parse().ok()).unwrap_or(4777);
    let work = std::env::temp_dir().join("sift-bridge");
    let state = Arc::new(AppState::new(work.join("cache"), work.join("data")));
    let router = Router::new()
        .route("/invoke/{cmd}", post(invoke))
        .nest("/img", Router::new().route("/{kind}", get(image)))
        .layer(middleware::from_fn(cors))
        .with_state(state);
    let runtime = tokio::runtime::Builder::new_multi_thread().enable_all().build().expect("runtime");
    runtime.block_on(async {
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", port)).await.expect("bind");
        println!("bridge on http://127.0.0.1:{port}");
        axum::serve(listener, router).await.expect("serve");
    });
}
