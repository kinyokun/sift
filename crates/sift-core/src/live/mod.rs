//! Live Photos: a still and a short QuickTime movie that belong together.
//! What ties them is a content identifier, a UUID stored in the still (in its
//! Apple maker note) and in the movie's metadata. Recognising a pair works
//! everywhere; writing one needs the system's encoder (`writer`, Apple only).
//! A pair is either two files side by side (what Sift writes) or one Live
//! Photo bundle (`.pvt`, what AirDrop between Macs sometimes leaves).

use crate::Result;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

#[cfg(target_vendor = "apple")]
mod writer;
#[cfg(target_vendor = "apple")]
pub use writer::*;

const IDENTIFIER_KEY: &[u8] = b"com.apple.quicktime.content.identifier";
/// A movie's index (`moov`) is small; anything larger is not read.
const MAX_INDEX: u64 = 16 << 20;
/// The still's identifier sits in its EXIF block, near the start of the file.
const STILL_HEAD: u64 = 1 << 20;

/// How long a frame stays up when the files do not say when they were taken.
const DEFAULT_STEP: f64 = 0.1;
/// Frames are shown at the pace they were shot, up to the fastest bursts
/// cameras shoot (120 a second); a slow burst is sped up so that it does not
/// turn into a slide show.
const STEP_RANGE: (f64, f64) = (1.0 / 120.0, 0.25);

/// When each frame appears, in seconds from the first, and when the movie ends.
/// `taken` holds the capture times in milliseconds where the files record them.
pub fn timeline(taken: &[Option<i64>]) -> (Vec<f64>, f64) {
    let gaps: Vec<f64> = taken.windows(2).filter_map(|w| Some((w[1]? - w[0]?) as f64 / 1000.0)).filter(|g| *g >= 0.0).collect();
    let mut usual: Vec<f64> = gaps.iter().copied().filter(|g| *g > 0.0).collect();
    usual.sort_by(f64::total_cmp);
    // One pace for the whole movie: the real rate of the burst. Cameras record
    // the time to a hundredth of a second, so single gaps are off by up to that
    // much and only their average is right. Pauses (a full buffer) are left out
    // so that they do not slow everything down.
    let step = usual.get(usual.len() / 2).map_or(DEFAULT_STEP, |median| {
        let steady: Vec<f64> = gaps.iter().copied().filter(|g| *g <= median * 2.0).collect();
        steady.iter().sum::<f64>() / steady.len() as f64
    });
    let step = step.clamp(STEP_RANGE.0, STEP_RANGE.1);
    ((0..taken.len()).map(|i| i as f64 * step).collect(), taken.len() as f64 * step)
}

/// Extension of the package AirDrop uses for a Live Photo between Macs: a
/// folder holding the still, the movie (both with the same name) and
/// `metadata.plist`. Only Apple's own tools know it; anywhere else it is a
/// folder of two files.
pub const BUNDLE_EXT: &str = "pvt";

/// Whether `path` names a Live Photo bundle (by its extension).
pub fn is_bundle(path: &Path) -> bool {
    path.extension().is_some_and(|e| e.eq_ignore_ascii_case(BUNDLE_EXT))
}

/// The still and the movie inside a Live Photo bundle.
pub fn bundle_members(bundle: &Path) -> Option<(PathBuf, PathBuf)> {
    let (mut still, mut movie) = (None, None);
    for entry in std::fs::read_dir(bundle).ok()?.flatten() {
        let p = entry.path();
        if p.file_name().and_then(|n| n.to_str()).is_none_or(|n| n.starts_with('.')) {
            continue;
        }
        if p.extension().is_some_and(|x| x.eq_ignore_ascii_case("mov")) {
            movie = Some(p);
        } else if crate::formats::is_supported(&p) {
            still = Some(p);
        }
    }
    Some((still?, movie?))
}

fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

/// The first text shaped like a UUID (`8-4-4-4-12` hex digits).
fn uuid_in(data: &[u8]) -> Option<&[u8]> {
    data.windows(36).find(|w| w.iter().enumerate().all(|(i, b)| if matches!(i, 8 | 13 | 18 | 23) { *b == b'-' } else { b.is_ascii_hexdigit() }))
}

/// The content identifier of a QuickTime movie, if it has one.
pub fn movie_identifier(path: &Path) -> Result<Option<String>> {
    let mut file = File::open(path)?;
    let length = file.metadata()?.len();
    let mut at = 0u64;
    // Walk the top-level boxes to the index; the media data is skipped unread.
    while at + 8 <= length {
        let mut head = [0u8; 16];
        file.seek(SeekFrom::Start(at))?;
        file.read_exact(&mut head[..8])?;
        let mut size = u32::from_be_bytes([head[0], head[1], head[2], head[3]]) as u64;
        let mut header = 8;
        if size == 1 {
            file.read_exact(&mut head[8..])?;
            size = u64::from_be_bytes(head[8..].try_into().unwrap());
            header = 16;
        } else if size == 0 {
            size = length - at;
        }
        if size < header || at + size > length {
            break;
        }
        if &head[4..8] == b"moov" {
            if size > MAX_INDEX {
                break;
            }
            let mut index = vec![0u8; (size - header) as usize];
            file.read_exact(&mut index)?;
            let Some(key) = find(&index, IDENTIFIER_KEY) else { break };
            // The value follows the list of keys.
            return Ok(uuid_in(&index[key..]).map(|u| String::from_utf8_lossy(u).into_owned()));
        }
        at += size;
    }
    Ok(None)
}

/// Whether the still carries this content identifier.
pub fn still_has(path: &Path, identifier: &str) -> Result<bool> {
    let mut head = Vec::new();
    File::open(path)?.take(STILL_HEAD).read_to_end(&mut head)?;
    Ok(find(&head, identifier.as_bytes()).is_some())
}

/// The movie that makes `still` a Live Photo: the file of the same name
/// ending in `.mov`, if both carry the same content identifier.
pub fn movie_of(still: &Path, movie: &Path) -> Option<PathBuf> {
    let id = movie_identifier(movie).ok()??;
    still_has(still, &id).ok()?.then(|| movie.to_path_buf())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_movie_keeps_the_pace_of_the_burst_within_bounds() {
        // Seven frames a second, with one pause.
        let (times, duration) = timeline(&[Some(0), Some(140), Some(280), Some(900), Some(1040)]);
        assert!((times[1] - 0.14).abs() < 1e-9 && (times[4] - 0.56).abs() < 1e-9);
        assert!((duration - 0.7).abs() < 1e-9);
        // Fifteen a second, the times rounded to a hundredth as cameras write them: a second of shooting is a second of movie.
        let shot: Vec<Option<i64>> = (0..16).map(|i| Some((i as f64 * 1000.0 / 15.0 / 10.0).floor() as i64 * 10)).collect();
        assert!((timeline(&shot).0[15] - 1.0).abs() < 0.01);
        // One frame a second would be a slide show.
        assert!((timeline(&[Some(0), Some(1000), Some(2000)]).1 - 0.75).abs() < 1e-9);
        // 120 a second: several frames share a hundredth, and a second is still a second.
        let fast: Vec<Option<i64>> = (0..121).map(|i| Some((i as f64 * 1000.0 / 120.0 / 10.0).floor() as i64 * 10)).collect();
        assert!((timeline(&fast).0[120] - 1.0).abs() < 0.01);
        // No capture times at all.
        assert!((timeline(&[None, None, None]).1 - 0.3).abs() < 1e-9);
    }

    fn boxed(kind: &[u8; 4], body: &[u8]) -> Vec<u8> {
        let mut out = ((body.len() + 8) as u32).to_be_bytes().to_vec();
        out.extend_from_slice(kind);
        out.extend_from_slice(body);
        out
    }

    #[test]
    fn a_pair_is_two_files_with_the_same_identifier() {
        let dir = std::env::temp_dir().join(format!("sift-live-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let id = "58813388-F52F-4098-9F81-35A7A9D1CF94";
        let mut index = b"....keys".to_vec();
        index.extend_from_slice(IDENTIFIER_KEY);
        index.extend_from_slice(b"....ilst....data\0\0\0\x01\0\0\0\0");
        index.extend_from_slice(id.as_bytes());
        // The index comes after the media data, as the system writes it.
        let mut movie = boxed(b"ftyp", b"qt  ");
        movie.extend(boxed(b"mdat", &[7u8; 300]));
        movie.extend(boxed(b"moov", &index));
        let (mov, still, other, plain) = (dir.join("a.mov"), dir.join("a.jpg"), dir.join("b.jpg"), dir.join("c.mov"));
        std::fs::write(&mov, &movie).unwrap();
        std::fs::write(&still, [b"\xFF\xD8...Apple iOS...".as_slice(), id.as_bytes()].concat()).unwrap();
        std::fs::write(&other, b"\xFF\xD8 an ordinary picture").unwrap();
        std::fs::write(&plain, [boxed(b"ftyp", b"qt  "), boxed(b"moov", b"....mvhd")].concat()).unwrap();

        assert_eq!(movie_identifier(&mov).unwrap().as_deref(), Some(id));
        assert_eq!(movie_of(&still, &mov), Some(mov.clone()));
        assert_eq!(movie_of(&other, &mov), None);
        // A clip that merely shares the name is not part of the photo.
        assert_eq!(movie_identifier(&plain).unwrap(), None);
        assert_eq!(movie_of(&still, &plain), None);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_bundle_holds_one_still_and_one_movie() {
        let dir = std::env::temp_dir().join(format!("sift-pvt-{}", std::process::id()));
        let bundle = dir.join("IMG_1_live.pvt");
        std::fs::create_dir_all(&bundle).unwrap();
        for name in ["IMG_1_live.JPG", "IMG_1_live.MOV", "metadata.plist", "._IMG_1_live.JPG"] {
            std::fs::write(bundle.join(name), b"x").unwrap();
        }
        assert!(is_bundle(&bundle) && !is_bundle(&dir));
        assert_eq!(bundle_members(&bundle), Some((bundle.join("IMG_1_live.JPG"), bundle.join("IMG_1_live.MOV"))));
        std::fs::remove_file(bundle.join("IMG_1_live.MOV")).unwrap();
        assert_eq!(bundle_members(&bundle), None);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
