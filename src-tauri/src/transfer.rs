//! Local-network transfer: a small HTTP server another device opens in its
//! browser to download the shared photos or send files back.
//!
//! Access needs the random token from the QR code or the 6-digit PIN. Files
//! are addressed by index into the shared list, never by path, and uploaded
//! names are reduced to a bare file name inside the inbox folder.

use crate::state::{cache_file, cached};
use axum::body::Body;
use axum::extract::{Path as UrlPath, Query, Request, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{Html, IntoResponse, Redirect, Response};
use axum::routing::{get, post};
use axum::Router;
use rand::RngExt;
use serde::Serialize;
use sift_core::{formats, preview};
use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, SocketAddr, UdpSocket};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use tokio::io::AsyncWriteExt;

const MAX_PIN_FAILURES: u32 = 8;
const MAX_UPLOAD_BYTES: u64 = 8 << 30;
const INBOX: &str = "Sift Inbox";
const PAGE: &str = include_str!("transfer.html");

pub struct Handle {
    stop: tokio::sync::oneshot::Sender<()>,
}

impl Handle {
    pub fn stop(self) {
        let _ = self.stop.send(());
    }
}

#[derive(Serialize, Clone)]
pub struct Info {
    pub url: String,
    pub pin: String,
    /// QR code for `url` with the access token, as an SVG document.
    pub qr: String,
    pub count: usize,
    pub inbox: String,
}

struct Shared {
    cache_dir: PathBuf,
    /// Called with the path of every file a device sends back.
    on_received: Box<dyn Fn(String) + Send + Sync>,
    token: String,
    pin: String,
    title: String,
    files: Vec<PathBuf>,
    inbox: PathBuf,
    pin_failures: AtomicU32,
}

type Ctx = State<Arc<Shared>>;

fn lan_ip() -> IpAddr {
    // Connecting a UDP socket sends nothing; it only selects the outbound interface.
    UdpSocket::bind("0.0.0.0:0")
        .and_then(|s| s.connect("192.0.2.1:9").and_then(|_| s.local_addr()))
        .map_or(IpAddr::V4(Ipv4Addr::LOCALHOST), |a| a.ip())
}

fn authed(ctx: &Shared, headers: &HeaderMap) -> bool {
    let want = format!("sift={}", ctx.token);
    headers
        .get(header::COOKIE)
        .and_then(|c| c.to_str().ok())
        .is_some_and(|c| c.split(';').any(|kv| kv.trim() == want))
}

fn grant(ctx: &Shared) -> Response {
    let cookie = format!("sift={}; Path=/; HttpOnly; SameSite=Strict", ctx.token);
    ([(header::SET_COOKIE, cookie)], Redirect::to("/")).into_response()
}

fn deny() -> Response {
    StatusCode::UNAUTHORIZED.into_response()
}

fn escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

async fn index(State(ctx): Ctx, headers: HeaderMap, Query(q): Query<HashMap<String, String>>) -> Response {
    if q.get("t").is_some_and(|t| *t == ctx.token) {
        return grant(&ctx);
    }
    if !authed(&ctx, &headers) {
        let locked = ctx.pin_failures.load(Ordering::Relaxed) >= MAX_PIN_FAILURES;
        let page = PAGE
            .replace("__MODE__", if locked { "locked" } else { "pin" })
            .replace("__TITLE__", &escape(&ctx.title))
            .replace("__FILES__", "[]");
        return Html(page).into_response();
    }
    let movies = live_movies(&ctx.files);
    let list: Vec<serde_json::Value> = ctx
        .files
        .iter()
        .enumerate()
        .filter(|(i, _)| !movies.values().any(|m| m == i))
        .map(|(i, f)| {
            let size = std::fs::metadata(f).map_or(0, |m| m.len());
            serde_json::json!({ "i": i, "name": f.file_name().unwrap_or_default().to_string_lossy(), "size": size, "live": movies.contains_key(&i) })
        })
        .collect();
    let files = serde_json::to_string(&list).unwrap_or_else(|_| "[]".into()).replace("</", "<\\/");
    Html(PAGE.replace("__MODE__", "files").replace("__TITLE__", &escape(&ctx.title)).replace("__FILES__", &files)).into_response()
}

async fn enter_pin(State(ctx): Ctx, body: String) -> Response {
    if ctx.pin_failures.load(Ordering::Relaxed) >= MAX_PIN_FAILURES {
        return Redirect::to("/").into_response();
    }
    let entered: String = body.split('&').find_map(|kv| kv.strip_prefix("pin=")).unwrap_or("").chars().filter(char::is_ascii_digit).collect();
    if entered == ctx.pin {
        return grant(&ctx);
    }
    ctx.pin_failures.fetch_add(1, Ordering::Relaxed);
    Redirect::to("/?wrong=1").into_response()
}

fn file_at(ctx: &Shared, index: usize) -> Option<&PathBuf> {
    ctx.files.get(index)
}

fn is_movie(path: &Path) -> bool {
    path.extension().is_some_and(|x| x.eq_ignore_ascii_case("mov"))
}

/// The Live Photos among the shared files: the index of each still that has
/// a movie of the same name next to it, and the index of that movie.
fn live_movies(files: &[PathBuf]) -> HashMap<usize, usize> {
    let key = |p: &Path| p.with_extension("").to_string_lossy().to_ascii_lowercase();
    let movies: HashMap<String, usize> = files.iter().enumerate().filter(|(_, f)| is_movie(f)).map(|(i, f)| (key(f), i)).collect();
    files.iter().enumerate().filter(|(_, f)| !is_movie(f)).filter_map(|(i, f)| Some((i, *movies.get(&key(f))?))).collect()
}

async fn thumb(State(ctx): Ctx, headers: HeaderMap, UrlPath(index): UrlPath<usize>) -> Response {
    if !authed(&ctx, &headers) {
        return deny();
    }
    let Some(path) = file_at(&ctx, index).cloned() else { return StatusCode::NOT_FOUND.into_response() };
    let cache_dir = ctx.cache_dir.clone();
    let made = tokio::task::spawn_blocking(move || cached(&cache_file(&cache_dir, "thumb", &path, 320), || preview::scaled(&path, 320, 80))).await;
    match made {
        Ok(Ok(data)) => ([(header::CONTENT_TYPE, "image/jpeg"), (header::CACHE_CONTROL, "private, max-age=3600")], data).into_response(),
        _ => StatusCode::NOT_FOUND.into_response(),
    }
}

fn attachment(name: &str) -> String {
    let encoded = percent_encoding::utf8_percent_encode(name, percent_encoding::NON_ALPHANUMERIC);
    format!("attachment; filename*=UTF-8''{encoded}")
}

async fn stream_file(path: &Path, name: &str, mime: &str) -> Response {
    let Ok(file) = tokio::fs::File::open(path).await else { return StatusCode::NOT_FOUND.into_response() };
    let len = file.metadata().await.map_or(0, |m| m.len());
    Response::builder()
        .header(header::CONTENT_TYPE, mime)
        .header(header::CONTENT_LENGTH, len)
        .header(header::CONTENT_DISPOSITION, attachment(name))
        .body(Body::from_stream(tokio_util::io::ReaderStream::with_capacity(file, 256 * 1024)))
        .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
}

async fn download(State(ctx): Ctx, headers: HeaderMap, UrlPath(index): UrlPath<usize>) -> Response {
    if !authed(&ctx, &headers) {
        return deny();
    }
    let Some(path) = file_at(&ctx, index) else { return StatusCode::NOT_FOUND.into_response() };
    let name = path.file_name().unwrap_or_default().to_string_lossy().into_owned();
    stream_file(path, &name, "application/octet-stream").await
}

/// Everything as one uncompressed ZIP (photos do not compress, and stored
/// entries unpack instantly on phones).
async fn bundle(State(ctx): Ctx, headers: HeaderMap) -> Response {
    if !authed(&ctx, &headers) {
        return deny();
    }
    zip_of(&ctx, ctx.files.clone(), Some(ctx.title.clone()), &ctx.title).await
}

/// A Live Photo's still and movie together, as one ZIP named after it: a
/// browser cannot hand the pair to the photo library, but keeps them together.
async fn live_pair(State(ctx): Ctx, headers: HeaderMap, UrlPath(index): UrlPath<usize>) -> Response {
    if !authed(&ctx, &headers) {
        return deny();
    }
    let Some(movie) = live_movies(&ctx.files).get(&index).map(|&m| ctx.files[m].clone()) else { return StatusCode::NOT_FOUND.into_response() };
    let still = ctx.files[index].clone();
    let name = still.file_stem().unwrap_or_default().to_string_lossy().into_owned();
    zip_of(&ctx, vec![still, movie], None, &name).await
}

/// Streams `files` as `<name>.zip`, inside a folder of that name when given.
async fn zip_of(ctx: &Shared, files: Vec<PathBuf>, folder: Option<String>, name: &str) -> Response {
    let tmp = ctx.cache_dir.join(format!("transfer-{}.zip", rand::rng().random::<u64>()));
    let out = tmp.clone();
    let built = tokio::task::spawn_blocking(move || -> std::io::Result<()> {
        if let Some(dir) = out.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let mut zip = zip::ZipWriter::new(std::fs::File::create(&out)?);
        let options = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored).large_file(true);
        for f in &files {
            let name = f.file_name().unwrap_or_default().to_string_lossy();
            let entry = folder.as_ref().map_or_else(|| name.to_string(), |dir| format!("{dir}/{name}"));
            zip.start_file(entry, options).map_err(std::io::Error::other)?;
            std::io::copy(&mut std::fs::File::open(f)?, &mut zip)?;
        }
        zip.finish().map_err(std::io::Error::other)?;
        Ok(())
    })
    .await;
    if !matches!(built, Ok(Ok(()))) {
        let _ = tokio::fs::remove_file(&tmp).await;
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    }
    let response = stream_file(&tmp, &format!("{name}.zip"), "application/zip").await;
    // The open handle keeps the data readable; the name can go right away.
    let _ = tokio::fs::remove_file(&tmp).await;
    response
}

/// Reduces a client-supplied name to a safe file name inside the inbox.
fn safe_name(raw: &str) -> Option<String> {
    let name = raw.rsplit(['/', '\\']).next()?.trim();
    let name: String = name.chars().filter(|c| !c.is_control() && !matches!(c, ':' | '*' | '?' | '"' | '<' | '>' | '|')).collect();
    // A movie may be the other half of a Live Photo sent along with its still.
    let ok = !name.is_empty() && !name.starts_with('.') && (formats::is_supported(Path::new(&name)) || is_movie(Path::new(&name)));
    ok.then_some(name)
}

fn free_path(dir: &Path, name: &str) -> PathBuf {
    let candidate = dir.join(name);
    if !candidate.exists() {
        return candidate;
    }
    let p = Path::new(name);
    let (stem, ext) = (p.file_stem().unwrap_or_default().to_string_lossy(), p.extension().unwrap_or_default().to_string_lossy());
    (2..).map(|n| dir.join(format!("{stem} {n}.{ext}"))).find(|c| !c.exists()).unwrap()
}

async fn upload(State(ctx): Ctx, headers: HeaderMap, Query(q): Query<HashMap<String, String>>, request: Request) -> Response {
    if !authed(&ctx, &headers) {
        return deny();
    }
    let Some(name) = q.get("name").and_then(|n| safe_name(n)) else { return StatusCode::BAD_REQUEST.into_response() };
    if tokio::fs::create_dir_all(&ctx.inbox).await.is_err() {
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    }
    let target = free_path(&ctx.inbox, &name);
    let partial = target.with_extension("part");
    let result: std::io::Result<()> = async {
        use futures_util::StreamExt;
        let mut file = tokio::fs::File::create(&partial).await?;
        let mut stream = request.into_body().into_data_stream();
        let mut written = 0u64;
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(std::io::Error::other)?;
            written += chunk.len() as u64;
            if written > MAX_UPLOAD_BYTES {
                return Err(std::io::Error::other("upload too large"));
            }
            file.write_all(&chunk).await?;
        }
        file.sync_all().await?;
        tokio::fs::rename(&partial, &target).await
    }
    .await;
    match result {
        Ok(()) => {
            (ctx.on_received)(target.to_string_lossy().into_owned());
            StatusCode::OK.into_response()
        }
        Err(_) => {
            let _ = tokio::fs::remove_file(&partial).await;
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

pub async fn start(
    cache_dir: PathBuf,
    on_received: Box<dyn Fn(String) + Send + Sync>,
    folder: PathBuf,
    files: Vec<PathBuf>,
) -> Result<(Handle, Info), String> {
    let listener = tokio::net::TcpListener::bind(SocketAddr::from(([0, 0, 0, 0], 0))).await.map_err(|e| e.to_string())?;
    let port = listener.local_addr().map_err(|e| e.to_string())?.port();
    let mut rng = rand::rng();
    let token: String = (0..32).map(|_| char::from_digit(rng.random_range(0..16), 16).unwrap()).collect();
    let pin = format!("{:06}", rng.random_range(0..1_000_000));
    let title = folder.file_name().map_or("Sift".to_string(), |n| n.to_string_lossy().into_owned());
    let inbox = folder.join(INBOX);
    let url = format!("http://{}:{port}", lan_ip());
    let qr = qrcode::QrCode::new(format!("{url}/?t={token}"))
        .map_err(|e| e.to_string())?
        .render::<qrcode::render::svg::Color>()
        .min_dimensions(240, 240)
        .quiet_zone(false)
        .build();
    let info = Info { url, pin: pin.clone(), qr, count: files.len(), inbox: inbox.to_string_lossy().into_owned() };

    let shared = Arc::new(Shared { cache_dir, on_received, token, pin, title, files, inbox, pin_failures: AtomicU32::new(0) });
    let router = Router::new()
        .route("/", get(index))
        .route("/pin", post(enter_pin))
        .route("/t/{index}", get(thumb))
        .route("/f/{index}", get(download))
        .route("/zip", get(bundle))
        .route("/l/{index}", get(live_pair))
        .route("/up", post(upload))
        .with_state(shared);
    let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
    tokio::spawn(async move {
        let _ = axum::serve(listener, router)
            .with_graceful_shutdown(async {
                let _ = stopped.await;
            })
            .await;
    });
    Ok((Handle { stop }, info))
}

#[cfg(test)]
mod tests {
    use super::{live_movies, safe_name};
    use std::collections::HashMap;
    use std::path::PathBuf;

    #[test]
    fn upload_names_cannot_leave_the_inbox() {
        assert_eq!(safe_name("../../etc/a.jpg").as_deref(), Some("a.jpg"));
        assert_eq!(safe_name("C:\\Users\\me\\b.NEF").as_deref(), Some("b.NEF"));
        assert_eq!(safe_name("/abs/c.cr3").as_deref(), Some("c.cr3"));
        assert_eq!(safe_name(".."), None);
        assert_eq!(safe_name(".hidden.jpg"), None);
        assert_eq!(safe_name("run.sh"), None);
        assert_eq!(safe_name(""), None);
        assert_eq!(safe_name("IMG_1.MOV").as_deref(), Some("IMG_1.MOV"));
    }

    #[test]
    fn a_still_with_a_movie_of_its_name_is_one_live_photo() {
        let files: Vec<PathBuf> = ["a/IMG_1.JPG", "a/IMG_2.JPG", "a/IMG_1.MOV", "a/IMG_3.HEIC", "a/clip.mov", "b/IMG_2.MOV"].iter().map(PathBuf::from).collect();
        assert_eq!(live_movies(&files), HashMap::from([(0, 2)]));
    }
}
