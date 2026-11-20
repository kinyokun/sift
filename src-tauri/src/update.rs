//! In-app updates, for official desktop builds only: those carry Nikon's
//! renderer, which a build from source must not acquire by updating itself.
//!
//! A release is described by `latest.json` among its GitHub release assets.
//! GitHub is slow or unreachable from mainland China, so the manifest and the
//! package are also fetched through public accelerators. What is downloaded is
//! checked against the signing key and against the version it was signed for,
//! so the worst an accelerator can do is fail.
//!
//! Besides the full package, releases carry deltas from the releases before
//! them (see `sift-delta`). `latest.json` points to the newest release's
//! `updates.json`, which lists the deltas there are and the notes of every
//! release; the deltas that lead from the installed version to the newest with
//! the least to download are applied one after another, and the full package
//! is taken when there are none, they are no smaller, or one fails. Either way
//! the update is prepared in the background (`update_download`): the new
//! release is laid out beside the installed one and every file checked, so
//! putting it in place is a quick swap, done when the app quits or at once
//! with a restart (`update_install`).
use serde::Serialize;
use tauri::AppHandle;

/// Whether this build updates itself: only when built with `SIFT_OFFICIAL` set,
/// which the release pipeline does when it has the signing key (see `build.yml`).
pub const ENABLED: bool = cfg!(desktop) && option_env!("SIFT_OFFICIAL").is_some();

const UNAVAILABLE: &str = "this build does not update itself";

/// One release's section of CHANGELOG.md.
#[derive(Clone, Serialize)]
pub struct Release {
    version: String,
    date: Option<String>,
    notes: String,
}

/// A newer release, as shown to the user.
#[derive(Clone, Serialize)]
pub struct Offer {
    version: String,
    current: String,
    /// What changed since the installed version, newest first.
    releases: Vec<Release>,
    /// What there is to download when a delta covers the installed version.
    delta_size: Option<u64>,
    /// Downloaded and waiting to be put in place.
    ready: bool,
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

/// Downloads and prepares the release found by `update_check`, reporting
/// `update-progress` events; it is put in place when the app quits.
#[tauri::command]
pub async fn update_download(app: AppHandle) -> Result<(), String> {
    #[cfg(desktop)]
    if ENABLED {
        return desktop::download(&app).await;
    }
    let _ = app;
    Err(UNAVAILABLE.into())
}

/// Puts the release found by `update_check` in place, downloading it first if
/// need be, and restarts the app. Only returns on failure.
#[tauri::command]
pub async fn update_install(app: AppHandle) -> Result<(), String> {
    #[cfg(desktop)]
    if ENABLED {
        return desktop::install(&app).await;
    }
    let _ = app;
    Err(UNAVAILABLE.into())
}

/// Stops a download in progress; it then fails with "cancelled".
#[tauri::command]
pub fn update_cancel(app: AppHandle) {
    #[cfg(desktop)]
    if ENABLED {
        desktop::cancel(&app);
    }
    let _ = app;
}

/// Throws away a downloaded release, so it is not put in place on quitting.
#[tauri::command]
pub fn update_discard(app: AppHandle) {
    #[cfg(desktop)]
    if ENABLED {
        desktop::discard(&app);
    }
    let _ = app;
}

#[cfg(desktop)]
pub use desktop::{attach, on_exit, startup};
#[cfg(all(desktop, debug_assertions))]
pub use desktop::unattended;

#[cfg(desktop)]
mod desktop {
    use super::{Offer, Release, ENABLED};
    use semver::Version;
    use serde::{Deserialize, Serialize};
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant};
    use tauri::{AppHandle, Emitter, Manager};
    use sift_delta::catalogue::{Catalogue, Hop};
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
    /// Where a release is prepared on Windows, inside the install folder.
    const STAGE: &str = ".sift-update";
    /// The name `updates.json` gives this platform.
    const OS: &str = std::env::consts::OS;

    #[derive(Clone, Serialize)]
    struct Progress {
        downloaded: u64,
        total: Option<u64>,
        /// "download", "prepare" (laying the release out) or "install".
        step: &'static str,
    }

    #[derive(Default)]
    pub struct Pending {
        found: Mutex<Option<Found>>,
        ready: Mutex<Option<Ready>>,
        busy: AtomicBool,
        cancelled: AtomicBool,
    }

    #[derive(Clone)]
    struct Found {
        /// As announced, with the package's address on GitHub.
        update: Update,
        /// Prefixes to download through, the most promising first; empty is GitHub itself.
        routes: Vec<String>,
        /// The deltas to take from the installed version; none for the full package.
        hops: Vec<Hop>,
        releases: Vec<Release>,
        /// Whether `hops` and `releases` come from `updates.json`.
        catalogued: bool,
    }

    /// Where `latest.json` says its `updates.json` is.
    #[derive(Deserialize)]
    struct CatalogueLink {
        url: String,
        /// Made for the version `latest.json` announces.
        signature: String,
    }
    /// A release downloaded and laid out, waiting to be put in place.
    enum Ready {
        /// macOS: the new app, to be swapped with `bundle`.
        Bundle { version: String, staged: PathBuf, bundle: PathBuf },
        /// Windows: the files that change, staged in the install folder.
        Files { version: String, staged: sift_delta::Staged },
        /// The verified full package, for the updater to install: where the
        /// app cannot be replaced by Sift itself (macOS asks for a password).
        Package { version: String, update: Box<Update>, path: PathBuf },
    }

    impl Ready {
        fn version(&self) -> &str {
            match self {
                Ready::Bundle { version, .. } | Ready::Files { version, .. } | Ready::Package { version, .. } => version,
            }
        }

        /// Deletes what was prepared; for a release no longer wanted.
        fn discard(self) {
            match self {
                Ready::Bundle { staged, .. } => {
                    let _ = std::fs::remove_dir_all(staged.parent().unwrap_or(&staged));
                }
                Ready::Files { staged, .. } => staged.discard(),
                Ready::Package { path, .. } => {
                    let _ = std::fs::remove_file(path);
                }
            }
        }
    }

    /// Where the installed release can be replaced by Sift itself.
    enum Target {
        /// A macOS app, swapped for one built in `stage`, a folder on its volume.
        #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
        Bundle { bundle: PathBuf, stage: PathBuf },
        /// The Windows install folder, whose files are replaced one by one.
        #[cfg_attr(not(windows), allow(dead_code))]
        Folder { root: PathBuf },
    }

    fn err(e: impl std::fmt::Display) -> String {
        e.to_string()
    }

    pub fn attach(builder: tauri::Builder<tauri::Wry>) -> tauri::Builder<tauri::Wry> {
        if !ENABLED {
            return builder;
        }
        let mut updater = tauri_plugin_updater::Builder::new();
        // Debug builds can be tested with packages signed by a key of one's own.
        if cfg!(debug_assertions) {
            if let Ok(key) = std::env::var("SIFT_UPDATE_PUBKEY") {
                updater = updater.pubkey(key);
            }
        }
        builder.plugin(updater.build()).manage(Pending::default())
    }
    /// Whether files can be made and removed in `dir`.
    fn writable(dir: &Path) -> bool {
        let probe = dir.join(format!(".sift-probe-{}", std::process::id()));
        std::fs::write(&probe, b"").and_then(|()| std::fs::remove_file(&probe)).is_ok()
    }

    fn cache(app: &AppHandle) -> Option<PathBuf> {
        app.path().app_cache_dir().ok().map(|dir| dir.join("update"))
    }

    #[cfg(target_os = "macos")]
    fn target(app: &AppHandle) -> Option<Target> {
        use std::os::unix::fs::MetadataExt;
        let exe = std::env::current_exe().ok()?;
        // Sift.app/Contents/MacOS/sift
        let bundle = exe.parent()?.parent()?.parent()?.to_path_buf();
        let parent = bundle.parent()?;
        if bundle.extension()? != "app" || !writable(parent) || !writable(&bundle) {
            return None;
        }
        // Swapping only works within one volume; the cache folder normally is on the app's.
        let volume = std::fs::metadata(parent).ok()?.dev();
        let cache = cache(app)?;
        std::fs::create_dir_all(&cache).ok()?;
        let stage = if std::fs::metadata(&cache).ok()?.dev() == volume { cache } else { parent.join(STAGE) };
        Some(Target::Bundle { bundle, stage })
    }

    #[cfg(windows)]
    fn target(_app: &AppHandle) -> Option<Target> {
        let root = std::env::current_exe().ok()?.parent()?.to_path_buf();
        writable(&root).then_some(Target::Folder { root })
    }

    #[cfg(not(any(target_os = "macos", windows)))]
    fn target(_app: &AppHandle) -> Option<Target> {
        None
    }

    /// Clears what an earlier run left: the app it swapped out, a release it
    /// prepared but never put in place, files it set aside on Windows. The
    /// folders are only moved aside here, at once, and deleted in the
    /// background, as an update may be prepared in them again right away.
    pub fn startup(app: &AppHandle) {
        if !ENABLED {
            return;
        }
        let mut folders = Vec::new();
        folders.extend(cache(app));
        let exe = std::env::current_exe().ok();
        #[cfg(target_os = "macos")]
        folders.extend(exe.as_ref().and_then(|exe| exe.ancestors().nth(4)).map(|parent| parent.join(STAGE)));
        #[cfg(windows)]
        folders.extend(exe.as_ref().and_then(|exe| exe.parent()).map(|root| root.join(STAGE)));
        let aside: Vec<(PathBuf, String)> = folders
            .iter()
            .filter_map(|folder| {
                let prefix = format!("{}.trash-", folder.file_name()?.to_string_lossy());
                let _ = std::fs::rename(folder, folder.with_file_name(format!("{prefix}{}", std::process::id())));
                Some((folder.parent()?.to_path_buf(), prefix))
            })
            .collect();
        std::thread::spawn(move || {
            for (parent, prefix) in aside {
                for entry in std::fs::read_dir(parent).into_iter().flatten().flatten() {
                    if entry.file_name().to_string_lossy().starts_with(&prefix) {
                        let _ = std::fs::remove_dir_all(entry.path());
                    }
                }
            }
            #[cfg(windows)]
            if let Some(root) = exe.as_ref().and_then(|exe| exe.parent()) {
                sift_delta::sweep(root);
            }
            let _ = exe;
        });
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

    /// The releases after the installed one up to `update`, newest first, from
    /// the history in `updates.json`; or the notes of `update` alone.
    fn releases(update: &Update, history: &[sift_delta::catalogue::Release]) -> Vec<Release> {
        let (current, newest) = (Version::parse(&update.current_version).ok(), Version::parse(&update.version).ok());
        let mut releases: Vec<(Version, Release)> = history
            .iter()
            .filter_map(|e| Some((Version::parse(&e.version).ok()?, e)))
            .filter(|(v, _)| current.as_ref().is_none_or(|c| v > c) && newest.as_ref().is_none_or(|n| v <= n))
            .map(|(v, e)| (v, Release { version: e.version.clone(), date: e.date.clone(), notes: e.notes.clone() }))
            .collect();
        if releases.is_empty() {
            return vec![Release { version: update.version.clone(), date: None, notes: update.body.clone().unwrap_or_default() }];
        }
        releases.sort_by(|a, b| b.0.cmp(&a.0));
        releases.into_iter().map(|(_, r)| r).collect()
    }

    /// The `updates.json` of the release `update` announces, if it has one.
    async fn catalogue(update: &Update, routes: &[String]) -> Option<Catalogue> {
        let link: CatalogueLink = serde_json::from_value(update.raw_json.get("catalogue")?.clone()).ok()?;
        let item = Item { url: link.url, signature: link.signature, version: update.version.clone() };
        match fetch(update, routes, &item, None, None).await {
            Ok(bytes) => serde_json::from_slice(&bytes).map_err(|e| log(&format!("updates.json unreadable: {e}"))).ok(),
            Err(e) => {
                log(&format!("updates.json not downloaded: {e}"));
                None
            }
        }
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
        let routes: Vec<String> = order.into_iter().map(|i| routes[i].clone()).collect();
        // What updates.json said is kept for as long as the same release is on offer.
        let known = pending.found.lock().unwrap().clone().filter(|f| f.catalogued && f.update.version == newest.version && f.update.current_version == newest.current_version);
        let (hops, releases, catalogued) = match known {
            Some(found) => (found.hops, found.releases, true),
            None => {
                let catalogue = catalogue(newest, &routes).await;
                let catalogued = catalogue.is_some();
                let catalogue = catalogue.unwrap_or_default();
                let hops = match target(app) {
                    Some(_) => sift_delta::catalogue::route(
                        catalogue.deltas.get(OS).map(Vec::as_slice).unwrap_or_default(),
                        &newest.current_version,
                        &newest.version,
                        catalogue.full.get(OS).copied(),
                    ),
                    None => Vec::new(),
                };
                (hops, releases(newest, &catalogue.history), catalogued)
            }
        };
        let ready = pending.ready.lock().unwrap().as_ref().is_some_and(|r| r.version() == newest.version);
        let offer = Offer {
            version: newest.version.clone(),
            current: newest.current_version.clone(),
            releases: releases.clone(),
            delta_size: (!hops.is_empty()).then(|| hops.iter().map(|h| h.size).sum()),
            ready,
        };
        *pending.found.lock().unwrap() = Some(Found { update: newest.clone(), routes, hops, releases, catalogued });
        Ok(Some(offer))
    }
    /// Makes sure the release last found is downloaded and laid out.
    pub async fn download(app: &AppHandle) -> Result<(), String> {
        let pending = app.state::<Pending>();
        let found = pending.found.lock().unwrap().clone().ok_or("no update has been found")?;
        if pending.ready.lock().unwrap().as_ref().is_some_and(|r| r.version() == found.update.version) {
            return Ok(());
        }
        if pending.busy.swap(true, Ordering::SeqCst) {
            return Err("the update is already being downloaded".into());
        }
        pending.cancelled.store(false, Ordering::SeqCst);
        let ready = prepare(app, &found, &pending.cancelled).await;
        pending.busy.store(false, Ordering::SeqCst);
        let ready = ready?;
        if let Some(old) = pending.ready.lock().unwrap().replace(ready) {
            std::thread::spawn(move || old.discard());
        }
        Ok(())
    }

    async fn prepare(app: &AppHandle, found: &Found, cancelled: &AtomicBool) -> Result<Ready, String> {
        if let (false, Some(target)) = (found.hops.is_empty(), target(app)) {
            match chain(app, found, target, cancelled).await {
                Ok(ready) => return Ok(ready),
                Err(e) if e == CANCELLED => return Err(e),
                Err(e) => log(&format!("deltas failed, downloading the full package: {e}")),
            }
        }
        let item = Item { url: found.update.download_url.to_string(), signature: found.update.signature.clone(), version: found.update.version.clone() };
        let package = fetch(&found.update, &found.routes, &item, Some(Report { app, before: 0, total: None }), Some(cancelled)).await?;
        let _ = app.emit(PROGRESS, Progress { downloaded: package.len() as u64, total: Some(package.len() as u64), step: "prepare" });
        let (update, target, cache) = (found.update.clone(), target(app), cache(app));
        tauri::async_runtime::spawn_blocking(move || from_package(package, update, target, cache)).await.map_err(err)?
    }

    fn log(message: &str) {
        eprintln!("update: {message}");
    }

    /// Where the release a delta makes is laid out: the folder to delete when
    /// it is no longer wanted, and the release in it.
    fn tree(target: &Target, version: &str) -> (PathBuf, PathBuf) {
        match target {
            Target::Bundle { bundle, stage } => {
                let folder = stage.join(version);
                let app = folder.join(bundle.file_name().unwrap_or("Sift.app".as_ref()));
                (folder, app)
            }
            Target::Folder { root } => {
                let folder = root.join(STAGE).join(version);
                (folder.clone(), folder)
            }
        }
    }

    /// Downloads the deltas `found` leads through and applies them one after
    /// another, each to the release the one before laid out.
    async fn chain(app: &AppHandle, found: &Found, target: Target, cancelled: &AtomicBool) -> Result<Ready, String> {
        let mut last = None;
        let made = steps(app, found, &target, cancelled, &mut last).await;
        let Some((folder, out)) = last else { return Err(made.err().unwrap_or_else(|| "no deltas".into())) };
        let (manifest, removed) = match made {
            Ok(made) => made,
            Err(e) => {
                std::thread::spawn(move || std::fs::remove_dir_all(folder));
                return Err(e);
            }
        };
        let version = manifest.version.clone();
        match target {
            Target::Bundle { bundle, .. } => Ok(Ready::Bundle { version, staged: out, bundle }),
            Target::Folder { root } => {
                let staged = tauri::async_runtime::spawn_blocking(move || {
                    let staged = sift_delta::Staged::prepare(&root, &out, &manifest, &removed);
                    if staged.is_err() {
                        let _ = std::fs::remove_dir_all(&folder);
                    }
                    staged
                });
                Ok(Ready::Files { version, staged: staged.await.map_err(err)?.map_err(err)? })
            }
        }
    }

    /// The work of [`chain`]; `last` is kept up to date with the release laid
    /// out last (its folder, and the release in it), for the caller to keep or
    /// delete. Gives the last delta's manifest and what all of them removed.
    async fn steps(
        app: &AppHandle,
        found: &Found,
        target: &Target,
        cancelled: &AtomicBool,
        last: &mut Option<(PathBuf, PathBuf)>,
    ) -> Result<(sift_delta::Manifest, Vec<String>), String> {
        let installed = match target {
            Target::Bundle { bundle, .. } => bundle.clone(),
            Target::Folder { root } => root.clone(),
        };
        let total = found.hops.iter().map(|h| h.size).sum();
        let (mut before, mut removed, mut manifest) = (0, Vec::new(), None);
        for hop in &found.hops {
            let item = Item { url: hop.url.clone(), signature: hop.signature.clone(), version: hop.to.clone() };
            let bytes = fetch(&found.update, &found.routes, &item, Some(Report { app, before, total: Some(total) }), Some(cancelled)).await?;
            before += hop.size;
            let _ = app.emit(PROGRESS, Progress { downloaded: before, total: Some(total), step: "prepare" });
            let base = last.as_ref().map_or_else(|| installed.clone(), |(_, out)| out.clone());
            let (folder, out) = tree(target, &hop.to);
            let previous = last.as_ref().map(|(folder, _)| folder.clone());
            let job = {
                let (folder, out, hop) = (folder.clone(), out.clone(), hop.clone());
                move || -> Result<sift_delta::Manifest, String> {
                    let delta = sift_delta::Delta::parse(&bytes).map_err(err)?;
                    if delta.manifest.from != hop.from || delta.manifest.version != hop.to {
                        return Err(format!("the delta for {} to {} is for {} to {}", hop.from, hop.to, delta.manifest.from, delta.manifest.version));
                    }
                    let _ = std::fs::remove_dir_all(&folder);
                    let made = out.parent().map_or(Ok(()), std::fs::create_dir_all).and_then(|()| delta.build(&base, &out));
                    if let Err(e) = made {
                        let _ = std::fs::remove_dir_all(&folder);
                        return Err(format!("{} to {}: {e}", hop.from, hop.to));
                    }
                    if let Some(previous) = previous {
                        let _ = std::fs::remove_dir_all(previous);
                    }
                    Ok(delta.manifest)
                }
            };
            let next = tauri::async_runtime::spawn_blocking(job).await.map_err(err)??;
            *last = Some((folder, out));
            removed.extend(next.removed.iter().cloned());
            manifest = Some(next);
            if cancelled.load(Ordering::SeqCst) {
                return Err(CANCELLED.into());
            }
        }
        Ok((manifest.ok_or("no deltas")?, removed))
    }
    /// Lays out a full package that `fetch` has verified: a macOS app is unpacked
    /// to be swapped in; anything else is kept for the updater to install.
    fn from_package(package: Vec<u8>, update: Update, target: Option<Target>, cache: Option<PathBuf>) -> Result<Ready, String> {
        let version = update.version.clone();
        #[cfg(target_os = "macos")]
        if let Some(Target::Bundle { bundle, stage }) = &target {
            let folder = stage.join(&version);
            let staged = folder.join(bundle.file_name().ok_or("no app name")?);
            let _ = std::fs::remove_dir_all(&folder);
            match unpack(&package, &staged) {
                Ok(()) => return Ok(Ready::Bundle { version, staged, bundle: bundle.clone() }),
                Err(e) => {
                    let _ = std::fs::remove_dir_all(&folder);
                    log(&format!("could not unpack the app, leaving it to the updater: {e}"));
                }
            }
        }
        let _ = target;
        let dir = cache.ok_or("no cache folder")?;
        std::fs::create_dir_all(&dir).map_err(err)?;
        let path = dir.join(format!("package-{version}"));
        std::fs::write(&path, &package).map_err(err)?;
        Ok(Ready::Package { version, update: Box::new(update), path })
    }

    /// Unpacks a `.app.tar.gz` (one app folder at the top) as `out`.
    #[cfg(target_os = "macos")]
    fn unpack(package: &[u8], out: &Path) -> std::io::Result<()> {
        std::fs::create_dir_all(out)?;
        let mut archive = tar::Archive::new(flate2::read::GzDecoder::new(package));
        for entry in archive.entries()? {
            let mut entry = entry?;
            let path = entry.path()?.into_owned();
            let inside: PathBuf = path.components().skip(1).collect();
            if inside.as_os_str().is_empty() {
                continue;
            }
            if !inside.components().all(|c| matches!(c, std::path::Component::Normal(_))) {
                return Err(std::io::Error::other(format!("unsafe path {}", path.display())));
            }
            let dest = out.join(&inside);
            if let Some(parent) = dest.parent() {
                std::fs::create_dir_all(parent)?;
            }
            entry.unpack(&dest)?;
        }
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(out, std::fs::Permissions::from_mode(0o755))
    }

    /// Something to download, and the version it is signed for.
    struct Item {
        url: String,
        signature: String,
        version: String,
    }

    /// How far a download has come is reported as part of a larger one: what
    /// came before it, and the total of all (or just its own size).
    struct Report<'a> {
        app: &'a AppHandle,
        before: u64,
        total: Option<u64>,
    }

    /// Fetches `item` through each route in turn until one delivers bytes that
    /// pass the signature check. Without a report it is something small, which
    /// gets the time a check does.
    async fn fetch(update: &Update, routes: &[String], item: &Item, report: Option<Report<'_>>, cancelled: Option<&AtomicBool>) -> Result<Vec<u8>, String> {
        let stopped = || cancelled.is_some_and(|c| c.load(Ordering::SeqCst));
        let mut failure = String::from("nowhere to download from");
        for prefix in routes {
            let mut update = update.clone();
            update.download_url = format!("{prefix}{}", item.url).parse().map_err(err)?;
            update.signature = item.signature.clone();
            update.version = item.version.clone();
            // Packages and deltas are watched for stalls below rather than timed.
            update.timeout = report.is_none().then_some(CHECK_TIMEOUT);
            let alive = Arc::new(Mutex::new(Instant::now()));
            let beat = alive.clone();
            let progress = report.as_ref().map(|r| (r.app.clone(), r.before, r.total));
            let (mut downloaded, mut told) = (0u64, None::<Instant>);
            let job = update.download(
                move |chunk, length| {
                    downloaded += chunk as u64;
                    let now = Instant::now();
                    *beat.lock().unwrap() = now;
                    let Some((app, before, total)) = &progress else { return };
                    if told.is_none_or(|at| now.duration_since(at) >= REPORT_EVERY) || Some(downloaded) == length {
                        told = Some(now);
                        let _ = app.emit(PROGRESS, Progress { downloaded: before + downloaded, total: total.or(length), step: "download" });
                    }
                },
                || {},
            );
            let mut job = std::pin::pin!(job);
            let outcome = loop {
                match tokio::time::timeout(TICK, job.as_mut()).await {
                    Ok(done) => break done.map_err(err),
                    Err(_) if stopped() => return Err(CANCELLED.into()),
                    Err(_) if alive.lock().unwrap().elapsed() > STALL => break Err("the download stalled".into()),
                    Err(_) => {}
                }
            };
            match outcome {
                // Checking the signature of a large package takes a moment in
                // which nothing above looks at the flag.
                Ok(_) if stopped() => return Err(CANCELLED.into()),
                Ok(bytes) => return Ok(bytes),
                Err(e) => failure = e,
            }
        }
        Err(failure)
    }
    /// Puts a prepared release in place. Installing a full package on Windows
    /// starts the installer and ends this process; `restart` has the installer
    /// start the new version.
    fn apply(ready: &Ready, restart: bool) -> Result<(), String> {
        match ready {
            Ready::Bundle { staged, bundle, .. } => {
                sift_delta::swap(bundle, staged).map_err(err)?;
                // Finder and Launch Services notice the new app sooner.
                let _ = std::process::Command::new("touch").arg(bundle).status();
                Ok(())
            }
            Ready::Files { version, staged } => {
                staged.commit().map_err(err)?;
                #[cfg(windows)]
                registered_version(version);
                let _ = version;
                Ok(())
            }
            Ready::Package { update, path, .. } => {
                let package = std::fs::read(path).map_err(err)?;
                let update = (**update).clone().restart_after_install(restart);
                update.install(package).map_err(err)
            }
        }
    }

    /// The version Windows lists under installed apps, which the installer
    /// wrote and also reads to tell an update from a downgrade.
    #[cfg(windows)]
    fn registered_version(version: &str) {
        const KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Uninstall\Sift";
        for root in [windows_registry::CURRENT_USER, windows_registry::LOCAL_MACHINE] {
            if let Ok(key) = root.options().read().write().open(KEY) {
                if key.get_string("DisplayVersion").is_ok() {
                    let _ = key.set_string("DisplayVersion", version);
                    return;
                }
            }
        }
    }

    pub async fn install(app: &AppHandle) -> Result<(), String> {
        download(app).await?;
        let pending = app.state::<Pending>();
        let ready = pending.ready.lock().unwrap().take().ok_or("the update is not ready")?;
        let _ = app.emit(PROGRESS, Progress { downloaded: 0, total: None, step: "install" });
        // On macOS the updater may ask for an administrator password through
        // the main thread and waits for it, so this must not run there.
        let (applied, ready) = tauri::async_runtime::spawn_blocking(move || (apply(&ready, true), ready)).await.map_err(err)?;
        if let Err(e) = applied {
            *pending.ready.lock().unwrap() = Some(ready);
            return Err(e);
        }
        // Windows never gets here after a full package: the installer has started and the app ended.
        app.restart()
    }

    /// Puts a prepared release in place as the app quits.
    pub fn on_exit(app: &AppHandle) {
        let Some(pending) = app.try_state::<Pending>() else { return };
        let Some(ready) = pending.ready.lock().unwrap().take() else { return };
        // The updater would ask for a password through an event loop that has ended.
        if cfg!(target_os = "macos") && matches!(ready, Ready::Package { .. }) {
            return;
        }
        if let Err(e) = apply(&ready, false) {
            log(&format!("could not put {} in place: {e}", ready.version()));
        }
    }

    pub fn cancel(app: &AppHandle) {
        app.state::<Pending>().cancelled.store(true, Ordering::SeqCst);
    }

    pub fn discard(app: &AppHandle) {
        if let Some(ready) = app.state::<Pending>().ready.lock().unwrap().take() {
            std::thread::spawn(move || ready.discard());
        }
    }

    /// With `SIFT_UPDATE_AUTO` set, a debug build updates itself at launch
    /// without asking, so the whole path can be tested where no window can be
    /// clicked: `restart` puts the update in place and restarts at once,
    /// `quit` downloads it and quits, leaving it to be put in place on the way out.
    #[cfg(debug_assertions)]
    pub fn unattended(app: &AppHandle) {
        let Some(mode) = std::env::var_os("SIFT_UPDATE_AUTO") else { return };
        if !ENABLED {
            return;
        }
        let app = app.clone();
        tauri::async_runtime::spawn(async move {
            match check(&app).await {
                Ok(Some(offer)) => {
                    eprintln!("update: {} -> {} (delta: {:?})", offer.current, offer.version, offer.delta_size);
                    if mode == "quit" {
                        match download(&app).await {
                            Ok(()) => {
                                eprintln!("update: ready, quitting");
                                app.exit(0);
                            }
                            Err(e) => eprintln!("update: download failed: {e}"),
                        }
                    } else if let Err(e) = install(&app).await {
                        eprintln!("update: install failed: {e}");
                    }
                }
                Ok(None) => eprintln!("update: {} is the newest", app.package_info().version),
                Err(e) => eprintln!("update: check failed: {e}"),
            }
        });
    }
}
