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
use std::fs::{self, OpenOptions};
use std::io::{Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

const SONY_RATING: u16 = 0x2002;
/// A JPEG marker segment holds at most 65533 payload bytes.
const MAX_APP1_BODY: usize = 65533;

#[derive(serde::Serialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Stored {
    /// Written into the image file itself.
    Embedded,
    /// Written to a `.xmp` sidecar next to the image.
    Sidecar,
}

/// Existing sidecar for `path`, or the name a new one should get.
pub fn sidecar_path(path: &Path) -> PathBuf {
    for ext in ["xmp", "XMP"] {
        let p = path.with_extension(ext);
        if p.is_file() {
            return p;
        }
    }
    path.with_extension("xmp")
}

/// Absolute position of the Sony MakerNote Rating value (a u32 stored inline).
fn sony_rating_pos(p: &Parsed) -> Option<(u64, bool)> {
    let (t, ifd) = container::sony_maker_ifd(p)?;
    let e = ifd.entries.into_iter().find(|e| e.tag == SONY_RATING && e.typ == 4 && e.count == 1)?;
    Some((e.pos + 8, t.le))
}

pub fn read(src: &Src, p: &Parsed, path: &Path) -> u8 {
    let embedded = || {
        let r = p.xmp?;
        xmp::read_rating(&src.read(r.pos, r.len.min(1 << 20))?)
    };
    let sidecar = || xmp::read_rating(&fs::read(sidecar_path(path)).ok()?);
    let sony = || {
        let (pos, le) = sony_rating_pos(p)?;
        let v = crate::io::u32_at(&src.read(pos, 4)?, 0, le)?;
        (1..=5).contains(&v).then_some(v as u8)
    };
    let found = match p.format {
        Format::Jpeg => embedded().or_else(sidecar),
        _ => sidecar().or_else(embedded).or_else(sony),
    };
    found.unwrap_or(0)
}

fn overwrite(path: &Path, pos: u64, bytes: &[u8]) -> std::io::Result<()> {
    let mut f = OpenOptions::new().write(true).open(path)?;
    f.seek(SeekFrom::Start(pos))?;
    f.write_all(bytes)?;
    crate::io::sync(&f)
}

fn replace_file(path: &Path, parts: &[&[u8]]) -> std::io::Result<()> {
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

fn write_jpeg(path: &Path, src: &Src, info: &jpeg::JpegInfo, rating: u8) -> Result<()> {
    if let Some((pos, len)) = info.xmp {
        let old = src.read(pos, len).ok_or(Error::Malformed)?;
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

/// Writes a 0..=5 rating for `path`, keeping the file's modification time.
pub fn write(path: &Path, rating: u8) -> Result<Stored> {
    if rating > 5 {
        return Err(Error::Unsupported("rating must be 0-5"));
    }
    let mtime = fs::metadata(path).map(|m| filetime::FileTime::from_last_modification_time(&m))?;
    let stored = write_inner(path, rating);
    let _ = filetime::set_file_mtime(path, mtime);
    stored
}

fn write_inner(path: &Path, rating: u8) -> Result<Stored> {
    let src = Src::open(path)?;
    let p = container::parse(&src);
    if p.format == Format::Jpeg {
        let info = p.jpeg.as_ref().ok_or(Error::Malformed)?;
        write_jpeg(path, &src, info, rating)?;
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
