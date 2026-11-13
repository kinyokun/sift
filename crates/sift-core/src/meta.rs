use crate::af::{self, AfInfo};
use crate::container::{self, Format};
use crate::exif::{self, Exif};
use crate::io::Src;
use crate::{formats, rating, stack, Result};
use serde::Serialize;
use std::path::Path;

/// Embedded previews at or below this long edge are too small to judge focus.
pub const LOW_RES_EDGE: u32 = 2048;

#[derive(Serialize, Clone, Debug)]
pub struct Meta {
    pub format: Format,
    pub raw: bool,
    /// EXIF orientation (1-8) of the sensor data.
    pub orientation: u8,
    pub rating: u8,
    pub exif: Exif,
    pub af: Option<AfInfo>,
    pub look: Option<Look>,
    /// The file keeps its Picture Control where `nikon::record_look` can write
    /// another one, tone curve included.
    pub look_recordable: bool,
    /// Pixel size of the largest embedded preview, before rotation.
    pub preview_width: u32,
    pub preview_height: u32,
    /// True when a RAW only embeds a small preview and needs a full decode to check focus.
    pub low_res: bool,
}

/// The in-camera look (Nikon Picture Control) a photo was rendered with.
#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct Look {
    pub name: String,
    /// The built-in Picture Control this look is derived from.
    pub base: String,
    /// True for a user or Imaging Cloud recipe rather than a built-in look.
    pub custom: bool,
}

fn title_case(s: &str) -> String {
    s.split_whitespace()
        .map(|w| {
            let mut c = w.chars();
            c.next().map_or(String::new(), |f| f.to_uppercase().chain(c.flat_map(char::to_lowercase)).collect())
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Nikon PictureControlData (MakerNote tag 0x0023): two 20-byte names whose
/// position depends on the record version.
fn nikon_look(p: &container::Parsed) -> Option<Look> {
    let mn = p.maker.as_ref()?;
    let (t, ifd_off) = container::nikon_maker_tiff(mn.parent.src, mn)?;
    let d = t.data(t.ifd(ifd_off)?.get(0x0023)?)?;
    let at = match d.get(..2)? {
        b"01" | b"02" => 4,
        b"03" => 8,
        _ => return None,
    };
    let text = |o: usize| {
        let raw = d.get(o..o + 20)?;
        let end = raw.iter().position(|&b| b == 0).unwrap_or(raw.len());
        let s = String::from_utf8_lossy(&raw[..end]).trim().to_string();
        (!s.is_empty()).then_some(s)
    };
    let (name, base) = (text(at)?, text(at + 20).unwrap_or_default());
    let custom = !base.is_empty() && !name.eq_ignore_ascii_case(&base);
    // Built-in names are stored in capitals; recipes keep the name they were given.
    Some(Look { name: if custom { name } else { title_case(&name) }, base: title_case(&base), custom })
}

/// Whether the MakerNote has the three values `nikon::record_look` rewrites.
fn nikon_recordable(p: &container::Parsed) -> bool {
    (|| {
        let mn = p.maker.as_ref()?;
        let (t, ifd_off) = container::nikon_maker_tiff(mn.parent.src, mn)?;
        let ifd = t.ifd(ifd_off)?;
        let long = |tag: u16, min: usize| ifd.get(tag).is_some_and(|e| t.byte_len(e) >= min);
        Some(long(0x0023, 100) && long(0x005c, 72) && long(0x008c, 578))
    })()
    .unwrap_or(false)
}

/// Just the star rating, read without probing embedded previews.
pub fn rating(path: &Path) -> Result<u8> {
    let src = Src::open(path)?;
    let p = container::parse_metadata(&src);
    Ok(rating::read(&src, &p, path))
}

/// What the photo grid sorts and filters by, read in one pass over the metadata.
#[derive(Debug, Clone, Default, Serialize)]
pub struct Brief {
    pub rating: u8,
    /// Capture time as "YYYY-MM-DD HH:MM:SS" (sorts as text), when the file records one.
    pub date: Option<String>,
    /// What the file says about the burst or pixel-shift series it belongs to.
    #[serde(skip)]
    pub series: stack::Hint,
}

pub fn brief(path: &Path) -> Result<Brief> {
    let src = Src::open(path)?;
    let p = container::parse_metadata(&src);
    Ok(Brief { rating: rating::read(&src, &p, path), date: exif::read(&p).datetime, series: stack::hint(&p, path) })
}

pub fn read(path: &Path) -> Result<Meta> {
    let src = Src::open(path)?;
    let p = container::parse(&src);
    let orientation = exif::orientation(&p);
    let mut exif = exif::read(&p);
    let af = match (&exif.make, &exif.model) {
        (Some(make), model) => af::read(&p, make, model.as_deref().unwrap_or(""), orientation),
        _ => None,
    };
    let raw = formats::is_raw(path);
    let (mut pw, mut ph) = p.largest_preview().map_or((0, 0), |j| (j.width, j.height));
    if p.previews.is_empty() && !raw {
        if let Ok((w, h)) = image::image_dimensions(path) {
            (pw, ph) = (w, h);
            exif.width = Some(w);
            exif.height = Some(h);
        }
    }
    Ok(Meta {
        format: p.format,
        raw,
        orientation,
        rating: rating::read(&src, &p, path),
        look: nikon_look(&p),
        look_recordable: nikon_recordable(&p),
        exif,
        af,
        preview_width: pw,
        preview_height: ph,
        low_res: raw && pw.max(ph) <= LOW_RES_EDGE,
    })
}
