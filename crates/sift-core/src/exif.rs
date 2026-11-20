use crate::container::{self, Parsed};
use crate::io::Src;
use crate::jpeg;
use crate::tiff::{Entry, Ifd, Tiff, T_EXIF_IFD, T_ORIENTATION};
use serde::Serialize;

#[derive(Serialize, Clone, Debug, Default)]
pub struct Exif {
    pub make: Option<String>,
    pub model: Option<String>,
    pub lens: Option<String>,
    /// Focal length in millimetres.
    pub focal: Option<f32>,
    pub aperture: Option<f32>,
    /// Exposure time in seconds.
    pub shutter: Option<f64>,
    pub iso: Option<u32>,
    /// Exposure compensation in EV.
    pub bias: Option<f32>,
    /// `YYYY-MM-DD HH:MM:SS`
    pub datetime: Option<String>,
    pub width: Option<u32>,
    pub height: Option<u32>,
    /// Focal length converted to the 35 mm format, when the camera recorded it.
    pub focal35: Option<u32>,
    /// EXIF ExposureProgram (1 manual, 2 program, 3 aperture priority, 4 shutter priority, ...).
    pub program: Option<u8>,
    /// EXIF MeteringMode (2 centre-weighted, 3 spot, 5 matrix, ...).
    pub metering: Option<u8>,
    /// White balance set by hand rather than automatically.
    pub manual_wb: Option<bool>,
    /// The flash fired.
    pub flash: Option<bool>,
    /// Who took it and the copyright notice, as set in the camera.
    pub artist: Option<String>,
    pub copyright: Option<String>,
}

pub fn orientation(p: &Parsed) -> u8 {
    // A HEIF's own rotation counts, not the one in its EXIF block.
    if let Some(heif) = p.heif {
        return heif.orientation;
    }
    p.ifd0
        .as_ref()
        .and_then(|(t, i)| i.get(T_ORIENTATION).and_then(|e| t.uint(e)))
        .filter(|v| (1..=8).contains(v))
        .unwrap_or(1) as u8
}

/// Whether the camera recorded the picture in Adobe RGB rather than sRGB.
/// DCF has a camera say so with ColorSpace "uncalibrated" and the
/// interoperability index R03; some cameras leave the index out (Canon
/// CR3, Olympus), a few write ColorSpace 2, and Nikon, Canon and Pentax RAW
/// files may say it in the MakerNote alone. "Uncalibrated" with R98, DCF's
/// sRGB, is sRGB.
pub fn adobe_rgb(p: &Parsed) -> bool {
    // With a profile, "uncalibrated" says to look there.
    if p.jpeg.as_ref().is_some_and(|info| info.icc) {
        return false;
    }
    let named = p.exif.as_ref().and_then(|(t, exif)| exif.get(0xA001).map(|_| adobe_rgb_in(t, exif)));
    named.unwrap_or(false) || maker_adobe_rgb(p).unwrap_or(false) || (named.is_none() && preview_adobe_rgb(p))
}

/// What the largest preview's own EXIF says: Panasonic RAW files keep their
/// EXIF block whole only in the JPEG they embed.
fn preview_adobe_rgb(p: &Parsed) -> bool {
    let (Some((t, _)), Some(j)) = (&p.ifd0, p.largest_preview()) else { return false };
    jpeg::scan(t.src, j.offset).filter(|info| !info.icc).and_then(|info| info.exif_base).is_some_and(|base| adobe_rgb_at(t.src, base))
}

fn adobe_rgb_in(t: &Tiff, exif: &Ifd) -> bool {
    match exif.get(0xA001).and_then(|e| t.uint(e)) {
        Some(2) => true,
        Some(0xFFFF) => {
            let interop = exif.get(0xA005).and_then(|e| t.uint(e)).and_then(|at| t.ifd(at));
            interop.and_then(|i| t.ascii(i.get(0x0001)?)).is_none_or(|index| index != "R98")
        }
        _ => false,
    }
}

/// [`adobe_rgb`] by the EXIF block (TIFF header first) at `base` alone.
pub fn adobe_rgb_at(src: &Src, base: u64) -> bool {
    let Some((t, off)) = Tiff::open(src, base) else { return false };
    t.ifd(off).and_then(|ifd0| t.uint(ifd0.get(T_EXIF_IFD)?)).and_then(|at| t.ifd(at)).is_some_and(|exif| adobe_rgb_in(&t, &exif))
}

/// The ColorSpace of a Nikon MakerNote (0x001E, 2 for Adobe RGB), a Canon
/// one (0x00B4, likewise) or a Pentax one (0x0037, 1 for Adobe RGB).
fn maker_adobe_rgb(p: &Parsed) -> Option<bool> {
    let mn = p.maker.as_ref()?;
    if let Some((t, off)) = container::nikon_maker_tiff(mn.parent.src, mn) {
        return Some(t.uint(t.ifd(off)?.get(0x001E)?)? == 2);
    }
    if let Some((crate::pentax::Kind::Pentax, t, ifd)) = crate::pentax::maker_ifd(mn) {
        return Some(t.uint(ifd.get(0x0037)?)? == 1);
    }
    let (t, e) = find(p, 0x010F)?;
    if !t.ascii(e)?.starts_with("Canon") {
        return None;
    }
    let t = &mn.parent;
    Some(t.uint(t.ifd_at(mn.pos)?.get(0x00B4)?)? == 2)
}

/// Looks a tag up in the Exif IFD first, then IFD0.
fn find<'p, 'a>(p: &'p Parsed<'a>, tag: u16) -> Option<(&'p Tiff<'a>, &'p Entry)> {
    [&p.exif, &p.ifd0]
        .into_iter()
        .flatten()
        .find_map(|(t, i): &(Tiff, Ifd)| i.get(tag).map(|e| (t, e)))
}

pub fn read(p: &Parsed) -> Exif {
    let mut x = Exif::default();
    let pick = |tag: u16| find(p, tag);
    let text = |tag| pick(tag).and_then(|(t, e)| t.ascii(e));
    let num = |tag| pick(tag).and_then(|(t, e)| t.number(e));

    x.make = text(0x010F);
    x.model = text(0x0110);
    x.lens = text(0xA434);
    x.aperture = num(0x829D).filter(|v| *v > 0.3 && *v <= 128.0).map(|v| v as f32);
    x.shutter = num(0x829A).filter(|v| *v > 0.0 && *v < 36000.0);
    x.focal = num(0x920A).filter(|v| *v > 0.0 && *v < 5000.0).map(|v| v as f32);
    x.bias = num(0x9204).filter(|v| v.abs() < 30.0).map(|v| v as f32);
    x.iso = [0x8827u16, 0x8833, 0x8832]
        .into_iter()
        .filter_map(|tag| pick(tag).and_then(|(t, e)| t.uint(e)))
        .find(|v| *v > 0 && *v != 65535);
    x.datetime = text(0x9003).or_else(|| text(0x0132)).map(|s| {
        // "YYYY:MM:DD HH:MM:SS" -> "YYYY-MM-DD HH:MM:SS"
        let (d, t) = s.split_once(' ').unwrap_or((&s, ""));
        format!("{} {}", d.replace(':', "-"), t).trim().to_string()
    });
    let small = |tag: u16| pick(tag).and_then(|(t, e)| t.uint(e));
    x.focal35 = small(0xA405).filter(|v| (1..5000).contains(v));
    x.program = small(0x8822).filter(|v| (1..=8).contains(v)).map(|v| v as u8);
    x.metering = small(0x9207).filter(|v| (1..=6).contains(v)).map(|v| v as u8);
    x.manual_wb = small(0xA403).map(|v| v == 1);
    x.flash = small(0x9209).map(|v| v & 1 == 1);
    x.artist = text(0x013B);
    // The photographer's notice comes first; an editor's may follow after a NUL.
    x.copyright = text(0x8298);
    x.width = pick(0xA002).and_then(|(t, e)| t.uint(e)).filter(|v| *v > 0);
    x.height = pick(0xA003).and_then(|(t, e)| t.uint(e)).filter(|v| *v > 0);
    if x.width.is_none() || x.height.is_none() {
        if let Some(j) = p.largest_preview() {
            x.width = Some(j.width);
            x.height = Some(j.height);
        }
    }
    x
}
