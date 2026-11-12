//! Full-resolution renderings of RAW files, picking the best decoder the
//! platform offers.

use crate::{Error, Result};
use std::path::Path;

fn orientation_of(path: &Path) -> u8 {
    crate::io::Src::open(path).map_or(1, |src| crate::exif::orientation(&crate::container::parse(&src)))
}

/// The photo without the in-camera look (Picture Control, recipe, film
/// simulation), as a JPEG whose EXIF orientation makes it display upright
/// (decode with [`crate::preview::decode_upright`] to get upright pixels).
///
/// Apple platforms use the system RAW engine, which also reads formats the
/// bundled decoder cannot (Nikon High Efficiency). Elsewhere the bundled
/// decoder produces a plain neutral rendering.
pub fn original(path: &Path, quality: u8) -> Result<Vec<u8>> {
    #[cfg(target_vendor = "apple")]
    if let Ok(jpeg) = crate::apple::render_jpeg(path, orientation_of(path), quality as f32 / 100.0) {
        return Ok(jpeg);
    }
    #[cfg(feature = "develop")]
    {
        let img = crate::preview::orient(crate::develop::neutral(path)?, orientation_of(path));
        crate::preview::encode_jpeg(&img, quality)
    }
    #[cfg(not(feature = "develop"))]
    Err(Error::Unsupported("RAW decoding is not available in this build"))
}

/// A full-resolution rendering that keeps the camera's look, for RAW files
/// whose embedded preview is too small to judge focus.
pub fn matched(path: &Path, quality: u8) -> Result<Vec<u8>> {
    #[cfg(feature = "develop")]
    let bundled: Result<Vec<u8>> = crate::develop::develop(path)
        .and_then(|img| crate::preview::encode_jpeg(&crate::preview::orient(img, orientation_of(path)), quality));
    #[cfg(not(feature = "develop"))]
    let bundled: Result<Vec<u8>> = Err(Error::Unsupported("RAW decoding is not available in this build"));
    #[cfg(target_vendor = "apple")]
    if bundled.is_err() {
        return crate::apple::render_jpeg(path, orientation_of(path), quality as f32 / 100.0);
    }
    bundled
}

/// Formats nothing in this build can decode get a clear message instead of a
/// decoder's internal error.
pub fn explain(e: Error) -> Error {
    match &e {
        Error::Decode(msg) if msg.contains("not supported") || msg.contains("HighEfficency") => {
            Error::Unsupported("this RAW compression cannot be decoded on this platform")
        }
        _ => e,
    }
}
