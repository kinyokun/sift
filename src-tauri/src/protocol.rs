//! `sift://` image protocol: thumbnails, screen-size renditions,
//! full-resolution previews and the clipping warning for files inside
//! opened folders.

use crate::state::{cache_file, cached, AppState};
use percent_encoding::percent_decode_str;
use sift_core::{formats, preview, render, Error};
use std::path::Path;
use tauri::http::{header, Response, StatusCode};

const THUMB_QUALITY: u8 = 80;
const SCREEN_QUALITY: u8 = 88;
const DEVELOP_QUALITY: u8 = 92;

fn query(uri: &tauri::http::Uri, key: &str) -> Option<String> {
    uri.query()?.split('&').find_map(|kv| {
        let (k, v) = kv.split_once('=')?;
        (k == key).then(|| percent_decode_str(v).decode_utf8_lossy().into_owned())
    })
}

/// A full-resolution development of a RAW that keeps the camera's look, cached on disk.
pub fn developed(state: &AppState, path: &Path) -> sift_core::Result<Vec<u8>> {
    cached(&cache_file(&state.cache_dir, "dev", path, 0), || render::matched(path, DEVELOP_QUALITY).map_err(render::explain))
}

/// The RAW rendered without the in-camera look, cached on disk.
pub fn original(state: &AppState, path: &Path) -> sift_core::Result<Vec<u8>> {
    cached(&cache_file(&state.cache_dir, "orig", path, 0), || render::original(path, DEVELOP_QUALITY).map_err(render::explain))
}

/// Part of the cache key of rendered looks; bump when look rendering changes.
const LOOK_RENDER_VERSION: u32 = 6;

/// Develops a NEF with Nikon's own renderer: the camera's Standard Picture
/// Control for "no look" and `.cube` looks, the Picture Control itself for
/// NP3 looks. A custom tone curve in an NP3 is developed by the SDK too, from
/// a copy of the file with the look recorded in it; only where the file
/// cannot take it is the curve applied afterwards by us.
fn nikon_pixels(state: &AppState, engine: &sift_core::nikon::Engine, path: &Path, look: &str, edge: u32) -> Result<image::RgbImage, String> {
    use sift_core::nikon::{Base, Recipe};
    let look = if look == NO_LOOK { None } else { Some(state.look(look)?) };
    let recipe = look.as_ref().and_then(|l| l.np3.as_ref()).map_or_else(|| Recipe::base(Base::Standard), |np3| np3.recipe());
    let sift_core::nikon::Rendered { image: mut img, recorded } = engine.render(path, &recipe, edge).map_err(|e| e.to_string())?;
    match look.as_deref() {
        Some(sift_core::look::Look { np3: Some(_), .. }) if recorded => {}
        Some(sift_core::look::Look { np3: Some(np3), .. }) => np3.apply_curve(&mut img),
        Some(cube) => cube.lut.apply(&mut img),
        None => {}
    }
    Ok(img)
}

/// Marks a photo that should be shown without any look.
pub const NO_LOOK: &str = "original";

/// Upright pixels of `path` with a library look applied (or, for
/// [`NO_LOOK`], the plain rendering). RAW files start from the rendering
/// without the in-camera look, so looks are not stacked on top of each other.
pub fn look_pixels(state: &AppState, path: &Path, look: &str, edge: u32) -> Result<image::RgbImage, String> {
    if let Some(engine) = state.nikon_for(path) {
        return nikon_pixels(state, &engine, path, look, edge);
    }
    let mut img = if formats::is_raw(path) {
        let full = preview::decode_upright(&original(state, path).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
        if edge > 0 && full.width().max(full.height()) > edge {
            let k = edge as f32 / full.width().max(full.height()) as f32;
            image::imageops::thumbnail(&full, (full.width() as f32 * k) as u32, (full.height() as f32 * k) as u32)
        } else {
            full
        }
    } else {
        preview::pixels(path, if edge == 0 { u32::MAX } else { edge }).map_err(|e| e.to_string())?
    };
    if look != NO_LOOK {
        state.look(look)?.lut.apply(&mut img);
    }
    Ok(img)
}

/// Long edge of the copy the histogram is counted from.
pub const HISTOGRAM_EDGE: u32 = 512;
/// Long edge of the clipping warning laid over the photo.
const CLIP_EDGE: u32 = 1280;

/// A small upright copy (long edge about `size`) of what the viewer shows for
/// `path` (`look` and `edge` as in the viewer's image request), for the
/// histogram and the clipping warning.
pub fn analysis_pixels(state: &AppState, path: &Path, look: Option<&str>, edge: u32, size: u32) -> sift_core::Result<image::RgbImage> {
    match look {
        None => preview::pixels(path, size),
        // The viewer shows the cached "orig" rendition here, not a look render.
        Some(NO_LOOK) if formats::is_raw(path) && state.nikon_for(path).is_none() => preview::decode_upright_at(&original(state, path)?, size),
        Some(look) => preview::decode_upright_at(&look_jpeg(state, path, look, edge.min(MAX_LOOK_EDGE))?, size),
    }
}

/// The clipping warning for what the viewer shows: a transparent PNG with
/// blown highlights in red and crushed shadows in blue.
fn clipping_png(state: &AppState, path: &Path, look: Option<&str>, edge: u32) -> sift_core::Result<Vec<u8>> {
    use image::ImageEncoder;
    let mask = sift_core::histogram::clipping_mask(&analysis_pixels(state, path, look, edge, CLIP_EDGE)?);
    let mut out = Vec::new();
    image::codecs::png::PngEncoder::new(&mut out)
        .write_image(mask.as_raw(), mask.width(), mask.height(), image::ExtendedColorType::Rgba8)
        .map_err(|e| Error::Decode(e.to_string()))?;
    Ok(out)
}

const MAX_LOOK_EDGE: u32 = 4096;

fn look_jpeg(state: &AppState, path: &Path, look: &str, edge: u32) -> sift_core::Result<Vec<u8>> {
    // Renders by Nikon's SDK and by our own approximation never share a cache entry.
    let engine = if state.nikon_for(path).is_some() { "n" } else { "" };
    let kind = format!("look{LOOK_RENDER_VERSION}{engine}-{}", crate::state::path_hash(Path::new(look)));
    cached(&cache_file(&state.cache_dir, &kind, path, edge), || {
        let img = look_pixels(state, path, look, edge).map_err(Error::Decode)?;
        preview::encode_jpeg(&img, if edge == 0 { DEVELOP_QUALITY } else { SCREEN_QUALITY })
    })
}

fn image(state: &AppState, uri: &tauri::http::Uri) -> Result<Vec<u8>, String> {
    let path = state.check(&query(uri, "p").ok_or("missing path")?)?;
    let edge: u32 = query(uri, "e").and_then(|e| e.parse().ok()).unwrap_or(0);
    let kind = uri.path().trim_matches('/');
    let out = match kind {
        "thumb" => {
            let edge = edge.clamp(64, 1024);
            cached(&cache_file(&state.cache_dir, "thumb", &path, edge), || preview::scaled(&path, edge, THUMB_QUALITY))
        }
        "screen" => {
            let edge = edge.clamp(512, 4096);
            // Cameras embed a screen-size preview in RAW and JPEG files alike;
            // it is sent as is. Only files without one are scaled (and cached).
            match preview::embedded_screen(&path, edge) {
                Ok(Some(data)) => Ok(data),
                _ => cached(&cache_file(&state.cache_dir, "screen", &path, edge), || preview::scaled(&path, edge, SCREEN_QUALITY)),
            }
        }
        "full" => match preview::full(&path) {
            // RAW files without a usable JPEG (HEVC previews, some DNGs) are developed instead.
            Err(Error::NoPreview) if formats::is_raw(&path) => developed(state, &path),
            other => other,
        },
        // The movie of a Live Photo, whole: the page plays it from memory.
        "live" if path.extension().is_some_and(|x| x.eq_ignore_ascii_case("mov")) => std::fs::read(&path).map_err(Error::from),
        "dev" => developed(state, &path),
        "orig" => original(state, &path),
        "look" => look_jpeg(state, &path, &query(uri, "l").ok_or("missing look")?, edge.min(MAX_LOOK_EDGE)),
        "clip" => clipping_png(state, &path, query(uri, "l").as_deref(), edge),
        _ => return Err(format!("unknown image kind: {kind}")),
    };
    out.map_err(|e| e.to_string())
}

pub fn respond(state: &AppState, uri: &tauri::http::Uri) -> Response<Vec<u8>> {
    match image(state, uri) {
        Ok(data) => Response::builder()
            .header(header::CONTENT_TYPE, preview::mime_of(&data))
            // URLs carry the file's modification time, so responses never go stale.
            .header(header::CACHE_CONTROL, "private, max-age=31536000, immutable")
            .header(header::ACCESS_CONTROL_ALLOW_ORIGIN, "*")
            .body(data),
        Err(msg) => Response::builder()
            .status(StatusCode::NOT_FOUND)
            .header(header::CONTENT_TYPE, "text/plain; charset=utf-8")
            .header(header::ACCESS_CONTROL_ALLOW_ORIGIN, "*")
            .body(msg.into_bytes()),
    }
    .expect("static headers are valid")
}
