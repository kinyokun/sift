//! In-app updates, for official desktop builds only: those carry Nikon's
//! renderer, which a build from source must not acquire by updating itself.
//!
//! A release is described by `latest.json` among its GitHub release assets.
//! GitHub is slow or unreachable from mainland China, so the manifest and the
//! package are also fetched through public accelerators. The package is
//! checked against the signing key and against the version it was signed for,
//! so the worst an accelerator can do is fail.

use serde::Serialize;
use tauri::AppHandle;

/// Whether this build updates itself: only when built with `SIFT_OFFICIAL` set,
/// which the release pipeline does when it has the signing key (see `build.yml`).
pub const ENABLED: bool = cfg!(desktop) && option_env!("SIFT_OFFICIAL").is_some();

const UNAVAILABLE: &str = "this build does not update itself";

/// A newer release, as shown to the user.
#[derive(Clone, Serialize)]
pub struct Offer {
    version: String,
    current: String,
    /// That version's section of CHANGELOG.md.
    notes: String,
}

/// Looks for a newer release; `None` when this is the newest.
#[tauri::command]
pub async fn update_check(app: AppHandle) -> Result<Option<Offer>, String> {
    #[cfg(desktop)]
    if ENABLED {
        return desktop::check(&app).await;
    }
    let _ = app;
    Err(UNAVAILABLE.into())
}

/// Downloads the release found by `update_check`, installs it and restarts the
/// app, reporting `update-progress` events on the way. Only returns on failure.
#[tauri::command]
pub async fn update_install(app: AppHandle) -> Result<(), String> {
    #[cfg(desktop)]
    if ENABLED {
        return desktop::install(&app).await;
    }
    let _ = app;
    Err(UNAVAILABLE.into())
}

/// Stops a download in progress; `update_install` then fails with "cancelled".
#[tauri::command]
pub fn update_cancel(app: AppHandle) {
    #[cfg(desktop)]
    if ENABLED {
        desktop::cancel(&app);
    }
    let _ = app;
}

#[cfg(desktop)]
pub use desktop::attach;
#[cfg(all(desktop, debug_assertions))]
pub use desktop::unattended;

#[cfg(desktop)]
mod desktop {
    use super::{Offer, ENABLED};
    use semver::Version;
    use serde::Serialize;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant};
    use tauri::{AppHandle, Emitter, Manager};
    use tauri_plugin_updater::{Update, UpdaterExt};

    const MANIFEST: &str = "https://github.com/kinyokun/sift/releases/latest/download/latest.json";
    /// Accelerators for GitHub: each is put in front of a full GitHub address.
    /// They are asked alongside GitHub itself and preferred for the download.
    const MIRRORS: [&str; 2] = ["https://ghfast.top/", "https://gh-proxy.com/"];

    const CHECK_TIMEOUT: Duration = Duration::from_secs(10);
    /// How long the other sources get once one has answered: GitHub to correct
    /// a mirror still serving the manifest of the release before, a mirror to
    /// offer itself for the download. Waiting out a dead source every time
    /// would make "check now" take the full timeout.
    const GRACE: Duration = Duration::from_secs(3);
    /// A download that receives nothing for this long is given up on.
    const STALL: Duration = Duration::from_secs(20);
    const TICK: Duration = Duration::from_millis(250);
    const REPORT_EVERY: Duration = Duration::from_millis(100);
    const PROGRESS: &str = "update-progress";
    const CANCELLED: &str = "cancelled";

    #[derive(Clone, Serialize)]
    struct Progress {
        downloaded: u64,
        total: Option<u64>,
        /// The package has arrived and is being put in place.
        installing: bool,
    }

    #[derive(Default)]
    pub struct Pending {
        found: Mutex<Option<Found>>,
        downloading: AtomicBool,
        cancelled: AtomicBool,
    }

    #[derive(Clone)]
    struct Found {
        /// As announced, with the package's address on GitHub.
        update: Update,
        /// Prefixes to download through, the most promising first; empty is GitHub itself.
        routes: Vec<String>,
    }

    fn err(e: impl std::fmt::Display) -> String {
        e.to_string()
    }

    pub fn attach(builder: tauri::Builder<tauri::Wry>) -> tauri::Builder<tauri::Wry> {
        if !ENABLED {
            return builder;
        }
        builder.plugin(tauri_plugin_updater::Builder::new().build()).manage(Pending::default())
    }

    /// The manifest's address and the ways to reach it: the mirrors, then GitHub itself.
    fn sources() -> (String, Vec<String>) {
        let mut manifest = MANIFEST.to_string();
        let mut routes: Vec<String> = MIRRORS.iter().map(|m| m.to_string()).collect();
        // Debug builds can be pointed at a test server.
        if cfg!(debug_assertions) {
            if let Ok(url) = std::env::var("SIFT_UPDATE_MANIFEST") {
                manifest = url;
            }
            if let Ok(list) = std::env::var("SIFT_UPDATE_MIRRORS") {
                routes = list.split(',').filter(|m| !m.is_empty()).map(str::to_string).collect();
            }
        }
        routes.push(String::new());
        (manifest, routes)
    }

    pub async fn check(app: &AppHandle) -> Result<Option<Offer>, String> {
        let (manifest, routes) = sources();
        let mut asked = tokio::task::JoinSet::new();
        for (i, prefix) in routes.iter().enumerate() {
            let endpoint = format!("{prefix}{manifest}").parse().map_err(err)?;
            let updater = app.updater_builder().endpoints(vec![endpoint]).map_err(err)?.timeout(CHECK_TIMEOUT).build().map_err(err)?;
            asked.spawn(async move { (i, updater.check().await) });
        }
        // What each source said (`None`: nothing newer), in the order the answers arrived.
        let mut answers: Vec<(usize, Option<Update>)> = Vec::new();
        let mut failure = None;
        let mut deadline = None;
        loop {
            let next = match deadline {
                Some(at) => tokio::time::timeout_at(at, asked.join_next()).await.ok().flatten(),
                None => asked.join_next().await,
            };
            let (i, answer) = match next {
                None => break,
                Some(Err(_)) => continue,
                Some(Ok(done)) => done,
            };
            match answer {
                Ok(update) => {
                    answers.push((i, update));
                    deadline.get_or_insert_with(|| tokio::time::Instant::now() + GRACE);
                    // GitHub saying there is nothing newer ends it, and so does a
                    // mirror that has the release GitHub announced.
                    let github = answers.iter().find(|(i, _)| routes[*i].is_empty());
                    let has = |version: &str| answers.iter().any(|(i, u)| !routes[*i].is_empty() && u.as_ref().is_some_and(|u| u.version == version));
                    if github.is_some_and(|(_, said)| said.as_ref().is_none_or(|u| has(&u.version))) {
                        break;
                    }
                }
                Err(e) => failure = Some(err(e)),
            }
        }
        drop(asked);
        if answers.is_empty() {
            return Err(failure.unwrap_or_else(|| "no update source answered".into()));
        }

        // GitHub's own answer settles it; without one, the newest release a mirror knows of.
        let version = |u: &Update| Version::parse(&u.version).ok();
        let newest = match answers.iter().find(|(i, _)| routes[*i].is_empty()) {
            Some((_, update)) => update.as_ref(),
            None => answers.iter().filter_map(|(_, u)| u.as_ref()).reduce(|best, u| if version(u) > version(best) { u } else { best }),
        };
        let pending = app.state::<Pending>();
        let Some(newest) = newest else {
            *pending.found.lock().unwrap() = None;
            return Ok(None);
        };
        // The package comes through the mirrors that announced this release,
        // quickest first, then from GitHub if it did. The sources that did not
        // may only lag behind on the manifest, so they are tried after those,
        // again mirrors first.
        let announced: Vec<usize> = answers.iter().filter(|(_, u)| u.as_ref().is_some_and(|u| u.version == newest.version)).map(|(i, _)| *i).collect();
        let mut order: Vec<usize> = (0..routes.len()).collect();
        order.sort_by_key(|i| (!announced.contains(i), routes[*i].is_empty(), announced.iter().position(|a| a == i)));
        let offer = Offer { version: newest.version.clone(), current: newest.current_version.clone(), notes: newest.body.clone().unwrap_or_default() };
        *pending.found.lock().unwrap() = Some(Found { update: newest.clone(), routes: order.into_iter().map(|i| routes[i].clone()).collect() });
        Ok(Some(offer))
    }

    pub async fn install(app: &AppHandle) -> Result<(), String> {
        let pending = app.state::<Pending>();
        let found = pending.found.lock().unwrap().clone().ok_or("no update has been found")?;
        if pending.downloading.swap(true, Ordering::SeqCst) {
            return Err("the update is already being downloaded".into());
        }
        pending.cancelled.store(false, Ordering::SeqCst);
        let package = download(app, &found, &pending.cancelled).await;
        pending.downloading.store(false, Ordering::SeqCst);
        let package = package?;
        let size = package.len() as u64;
        let _ = app.emit(PROGRESS, Progress { downloaded: size, total: Some(size), installing: true });
        // On macOS the installer may ask for an administrator password through
        // the main thread and waits for it, so it must not run there.
        let update = found.update;
        tauri::async_runtime::spawn_blocking(move || update.install(package)).await.map_err(err)?.map_err(err)?;
        // Windows never gets here: the installer was started and the app has exited.
        app.restart()
    }

    pub fn cancel(app: &AppHandle) {
        app.state::<Pending>().cancelled.store(true, Ordering::SeqCst);
    }

    /// Fetches the package through each route in turn until one delivers bytes
    /// that pass the signature check.
    async fn download(app: &AppHandle, found: &Found, cancelled: &AtomicBool) -> Result<Vec<u8>, String> {
        let mut failure = String::new();
        for prefix in &found.routes {
            let mut update = found.update.clone();
            update.download_url = format!("{prefix}{}", found.update.download_url).parse().map_err(err)?;
            let alive = Arc::new(Mutex::new(Instant::now()));
            let (beat, window) = (alive.clone(), app.clone());
            let (mut downloaded, mut told) = (0u64, None::<Instant>);
            let job = update.download(
                move |chunk, total| {
                    downloaded += chunk as u64;
                    let now = Instant::now();
                    *beat.lock().unwrap() = now;
                    if told.is_none_or(|at| now.duration_since(at) >= REPORT_EVERY) || Some(downloaded) == total {
                        told = Some(now);
                        let _ = window.emit(PROGRESS, Progress { downloaded, total, installing: false });
                    }
                },
                || {},
            );
            let mut job = std::pin::pin!(job);
            let outcome = loop {
                match tokio::time::timeout(TICK, job.as_mut()).await {
                    Ok(done) => break done.map_err(err),
                    Err(_) if cancelled.load(Ordering::SeqCst) => return Err(CANCELLED.into()),
                    Err(_) if alive.lock().unwrap().elapsed() > STALL => break Err("the download stalled".into()),
                    Err(_) => {}
                }
            };
            match outcome {
                // Checking the signature of a large package takes a moment in
                // which nothing above looks at the flag.
                Ok(_) if cancelled.load(Ordering::SeqCst) => return Err(CANCELLED.into()),
                Ok(package) => return Ok(package),
                Err(e) => failure = e,
            }
        }
        Err(failure)
    }

    /// With `SIFT_UPDATE_AUTO` set, a debug build updates itself at launch
    /// without asking, so the whole path can be tested where no window can be clicked.
    #[cfg(debug_assertions)]
    pub fn unattended(app: &AppHandle) {
        if !ENABLED || std::env::var_os("SIFT_UPDATE_AUTO").is_none() {
            return;
        }
        let app = app.clone();
        tauri::async_runtime::spawn(async move {
            match check(&app).await {
                Ok(Some(offer)) => {
                    eprintln!("update: {} -> {}", offer.current, offer.version);
                    if let Err(e) = install(&app).await {
                        eprintln!("update: install failed: {e}");
                    }
                }
                Ok(None) => eprintln!("update: {} is the newest", app.package_info().version),
                Err(e) => eprintln!("update: check failed: {e}"),
            }
        });
    }
}
