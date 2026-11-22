//! Star ratings (`xmp:Rating`, Sony MakerNote Rating).
//!
//! Writes never move image data. RAW containers are only ever patched with
//! same-length bytes; when that is impossible the rating goes to a `.xmp`
//! sidecar. JPEG metadata segments may be resized, which rewrites the file
//! through a temp file and an atomic rename.

use crate::container::{self, Format, Parsed};
use crate::io::Src;
use crate::jpeg;
use crate::xmp::{self, Patch};
use crate::{Error, Result};
use serde::{Deserialize, Serialize};
use std::fs::{self, OpenOptions};
use std::io::{Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

const SONY_RATING: u16 = 0x2002;
/// A JPEG marker segment holds at most 65533 payload bytes.
const MAX_APP1_BODY: usize = 65533;

#[derive(Serialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Stored {
    /// Written into the image file itself.
    Embedded,
    /// Written to a `.xmp` sidecar next to the image.
    Sidecar,
}

/// Existing sidecar for `path`, or the name a new one should get.
pub fn sidecar_path(path: &Path) -> PathBuf {
    existing_sidecar(path).unwrap_or_else(|| path.with_extension("xmp"))
}

fn existing_sidecar(path: &Path) -> Option<PathBuf> {
    ["xmp", "XMP"].into_iter().map(|ext| path.with_extension(ext)).find(|p| p.is_file())
}

/// Absolute position of the Sony MakerNote Rating value (a u32 stored inline).
fn sony_rating_pos(p: &Parsed) -> Option<(u64, bool)> {
    let (t, ifd) = container::sony_maker_ifd(p)?;
    let e = ifd.entries.into_iter().find(|e| e.tag == SONY_RATING && e.typ == 4 && e.count == 1)?;
    Some((e.pos + 8, t.le))
}

pub fn read(src: &Src, p: &Parsed, path: &Path) -> u8 {
    read_beside(src, p, existing_sidecar(path).as_deref())
}

/// [`read`] with the sidecar already found (`None`: there is none), as one
/// listing of the folder tells for all its photos: on a network share,
/// looking for it costs a round trip or three per photo.
pub fn read_beside(src: &Src, p: &Parsed, sidecar: Option<&Path>) -> u8 {
    read_places(src, &Places::of(p), sidecar)
}

/// Where a file keeps its stars. Stars are written in place, in as many bytes
/// as before, or else the file is written anew in another size; so a file of
/// the size it had can have them read again from here, without being parsed.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Places {
    jpeg: bool,
    xmp: Option<(u64, usize)>,
    sony: Option<(u64, bool)>,
}

impl Places {
    pub fn of(p: &Parsed) -> Places {
        Places { jpeg: p.format == Format::Jpeg, xmp: p.xmp.map(|r| (r.pos, r.len.min(1 << 20))), sony: sony_rating_pos(p) }
    }
}

/// The rating read at `at`, as [`read_beside`] finds it; `None` when no
/// XMP packet is where there was one, and the file has to be parsed.
pub fn read_at(src: &Src, at: &Places, sidecar: Option<&Path>) -> Option<u8> {
    if let Some((pos, len)) = at.xmp {
        crate::io::find(&src.read(pos, len)?, b"xmpmeta")?;
    }
    Some(read_places(src, at, sidecar))
}

fn read_places(src: &Src, at: &Places, sidecar: Option<&Path>) -> u8 {
    let embedded = || {
        let (pos, len) = at.xmp?;
        xmp::read_rating(&src.read(pos, len)?)
    };
    let sidecar = || xmp::read_rating(&fs::read(sidecar?).ok()?);
    let sony = || {
        let (pos, le) = at.sony?;
        let v = crate::io::u32_at(&src.read(pos, 4)?, 0, le)?;
        (1..=5).contains(&v).then_some(v as u8)
    };
    let found = if at.jpeg { embedded().or_else(sidecar) } else { sidecar().or_else(embedded).or_else(sony) };
    found.unwrap_or(0)
}

fn overwrite(path: &Path, pos: u64, bytes: &[u8]) -> std::io::Result<()> {
    let mut f = OpenOptions::new().write(true).open(path)?;
    f.seek(SeekFrom::Start(pos))?;
    f.write_all(bytes)?;
    crate::io::sync(&f)
}

pub(crate) fn replace_file(path: &Path, parts: &[&[u8]]) -> std::io::Result<()> {
    let tmp = path.with_extension(format!(
        "{}.sift-tmp",
        path.extension().and_then(|e| e.to_str()).unwrap_or("")
    ));
    let result = (|| {
        let mut f = fs::File::create(&tmp)?;
        for part in parts {
            f.write_all(part)?;
        }
        crate::io::sync(&f)?;
        if let Ok(perm) = fs::metadata(path).map(|m| m.permissions()) {
            let _ = fs::set_permissions(&tmp, perm);
        }
        fs::rename(&tmp, path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result
}

fn write_sidecar(path: &Path, rating: u8, create: bool) -> Result<bool> {
    let side = sidecar_path(path);
    let data = match fs::read(&side) {
        // Clearing a rating removes a sidecar that holds nothing but our rating.
        Ok(old) if rating == 0 && crate::io::find(&old, b"x:xmptk=\"Sift\"").is_some() => {
            fs::remove_file(&side)?;
            return Ok(true);
        }
        Ok(old) => match xmp::set_rating(&old, rating) {
            Some(Patch::Same(v)) | Some(Patch::Resized(v)) => v,
            None => xmp::minimal_packet(rating),
        },
        Err(_) if create => xmp::minimal_packet(rating),
        Err(_) => return Ok(false),
    };
    replace_file(&side, &[&data])?;
    Ok(true)
}

/// `xmp` is the file's XMP packet, where it is and what it says, read before the file was closed.
fn write_jpeg(path: &Path, info: &jpeg::JpegInfo, xmp: Option<(u64, usize, Vec<u8>)>, rating: u8) -> Result<()> {
    if let Some((pos, len, old)) = xmp {
        match xmp::set_rating(&old, rating) {
            Some(Patch::Same(v)) => return Ok(overwrite(path, pos, &v)?),
            Some(Patch::Resized(v)) if v.len() + jpeg::XMP_HEADER.len() <= MAX_APP1_BODY => {
                let file = fs::read(path)?;
                let seg_start = (pos as usize) - jpeg::XMP_HEADER.len() - 4;
                let seg_len = ((v.len() + jpeg::XMP_HEADER.len() + 2) as u16).to_be_bytes();
                replace_file(
                    path,
                    &[&file[..seg_start + 2], &seg_len, jpeg::XMP_HEADER, &v, &file[pos as usize + len..]],
                )?;
                return Ok(());
            }
            _ => return Err(Error::Unsupported("XMP packet cannot be updated")),
        }
    }
    // No XMP yet: add a segment after the leading APP0/APP1 block.
    let at = info
        .segments
        .iter()
        .take_while(|s| matches!(s.marker, 0xE0 | 0xE1))
        .last()
        .map_or(2, |s| s.pos as usize + s.len as usize);
    let packet = xmp::minimal_packet(rating);
    let seg_len = ((packet.len() + jpeg::XMP_HEADER.len() + 2) as u16).to_be_bytes();
    let file = fs::read(path)?;
    replace_file(path, &[&file[..at], &[0xFF, 0xE1], &seg_len, jpeg::XMP_HEADER, &packet, &file[at..]])?;
    Ok(())
}

/// How long to wait, one after the other, for a photo that something has open
/// for a moment (decoding it, Explorer, a virus scanner) before giving up.
const BUSY_WAITS: [u64; 5] = [50, 100, 200, 400, 800];

/// The file is open elsewhere in a way that keeps it from being written or replaced.
fn busy(e: &Error) -> bool {
    let Error::Io(e) = e else { return false };
    if cfg!(windows) {
        // Access denied (what replacing an open file gives), sharing and lock violations.
        matches!(e.raw_os_error(), Some(5 | 32 | 33))
    } else {
        e.kind() == std::io::ErrorKind::ResourceBusy
    }
}

/// Writes a 0..=5 rating for `path`, keeping the file's modification time.
pub fn write(path: &Path, rating: u8) -> Result<Stored> {
    if rating > 5 {
        return Err(Error::Unsupported("rating must be 0-5"));
    }
    let mtime = fs::metadata(path).map(|m| filetime::FileTime::from_last_modification_time(&m))?;
    let mut stored = write_inner(path, rating);
    for wait in BUSY_WAITS {
        if !stored.as_ref().is_err_and(busy) {
            break;
        }
        std::thread::sleep(Duration::from_millis(wait));
        stored = write_inner(path, rating);
    }
    let _ = filetime::set_file_mtime(path, mtime);
    if stored.is_ok() {
        // The rating is in the photo by now; a sidecar of Nikon's that cannot
        // be written is no reason to report the rating as failed.
        let _ = crate::nksc::sync(path, rating);
    }
    stored
}

fn write_inner(path: &Path, rating: u8) -> Result<Stored> {
    let src = Src::open(path)?;
    let p = container::parse(&src);
    if p.format == Format::Jpeg {
        let info = p.jpeg.clone().ok_or(Error::Malformed)?;
        let xmp = match info.xmp {
            Some((pos, len)) => Some((pos, len, src.read(pos, len).ok_or(Error::Malformed)?)),
            None => None,
        };
        // Where Windows cannot rename over an open file (FAT, exFAT, network
        // shares), our own handle would keep the file from being replaced.
        drop(p);
        drop(src);
        write_jpeg(path, &info, xmp, rating)?;
        // Keep an existing sidecar in step so the two never disagree.
        write_sidecar(path, rating, false)?;
        return Ok(Stored::Embedded);
    }

    let mut embedded = false;
    if let Some(r) = p.xmp.filter(|r| r.len <= 1 << 20) {
        let old = src.read(r.pos, r.len).ok_or(Error::Malformed)?;
        if let Some(Patch::Same(v)) = xmp::set_rating(&old, rating) {
            overwrite(path, r.pos, &v)?;
            embedded = true;
        }
    }
    if let Some((pos, le)) = sony_rating_pos(&p) {
        let v = rating as u32;
        overwrite(path, pos, &if le { v.to_le_bytes() } else { v.to_be_bytes() })?;
        embedded = true;
    }
    let sidecar = write_sidecar(path, rating, !embedded)?;
    Ok(if embedded || !sidecar { Stored::Embedded } else { Stored::Sidecar })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn jpeg_in(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("sift-{name}-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("photo.jpg");
        let img = image::RgbImage::from_pixel(64, 48, image::Rgb([120, 90, 60]));
        fs::write(&path, crate::preview::encode_jpeg(&img, 90).unwrap()).unwrap();
        path
    }

    fn rating_of(path: &Path) -> u8 {
        let src = Src::open(path).unwrap();
        read(&src, &container::parse(&src), path)
    }

    #[test]
    fn a_jpeg_being_read_takes_a_rating() {
        let path = jpeg_in("rating-read");
        let reader = fs::File::open(&path).unwrap();
        // No XMP yet: the file is written anew.
        assert_eq!(write(&path, 3).unwrap(), Stored::Embedded);
        drop(reader);
        assert_eq!(rating_of(&path), 3);
        let _ = fs::remove_dir_all(path.parent().unwrap());
    }

    #[cfg(windows)]
    #[test]
    fn a_jpeg_held_for_a_moment_takes_a_rating_once_let_go() {
        use std::os::windows::fs::OpenOptionsExt;
        const FILE_SHARE_READ: u32 = 1;
        let path = jpeg_in("rating-held");
        // Open as a program that lets nobody replace the file would have it.
        let held = OpenOptions::new().read(true).share_mode(FILE_SHARE_READ).open(&path).unwrap();
        let release = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(300));
            drop(held);
        });
        assert_eq!(write(&path, 4).unwrap(), Stored::Embedded);
        release.join().unwrap();
        assert_eq!(rating_of(&path), 4);
        let _ = fs::remove_dir_all(path.parent().unwrap());
    }
}
