//! A Live Photo made of a movie that is already encoded: the frames are
//! copied into a QuickTime movie (`mov`) and a JPEG becomes the still
//! (`still`, or `heic` when it is to be a HEIC), both marked as one Live
//! Photo. None of this needs the system, so it is the same on every platform;
//! on Windows the movie comes from Media Foundation (`mf`).

use super::{heic, mov, staged, still, Still};
use crate::{preview, Error, Result};
use std::path::{Path, PathBuf};

/// The picture of the still as a JPEG: the frame's own file when it is one,
/// so that nothing it records is lost, and the JPEG that is shown otherwise.
fn cover(frame: &Path) -> Result<Vec<u8>> {
    let shown = preview::full(frame)?;
    if shown.starts_with(&[0xFF, 0xD8]) {
        return Ok(shown);
    }
    // PNG and the like are shown as they are.
    preview::encode_jpeg(&image::load_from_memory(&shown)?.to_rgb8(), 95)
}

/// The still of the pair, from the JPEG of its picture: that JPEG with the
/// identifier added, which keeps the picture and all the file records (its
/// own maker note excepted), or a HEIC of it, which encodes the picture again.
fn pictured(cover: &[u8], identifier: &str, format: Still) -> Result<Vec<u8>> {
    match format {
        Still::Jpeg => still::tagged(cover, identifier),
        Still::Heic => heic::encode(cover, identifier),
    }
}

/// Makes a Live Photo of a movie that already exists: `<stem>.JPG` (or
/// `.HEIC`, as `format` says) and `<stem>.MOV` in `dest`, both carrying
/// `identifier` (a UUID, see [`identifier`](super::identifier)). The frames
/// of `movie` (H.264 in an MP4 or QuickTime file) are copied, not encoded
/// again, and `cover` is the picture the still shows, the frame `moment`
/// seconds into the movie. Returns the paths of the still and the movie;
/// existing files are never replaced.
pub fn from_movie(movie: &Path, cover: &Path, identifier: &str, moment: f64, format: Still, dest: &Path, stem: &str) -> Result<(PathBuf, PathBuf)> {
    let shaped = |b: (usize, u8)| if matches!(b.0, 8 | 13 | 18 | 23) { b.1 == b'-' } else { b.1.is_ascii_hexdigit() };
    if identifier.len() != 36 || !identifier.bytes().enumerate().all(shaped) {
        return Err(Error::Unsupported("the identifier of a Live Photo is a UUID"));
    }
    staged(dest, stem, format, |still_path, movie_path| {
        mov::remux(movie, movie_path, identifier, moment, None)?;
        Ok(std::fs::write(still_path, pictured(&self::cover(cover)?, identifier, format)?)?)
    })
}

/// Writes `frames` (one displayable file per shot, in shooting order) as
/// `<stem>.JPG` (or `.HEIC`, as `format` says) and `<stem>.MOV` in `dest`.
/// `key` is the frame that becomes the still. Returns the paths of the still
/// and the movie; nothing is left behind on failure, and existing files of
/// those names are never replaced. A JPEG still is the frame's own JPEG with
/// the identifier added, not a new encoding.
#[cfg(windows)]
#[allow(clippy::too_many_arguments)]
pub fn export(frames: &[PathBuf], taken: &[Option<i64>], key: usize, format: Still, dest: &Path, stem: &str, cancel: &std::sync::atomic::AtomicBool, mut on_frame: impl FnMut(usize)) -> Result<(PathBuf, PathBuf)> {
    use super::{enough, frame, identifier, mf, timeline};
    use std::sync::atomic::Ordering;
    enough(frames)?;
    let key = key.min(frames.len() - 1);
    staged(dest, stem, format, |still_path, movie_path| {
        let id = identifier(&frames[key]);
        let cover = cover(&frames[key])?;
        let (times, duration) = timeline(taken);
        let step = duration / frames.len() as f64;
        let mut encode = || {
            // Media Foundation writes to a local file of its own; the destination
            // may be a network share or a path it does not take.
            let encoded = Scratch(std::env::temp_dir().join(format!("sift-live-{}-{id}.mp4", std::process::id())));
            let first = frame(&frames[0], None)?;
            let size = first.dimensions();
            let mut encoder = mf::Encoder::new(&encoded.0, size, step)?;
            let mut first = Some(first);
            for (i, path) in frames.iter().enumerate() {
                if cancel.load(Ordering::Relaxed) {
                    return Err(Error::Cancelled);
                }
                on_frame(i);
                let pixels = match first.take() {
                    Some(pixels) => pixels,
                    None => frame(path, Some(size))?,
                };
                encoder.write(&nv12(&pixels), times[i])?;
            }
            encoder.finish()?;
            // The still's moment is the middle of the time its frame is shown.
            mov::remux(&encoded.0, movie_path, &id, times[key] + step / 2.0, Some(mov::BT709))
        };
        // A HEIC of a large picture takes seconds, so the still is made while
        // the movie is; a cancelled movie waits for it.
        let (still, movie) = std::thread::scope(|scope| {
            let still = scope.spawn(|| pictured(&cover, &id, format));
            let movie = encode();
            (still.join(), movie)
        });
        movie?;
        let still = still.map_err(|_| Error::Decode("the still could not be written".into()))??;
        Ok(std::fs::write(still_path, still)?)
    })
}

/// Without an encoder for the movie there is no Live Photo to write.
#[cfg(not(any(target_vendor = "apple", windows)))]
#[allow(clippy::too_many_arguments)]
pub fn export(_: &[PathBuf], _: &[Option<i64>], _: usize, _: Still, _: &Path, _: &str, _: &std::sync::atomic::AtomicBool, _: impl FnMut(usize)) -> Result<(PathBuf, PathBuf)> {
    Err(Error::Unsupported("Live Photos cannot be made on this system"))
}

/// A temporary file, removed when it is no longer needed.
#[cfg(windows)]
struct Scratch(PathBuf);

#[cfg(windows)]
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

/// The picture as the encoder takes it: NV12, a plane of luma followed by
/// one of interleaved chroma at half the size, in BT.709 with video levels
/// (16-235). Both sides are even.
#[cfg(any(windows, test))]
fn nv12(img: &image::RgbImage) -> Vec<u8> {
    // BT.709 weights scaled to video levels, in 16-bit fixed point.
    const Y: [i32; 3] = [11966, 40254, 4064];
    const CB: [i32; 3] = [-6597, -22187, 28784];
    const CR: [i32; 3] = [28784, -26148, -2636];
    let (w, h) = (img.width() as usize, img.height() as usize);
    let rgb = img.as_raw();
    let mut out = vec![0u8; w * h * 3 / 2];
    let (luma, chroma) = out.split_at_mut(w * h);
    for (y, px) in luma.iter_mut().zip(rgb.as_chunks::<3>().0) {
        *y = ((Y[0] * px[0] as i32 + Y[1] * px[1] as i32 + Y[2] * px[2] as i32 + (1 << 15)) >> 16) as u8 + 16;
    }
    for (row, pairs) in chroma.chunks_exact_mut(w).enumerate() {
        for (x, pair) in pairs.as_chunks_mut::<2>().0.iter_mut().enumerate() {
            // The four pixels that share this chroma sample.
            let mut sum = [0i32; 3];
            for at in [row * 2 * w + x * 2, row * 2 * w + x * 2 + 1, (row * 2 + 1) * w + x * 2, (row * 2 + 1) * w + x * 2 + 1] {
                for (total, value) in sum.iter_mut().zip(&rgb[at * 3..at * 3 + 3]) {
                    *total += *value as i32;
                }
            }
            let part = |k: [i32; 3]| (((k[0] * sum[0] + k[1] * sum[1] + k[2] * sum[2] + (1 << 17)) >> 18) + 128) as u8;
            *pair = [part(CB), part(CR)];
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::super::{movie_identifier, movie_of};
    use super::*;

    const ID: &str = "58813388-F52F-4098-9F81-35A7A9D1CF94";

    #[test]
    fn a_movie_and_a_picture_become_a_pair() {
        let dir = std::env::temp_dir().join(format!("sift-live-pair-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let (source, picture) = (dir.join("burst.mp4"), dir.join("DSC_0001.JPG"));
        std::fs::write(&source, mov::tests::mp4(&[b"one", b"two", b"three"], false)).unwrap();
        let camera = still::tests::with_exif(&still::tests::picture(), &still::tests::camera(true, Some(&[b'N'; 120])));
        std::fs::write(&picture, &camera).unwrap();

        let (still, movie) = from_movie(&source, &picture, ID, 0.1, Still::Jpeg, &dir, "DSC_0001_live").unwrap();
        assert_eq!((still.clone(), movie.clone()), (dir.join("DSC_0001_live.JPG"), dir.join("DSC_0001_live.MOV")));
        // The reader takes the two for a Live Photo, and the cover for none.
        assert_eq!(movie_identifier(&movie).unwrap().as_deref(), Some(ID));
        assert_eq!(movie_of(&still, &movie), Some(movie.clone()));
        assert_eq!(movie_of(&picture, &movie), None);
        // The source files are as they were and nothing else is left.
        assert_eq!(std::fs::read(&picture).unwrap(), camera);
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 4);

        // Never over an existing pair, and not with something that is no identifier.
        assert!(from_movie(&source, &picture, ID, 0.1, Still::Jpeg, &dir, "DSC_0001_live").is_err());
        assert!(from_movie(&source, &picture, "not-an-identifier", 0.1, Still::Jpeg, &dir, "other").is_err());
        // A failure leaves nothing behind.
        assert!(from_movie(&picture, &picture, ID, 0.1, Still::Jpeg, &dir, "other").is_err());
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 4);

        // A picture that is no JPEG is made one.
        let png = dir.join("drawing.png");
        image::RgbImage::from_pixel(8, 8, image::Rgb([200, 100, 50])).save(&png).unwrap();
        let (still, movie) = from_movie(&source, &png, ID, 0.0, Still::Jpeg, &dir, "drawing_live").unwrap();
        assert_eq!(movie_of(&still, &movie), Some(movie));
        assert_eq!(image::open(&still).unwrap().to_rgb8().dimensions(), (8, 8));

        // The still as a HEIC: the same pair under the other name.
        #[cfg(not(any(target_os = "ios", target_os = "android")))]
        {
            let (still, movie) = from_movie(&source, &picture, ID, 0.1, Still::Heic, &dir, "DSC_0001_small").unwrap();
            assert_eq!((still.clone(), movie.clone()), (dir.join("DSC_0001_small.HEIC"), dir.join("DSC_0001_small.MOV")));
            assert_eq!(movie_of(&still, &movie), Some(movie));
            assert_eq!(crate::heif::decode(&still, u32::MAX).unwrap().dimensions(), (16, 16));
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn frames_go_to_the_encoder_in_video_levels() {
        let colours = [[255, 255, 255], [0, 0, 0], [255, 0, 0], [0, 0, 255]];
        // Four blocks of 2x2, one per colour, side by side.
        let img = image::RgbImage::from_fn(8, 2, |x, _| image::Rgb(colours[(x / 2) as usize]));
        let out = nv12(&img);
        assert_eq!(out.len(), 8 * 2 * 3 / 2);
        assert_eq!(out[..8], [235, 235, 16, 16, 63, 63, 32, 32]);
        assert_eq!(out[8..16], out[..8]);
        assert_eq!(out[16..], [128, 128, 128, 128, 102, 240, 240, 118]);
    }

    /// The whole way with the system's encoder, which only Windows has here.
    /// With `SIFT_LIVE_OUT` naming a folder, the pair is also left there, to
    /// be looked at or handed to Photos.
    #[cfg(windows)]
    #[test]
    fn a_burst_is_encoded_into_a_pair() {
        use std::sync::atomic::AtomicBool;
        let dir = std::env::temp_dir().join(format!("sift-live-encode-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        // A bright square that moves across a gradient.
        let burst = |name: &str, (w, h): (u32, u32), count: u32| -> Vec<PathBuf> {
            (0..count)
                .map(|i| {
                    let path = dir.join(format!("{name}_{i:04}.JPG"));
                    let square = |x: u32, y: u32| (w / 8 + i * w / 16..w / 4 + i * w / 16).contains(&x) && (h / 3..h / 2).contains(&y);
                    let img = image::RgbImage::from_fn(w, h, |x, y| if square(x, y) { image::Rgb([240, 200, 40]) } else { image::Rgb([(x * 255 / w) as u8, (y * 255 / h) as u8, 90]) });
                    std::fs::write(&path, preview::encode_jpeg(&img, 90).unwrap()).unwrap();
                    path
                })
                .collect()
        };
        let keep = |files: [&PathBuf; 2]| {
            if let Some(out) = std::env::var_os("SIFT_LIVE_OUT") {
                std::fs::create_dir_all(&out).unwrap();
                for file in files {
                    std::fs::copy(file, Path::new(&out).join(file.file_name().unwrap())).unwrap();
                }
            }
        };
        let going = AtomicBool::new(false);

        // The sizes and paces of real bursts: the movie's full size at 20 frames
        // a second, and an upright one at the fastest pace there is, its still a HEIC.
        for (name, size, count, gap, format) in [("WIDE", (1920, 1280), 12u32, 50i64, Still::Jpeg), ("TALL", (1280, 1920), 12, 8, Still::Heic)] {
            let frames = burst(name, size, count);
            let taken: Vec<Option<i64>> = (0..count as i64).map(|i| Some(i * gap)).collect();
            let (still, movie) = export(&frames, &taken, 5, format, &dir, &format!("{name}_live"), &going, |_| {}).unwrap();
            assert_eq!(still.extension().unwrap().to_str(), Some(format.ext()));
            assert_eq!(movie_of(&still, &movie), Some(movie.clone()));
            // What the encoder wrote, the decoder here reads back.
            assert_eq!(preview::pixels(&still, u32::MAX).unwrap().dimensions(), size);
            let (frames_written, seconds) = mov::tests::length(&movie);
            assert_eq!(frames_written, count as usize);
            let expected = super::super::timeline(&taken).1;
            assert!((seconds - expected).abs() < 0.02, "{name}: {seconds} s, not {expected}");
            keep([&still, &movie]);
            for file in frames.iter().chain([&still, &movie]) {
                std::fs::remove_file(file).unwrap();
            }
        }

        // 322 x 242, so that the sides are no multiple of 16.
        let frames = burst("DSC", (322, 242), 5);
        let taken: Vec<Option<i64>> = (0..5).map(|i| Some(i * 100)).collect();

        let mut seen = Vec::new();
        let (still, movie) = export(&frames, &taken, 2, Still::Jpeg, &dir, "DSC_0002_live", &going, |i| seen.push(i)).unwrap();
        assert_eq!(seen, [0, 1, 2, 3, 4]);
        assert_eq!((still.clone(), movie.clone()), (dir.join("DSC_0002_live.JPG"), dir.join("DSC_0002_live.MOV")));
        assert_eq!(movie_of(&still, &movie), Some(movie.clone()));
        // The still is the frame's own picture, and the movie holds every frame at the pace of the burst.
        assert_eq!(image::open(&still).unwrap().to_rgb8(), image::open(&frames[2]).unwrap().to_rgb8());
        let (count, seconds) = mov::tests::length(&movie);
        assert_eq!(count, 5);
        assert!((seconds - 0.5).abs() < 0.02, "{seconds}");
        keep([&still, &movie]);

        // No pair over another, and stopping leaves nothing.
        let listed = || std::fs::read_dir(&dir).unwrap().count();
        assert!(export(&frames, &taken, 2, Still::Jpeg, &dir, "DSC_0002_live", &going, |_| {}).is_err());
        going.store(true, std::sync::atomic::Ordering::Relaxed);
        assert!(matches!(export(&frames, &taken, 2, Still::Jpeg, &dir, "other", &going, |_| {}), Err(Error::Cancelled)));
        assert_eq!(listed(), 7);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
