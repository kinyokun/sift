//! HDR photos. Three kinds are read: HEIF pictures coded for HDR screens
//! (PQ, as Canon writes them; HLG, as Nikon and Sony do), and pictures that
//! carry a gain map: an ordinary picture plus a smaller one that says how
//! much brighter each part of it is on a screen that can show more (ISO
//! 21496-1, which the HDR JPEGs and HEICs of phones follow; Google's Ultra
//! HDR also puts the same values in XMP).
//!
//! Webviews show gain-map JPEGs in HDR (WebKit from macOS 26, Chromium and
//! so WebView2 from version 116), so an HDR photo is handed over as one
//! ([`Picture`]): a JPEG with a gain map as it is, anything else made into
//! one. Its ordinary picture is what screens without HDR show and what
//! everything else here works on (thumbnails, the histogram, looks).

use crate::container::{self, Format, Parsed};
use crate::io::Src;
use crate::{exif, jpeg, preview, Error, Result};
use image::{imageops, ImageEncoder, RgbImage};
use serde::Serialize;
use std::path::Path;

#[derive(Serialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum Hdr {
    /// Coded as light on a screen (SMPTE ST 2084).
    Pq,
    /// Coded as light of the scene (ARIB STD-B67).
    Hlg,
    /// An ordinary picture with a gain map.
    GainMap,
}

/// The kind of HDR photo `p` (read from `src`) is, if it is one.
pub fn of(src: &Src, p: &Parsed) -> Option<Hdr> {
    match p.format {
        Format::Heif => p.hdr,
        Format::Jpeg => gain_map(src, p).map(|_| Hdr::GainMap),
        _ => None,
    }
}

/// The kind of HDR photo the file at `path` is, if it is one.
pub fn kind(path: &Path) -> Option<Hdr> {
    let src = Src::open(path).ok()?;
    of(&src, &container::parse_metadata(&src))
}

/// The namespace of an ISO 21496-1 segment (APP2).
const ISO: &[u8] = b"urn:iso:std:iso:ts:21496:-1\0";
const HDRGM: &str = "http://ns.adobe.com/hdr-gain-map/1.0/";
/// What is added to both renditions before their ratio is taken, so black
/// does not need an endless gain (the value libultrahdr uses).
const OFFSET: f32 = 1.0 / 64.0;
/// JPEG quality of a gain map: it is smooth, and its errors are small changes of light.
const GAIN_QUALITY: u8 = 85;
/// A gain map is made at half the size of the picture, at most this long.
const GAIN_EDGE: u32 = 2560;

/// What a gain map means (ISO 21496-1), each value for red, green and blue.
/// Gains and headrooms are in stops (log2), offsets in linear light.
#[derive(Clone, Debug, PartialEq)]
pub struct Gain {
    /// The gain a coded 0 stands for, and a coded 1 (after `gamma`).
    pub min: [f32; 3],
    pub max: [f32; 3],
    pub gamma: [f32; 3],
    pub base_offset: [f32; 3],
    pub alternate_offset: [f32; 3],
    /// The headroom of a screen at or below which the ordinary picture is
    /// shown as it is, and at or above which the gain is applied in full.
    pub base_headroom: f32,
    pub alternate_headroom: f32,
    /// The gain applies to the ordinary picture's colours (not the HDR one's).
    pub base_space: bool,
}

/// A big-endian reader of ISO 21496-1 metadata.
struct Reader<'a>(&'a [u8], usize);

impl Reader<'_> {
    fn take<const N: usize>(&mut self) -> Option<[u8; N]> {
        let v = self.0.get(self.1..self.1 + N)?.try_into().ok()?;
        self.1 += N;
        Some(v)
    }
}

/// `v` as a fraction with a power-of-two denominator.
fn fraction(v: f32) -> (i32, u32) {
    const D: u32 = 1 << 20;
    ((v as f64 * D as f64).round().clamp(i32::MIN as f64, i32::MAX as f64) as i32, D)
}

impl Gain {
    /// Reads the binary form (the APP2 segment's body after its namespace).
    /// `None` for an HDR base with an ordinary picture as its alternative,
    /// which is not read here.
    pub fn from_iso(d: &[u8]) -> Option<Gain> {
        let mut r = Reader(d, 0);
        let [a, b, _, _] = r.take::<4>()?;
        if u16::from_be_bytes([a, b]) != 0 {
            return None;
        }
        let [flags] = r.take::<1>()?;
        if flags & 4 != 0 {
            return None;
        }
        let common = (flags & 8 != 0).then(|| r.take::<4>().map(u32::from_be_bytes)).flatten();
        let mut part = |signed: bool| -> Option<f32> {
            let n = r.take::<4>()?;
            let d = match common {
                Some(d) => d,
                None => u32::from_be_bytes(r.take::<4>()?),
            };
            let n = if signed { i32::from_be_bytes(n) as f64 } else { u32::from_be_bytes(n) as f64 };
            (d > 0).then(|| (n / d as f64) as f32)
        };
        let base_headroom = part(false)?;
        let alternate_headroom = part(false)?;
        let channels = if flags & 0x80 != 0 { 3 } else { 1 };
        let mut v = [[0f32; 5]; 3];
        for channel in v.iter_mut().take(channels) {
            for (x, signed) in channel.iter_mut().zip([true, true, false, true, true]) {
                *x = part(signed)?;
            }
        }
        if channels == 1 {
            v = [v[0]; 3];
        }
        let field = |k: usize| [v[0][k], v[1][k], v[2][k]];
        let [min, max, gamma, base_offset, alternate_offset] = [0, 1, 2, 3, 4].map(field);
        Some(Gain { min, max, gamma, base_offset, alternate_offset, base_headroom, alternate_headroom, base_space: flags & 0x40 != 0 })
    }

    /// The binary form, version 0, with a fraction for every value.
    pub fn iso(&self) -> Vec<u8> {
        let one = [self.min, self.max, self.gamma, self.base_offset, self.alternate_offset].iter().all(|v| v[0] == v[1] && v[0] == v[2]);
        let mut out = vec![0, 0, 0, 0, if one { 0 } else { 0x80 } | if self.base_space { 0x40 } else { 0 }];
        let mut put = |v: f32| {
            let (n, d) = fraction(v);
            out.extend(n.to_be_bytes());
            out.extend(d.to_be_bytes());
        };
        put(self.base_headroom.max(0.0));
        put(self.alternate_headroom.max(0.0));
        for c in 0..if one { 1 } else { 3 } {
            for v in [self.min[c], self.max[c], self.gamma[c].max(0.0), self.base_offset[c], self.alternate_offset[c]] {
                put(v);
            }
        }
        out
    }

    /// Reads Adobe's XMP form (`hdrgm`, as Ultra HDR writes it). `None`
    /// when it is not there or the base is the HDR picture.
    pub fn from_xmp(x: &str) -> Option<Gain> {
        if !x.contains(HDRGM) || field(x, "Version").is_none() {
            return None;
        }
        if x.contains("hdrgm:BaseRenditionIsHDR=\"True\"") || x.contains("<hdrgm:BaseRenditionIsHDR>True") {
            return None;
        }
        let three = |name: &str, default: f32| -> Option<[f32; 3]> {
            match field(x, name).as_deref() {
                None => Some([default; 3]),
                Some([v]) => Some([*v; 3]),
                Some([r, g, b]) => Some([*r, *g, *b]),
                Some(_) => None,
            }
        };
        let max = three("GainMapMax", f32::NAN).filter(|m| m.iter().all(|v| v.is_finite()))?;
        let one = |name: &str, default: f32| field(x, name).and_then(|v| v.first().copied()).unwrap_or(default);
        Some(Gain {
            min: three("GainMapMin", 0.0)?,
            max,
            gamma: three("Gamma", 1.0)?,
            base_offset: three("OffsetSDR", OFFSET)?,
            alternate_offset: three("OffsetHDR", OFFSET)?,
            base_headroom: one("HDRCapacityMin", 0.0),
            alternate_headroom: one("HDRCapacityMax", max.iter().fold(0.0, |a, b| a.max(*b))),
            base_space: true,
        })
    }

    /// The XMP form of a gain map's own packet.
    pub fn xmp(&self) -> String {
        let value = |v: [f32; 3]| if v[0] == v[1] && v[0] == v[2] { format!("{}", v[0]) } else { format!("{}, {}, {}", v[0], v[1], v[2]) };
        let mut attributes = String::new();
        let mut elements = String::new();
        for (name, v) in [("GainMapMin", self.min), ("GainMapMax", self.max), ("Gamma", self.gamma), ("OffsetSDR", self.base_offset), ("OffsetHDR", self.alternate_offset)] {
            if v[0] == v[1] && v[0] == v[2] {
                attributes += &format!(" hdrgm:{name}=\"{}\"", value(v));
            } else {
                elements += &format!("<hdrgm:{name}><rdf:Seq><rdf:li>{}</rdf:li><rdf:li>{}</rdf:li><rdf:li>{}</rdf:li></rdf:Seq></hdrgm:{name}>", v[0], v[1], v[2]);
            }
        }
        format!(
            "<x:xmpmeta xmlns:x=\"adobe:ns:meta/\"><rdf:RDF xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\"><rdf:Description rdf:about=\"\" xmlns:hdrgm=\"{HDRGM}\" hdrgm:Version=\"1.0\"{attributes} hdrgm:HDRCapacityMin=\"{}\" hdrgm:HDRCapacityMax=\"{}\" hdrgm:BaseRenditionIsHDR=\"False\">{elements}</rdf:Description></rdf:RDF></x:xmpmeta>",
            self.base_headroom, self.alternate_headroom
        )
    }

    /// The coded values that leave the ordinary picture as it is.
    pub fn neutral(&self) -> [u8; 3] {
        std::array::from_fn(|c| {
            let span = self.max[c] - self.min[c];
            let at = if span > 0.0 { (-self.min[c] / span).clamp(0.0, 1.0) } else { 0.0 };
            (at.powf(self.gamma[c].max(0.01)) * 255.0).round() as u8
        })
    }

    /// The gain (a factor) a coded value stands for in channel `c`, in full.
    pub fn factor(&self, coded: u8, c: usize) -> f32 {
        let v = (coded as f32 / 255.0).powf(1.0 / self.gamma[c].max(0.01));
        (self.min[c] + (self.max[c] - self.min[c]) * v).exp2()
    }
}

/// The values of an `hdrgm` field: an attribute or an element, one value or a list of three.
fn field(x: &str, name: &str) -> Option<Vec<f32>> {
    let attribute = format!("hdrgm:{name}=");
    if let Some(at) = x.find(&attribute) {
        let rest = &x[at + attribute.len()..];
        let quote = rest.chars().next()?;
        let value = rest.get(1..)?.split(quote).next()?;
        return value.split(',').map(|v| v.trim().parse().ok()).collect();
    }
    let (open, close) = (format!("<hdrgm:{name}>"), format!("</hdrgm:{name}>"));
    let start = x.find(&open)? + open.len();
    let inner = &x[start..start + x[start..].find(&close)?];
    if !inner.contains("<rdf:li") {
        return Some(vec![inner.trim().parse().ok()?]);
    }
    inner.split("<rdf:li").skip(1).map(|li| li.split_once('>')?.1.split('<').next()?.trim().parse().ok()).collect()
}

/// Where a JPEG's gain map is and what it means; `meaning` is `None` for
/// Apple's earlier gain maps, which only Apple's software reads (their
/// strength is in the maker note of the main picture).
pub struct GainRef {
    pub pos: u64,
    pub len: u64,
    pub meaning: Option<Gain>,
}

/// The gain map of a JPEG (listed in its MPF index), if it has one.
pub fn gain_map(src: &Src, p: &Parsed) -> Option<GainRef> {
    let base = p.jpeg.as_ref()?.mpf_base?;
    container::mpf_images(src, base).into_iter().skip(1).filter(|(kind, pos, _)| !container::is_mpf_preview(*kind) && *pos > base).find_map(|(_, pos, len)| {
        let info = jpeg::scan(src, pos)?;
        let (mut iso, mut xmp, mut apple) = (None, None, false);
        for s in info.segments.iter().filter(|s| matches!(s.marker, 0xE1 | 0xE2)) {
            let body = src.read(s.pos + 4, s.len as usize - 4)?;
            if s.marker == 0xE2 && body.starts_with(ISO) {
                iso = iso.or_else(|| Gain::from_iso(&body[ISO.len()..]));
            } else if s.marker == 0xE1 && body.starts_with(jpeg::XMP_HEADER) {
                let text = String::from_utf8_lossy(&body[jpeg::XMP_HEADER.len()..]);
                xmp = xmp.or_else(|| Gain::from_xmp(&text));
                apple |= text.contains("HDRGainMap") || text.contains("hdrgainmap");
            }
        }
        let meaning = iso.or(xmp);
        (meaning.is_some() || apple).then_some(GainRef { pos, len, meaning })
    })
}

/// An HDR photo as an ordinary picture and a gain map, both upright.
#[derive(Clone, Debug)]
pub struct Picture {
    /// What screens without HDR show (sRGB).
    pub base: RgbImage,
    /// The coded gain of each part of `base`; any size, it covers the whole picture.
    pub gain: RgbImage,
    pub meaning: Gain,
}

/// The HDR photo at `path` as a [`Picture`] whose ordinary picture is at
/// most `edge` long; `None` when it is not one, or not one this build reads.
pub fn picture(path: &Path, edge: u32) -> Result<Option<Picture>> {
    let src = Src::open(path)?;
    let p = container::parse_metadata(&src);
    match (p.format, of(&src, &p)) {
        (Format::Jpeg, Some(_)) => match Picture::from_jpeg(&std::fs::read(path)?, edge)? {
            Some(picture) => Ok(Some(picture)),
            // Apple's earlier gain maps, which only the system reads.
            #[cfg(target_vendor = "apple")]
            None => rendered(path, edge),
            #[cfg(not(target_vendor = "apple"))]
            None => Ok(None),
        },
        (Format::Heif, Some(_)) => crate::heif::hdr(path, edge),
        _ => Ok(None),
    }
}

/// The HDR photo at `path` as a JPEG with a gain map (see [`picture`]), upright, at `quality`.
pub fn jpeg(path: &Path, edge: u32, quality: u8) -> Result<Option<Vec<u8>>> {
    picture(path, edge)?.map(|p| p.encode(quality, |d| d)).transpose()
}

/// An HDR photo as the system renders it, in both ways, made into a
/// [`Picture`]: the ordinary picture an ordinary decode gives, and the gain
/// map from it to the HDR one.
#[cfg(target_vendor = "apple")]
pub(crate) fn rendered(path: &Path, edge: u32) -> Result<Option<Picture>> {
    let base = crate::apple::pixels(path, edge)?;
    let (w, h) = Picture::gain_size(base.width(), base.height());
    let (light, w, h) = crate::apple::light(path, w.max(h))?;
    Ok(Some(Picture::from_renditions(base, &light, w, h)))
}
/// The values of a JPEG as coded, without any colour conversion (a gain map's).
fn decode_coded(data: &[u8]) -> Result<RgbImage> {
    let mut dec = jpeg_decoder::Decoder::new(std::io::Cursor::new(data));
    let px = dec.decode()?;
    let info = dec.info().ok_or(Error::Malformed)?;
    let rgb = match info.pixel_format {
        jpeg_decoder::PixelFormat::RGB24 => px,
        jpeg_decoder::PixelFormat::L8 => px.iter().flat_map(|&v| [v, v, v]).collect(),
        _ => return Err(Error::Unsupported("gain map of an unusual kind")),
    };
    RgbImage::from_raw(info.width as u32, info.height as u32, rgb).ok_or(Error::Malformed)
}

/// The size `img` (made for a picture `from` pixels wide) has for one `to` wide.
fn resized(img: RgbImage, from: u32, to: (u32, u32)) -> RgbImage {
    if from == to.0 {
        return img;
    }
    let k = to.0 as f64 / from.max(1) as f64;
    let (w, h) = (((img.width() as f64 * k).round() as u32).max(1), ((img.height() as f64 * k).round() as u32).max(1));
    if (w, h) == img.dimensions() { img } else { imageops::resize(&img, w, h, imageops::FilterType::Triangle) }
}

impl Picture {
    /// A JPEG with a gain map whose meaning is known, upright, its ordinary
    /// picture at most `edge` long; `None` for any other JPEG.
    pub fn from_jpeg(data: &[u8], edge: u32) -> Result<Option<Picture>> {
        let src = Src::mem(data.to_vec());
        let p = container::parse(&src);
        let Some(GainRef { pos, len, meaning: Some(meaning) }) = gain_map(&src, &p) else { return Ok(None) };
        let full = p.jpeg.as_ref().map_or(0, |j| j.width);
        let base = preview::fit(preview::decode_embedded(data, edge, exif::adobe_rgb(&p))?, edge);
        let coded = data.get(pos as usize..(pos + len) as usize).ok_or(Error::Malformed)?;
        let gain = resized(decode_coded(coded)?, full, base.dimensions());
        let orientation = exif::orientation(&p);
        Ok(Some(Picture { base: preview::orient(base, orientation), gain: preview::orient(gain, orientation), meaning }))
    }

    /// Made from the two renditions of an HDR photo: `sdr`, what screens
    /// without HDR show, and `hdr`, linear sRGB in which 1 is the white of
    /// `sdr`, `width` x `height` (any size; the gain map's).
    pub fn from_renditions(sdr: RgbImage, hdr: &[f32], width: u32, height: u32) -> Picture {
        let small = imageops::resize(&sdr, width, height, imageops::FilterType::Triangle);
        let linear: [f32; 256] = std::array::from_fn(|i| crate::color::srgb_to_linear(i as f32 / 255.0));
        let stops: Vec<f32> = small.as_raw().iter().zip(hdr).map(|(&s, &h)| ((h.max(0.0) + OFFSET) / (linear[s as usize] + OFFSET)).log2()).collect();
        let (lo, hi) = stops.iter().fold((0f32, 0f32), |(lo, hi), &v| (lo.min(v), hi.max(v)));
        // A picture no brighter in HDR still gets a range, so every value means something.
        let hi = hi.max(lo + 1.0 / 64.0);
        let coded = stops.iter().map(|v| ((v - lo) / (hi - lo) * 255.0).round().clamp(0.0, 255.0) as u8).collect();
        let meaning = Gain {
            min: [lo; 3],
            max: [hi; 3],
            gamma: [1.0; 3],
            base_offset: [OFFSET; 3],
            alternate_offset: [OFFSET; 3],
            base_headroom: 0.0,
            alternate_headroom: hi.max(1.0 / 64.0),
            base_space: true,
        };
        Picture { base: sdr, gain: RgbImage::from_raw(width, height, coded).expect("one value a channel"), meaning }
    }

    /// The size of a gain map made for a picture `width` x `height`.
    pub fn gain_size(width: u32, height: u32) -> (u32, u32) {
        let long = width.max(height).div_ceil(2).clamp(1, GAIN_EDGE);
        let k = long as f64 / width.max(height).max(1) as f64;
        (((width as f64 * k).round() as u32).max(1), ((height as f64 * k).round() as u32).max(1))
    }

    /// Both pictures through `f`, which must treat any size alike (turning,
    /// cutting to a crop given as shares of the picture).
    pub fn map(self, f: impl Fn(RgbImage) -> RgbImage) -> Picture {
        Picture { base: f(self.base), gain: f(self.gain), meaning: self.meaning }
    }

    /// Made smaller so that the ordinary picture's long edge is `edge`.
    pub fn shrink(self, edge: u32) -> Picture {
        let width = self.base.width();
        let base = crate::edit::shrink(self.base, edge);
        let gain = resized(self.gain, width, base.dimensions());
        Picture { base, gain, meaning: self.meaning }
    }

    /// Takes the gain away where the ordinary picture differs from `before`
    /// (a watermark has been laid on it), so what was laid on shows as it
    /// is on HDR screens too.
    pub fn settle(&mut self, before: &RgbImage) {
        let (bw, bh) = self.base.dimensions();
        let (gw, gh) = self.gain.dimensions();
        if before.dimensions() != (bw, bh) {
            return;
        }
        // How much each part was drawn over, 0 to 1; a gain map pixel takes the most of what it covers.
        let mut cover = vec![0f32; (gw * gh) as usize];
        for (x, y, p) in self.base.enumerate_pixels() {
            let q = before.get_pixel(x, y);
            let d = (0..3).map(|c| p[c].abs_diff(q[c])).max().unwrap_or(0);
            if d > 0 {
                let at = (y as u64 * gh as u64 / bh as u64 * gw as u64 + x as u64 * gw as u64 / bw as u64) as usize;
                cover[at] = cover[at].max((d as f32 / 16.0).min(1.0));
            }
        }
        let neutral = self.meaning.neutral();
        for (p, &k) in self.gain.pixels_mut().zip(&cover) {
            for c in 0..3 {
                p[c] = (p[c] as f32 + (neutral[c] as f32 - p[c] as f32) * k).round() as u8;
            }
        }
    }

    /// The photo as a gain-map JPEG: the ordinary picture at `quality`,
    /// passed through `described` (which may add an EXIF block), then the
    /// gain map after it.
    pub fn encode(&self, quality: u8, described: impl FnOnce(Vec<u8>) -> Vec<u8>) -> Result<Vec<u8>> {
        let primary = described(preview::encode_jpeg(&self.base, quality)?);
        Ok(assemble(primary, &gain_jpeg(&self.gain, &self.meaning)?))
    }
}

/// An APP segment: marker, length, header, body.
fn app(marker: u8, header: &[u8], body: &[u8]) -> Vec<u8> {
    let mut seg = vec![0xFF, marker];
    seg.extend(((2 + header.len() + body.len()) as u16).to_be_bytes());
    seg.extend(header);
    seg.extend(body);
    seg
}

/// Where the first segment after a JPEG's leading JFIF and EXIF ones begins.
fn after_leading(data: &[u8]) -> Option<usize> {
    let info = jpeg::scan(&Src::mem(data.to_vec()), 0)?;
    Some(info.segments.iter().take_while(|s| matches!(s.marker, 0xE0 | 0xE1)).last().map_or(2, |s| (s.pos + s.len as u64) as usize))
}

/// A gain map as a JPEG of its own, with what it means in both forms.
fn gain_jpeg(gain: &RgbImage, meaning: &Gain) -> Result<Vec<u8>> {
    let mut data = Vec::new();
    let encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut data, GAIN_QUALITY);
    if gain.pixels().all(|p| p[0] == p[1] && p[1] == p[2]) {
        let grey: Vec<u8> = gain.pixels().map(|p| p[0]).collect();
        encoder.write_image(&grey, gain.width(), gain.height(), image::ExtendedColorType::L8)?;
    } else {
        encoder.write_image(gain.as_raw(), gain.width(), gain.height(), image::ExtendedColorType::Rgb8)?;
    }
    let at = after_leading(&data).ok_or(Error::Malformed)?;
    let segments = [app(0xE1, jpeg::XMP_HEADER, meaning.xmp().as_bytes()), app(0xE2, ISO, &meaning.iso())].concat();
    data.splice(at..at, segments);
    Ok(data)
}

/// `primary` (a JPEG) with `gain` (a gain map's JPEG, with what it means)
/// after it, listed in a Multi-Picture Format index and announced in both
/// ways readers look for: ISO 21496-1's version segment, and Ultra HDR's
/// XMP (left out when `primary` has an XMP packet of its own, which a
/// reader takes for its only one).
pub fn assemble(mut primary: Vec<u8>, gain: &[u8]) -> Vec<u8> {
    let Some(at) = after_leading(&primary) else { return primary };
    let gain_info = jpeg::scan(&Src::mem(gain.to_vec()), 0);
    let segment_of = |marker: u8, header: &[u8]| {
        gain_info.as_ref().is_some_and(|i| i.segments.iter().any(|s| s.marker == marker && gain.get(s.pos as usize + 4..).is_some_and(|b| b.starts_with(header))))
    };
    let has_xmp = jpeg::scan(&Src::mem(primary.clone()), 0).is_some_and(|i| i.xmp.is_some());
    let mut head = Vec::new();
    if !has_xmp && segment_of(0xE1, jpeg::XMP_HEADER) {
        let xmp = format!(
            "<x:xmpmeta xmlns:x=\"adobe:ns:meta/\"><rdf:RDF xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\"><rdf:Description rdf:about=\"\" xmlns:Container=\"http://ns.google.com/photos/1.0/container/\" xmlns:Item=\"http://ns.google.com/photos/1.0/container/item/\" xmlns:hdrgm=\"{HDRGM}\" hdrgm:Version=\"1.0\"><Container:Directory><rdf:Seq><rdf:li rdf:parseType=\"Resource\"><Container:Item Item:Semantic=\"Primary\" Item:Mime=\"image/jpeg\"/></rdf:li><rdf:li rdf:parseType=\"Resource\"><Container:Item Item:Semantic=\"GainMap\" Item:Mime=\"image/jpeg\" Item:Length=\"{}\"/></rdf:li></rdf:Seq></Container:Directory></rdf:Description></rdf:RDF></x:xmpmeta>",
            gain.len()
        );
        head.extend(app(0xE1, jpeg::XMP_HEADER, xmp.as_bytes()));
    }
    if segment_of(0xE2, ISO) {
        // The lowest version a reader must know, and the one it was written for.
        head.extend(app(0xE2, ISO, &[0, 0, 0, 0]));
    }
    // The index: a big-endian TIFF header, then one directory of three entries
    // (version, number of pictures, where the entries are) and the two entries.
    const MPF_LEN: usize = 4 + 4 + 8 + 2 + 3 * 12 + 4 + 2 * 16;
    let total = primary.len() + head.len() + MPF_LEN;
    let tiff_at = at + head.len() + 4 + 4;
    let mut mpf = b"MM\0\x2A\0\0\0\x08\0\x03".to_vec();
    mpf.extend([0xB0, 0x00, 0, 7, 0, 0, 0, 4]);
    mpf.extend(b"0100");
    mpf.extend([0xB0, 0x01, 0, 4, 0, 0, 0, 1, 0, 0, 0, 2]);
    mpf.extend([0xB0, 0x02, 0, 7, 0, 0, 0, 32]);
    mpf.extend(50u32.to_be_bytes());
    mpf.extend(0u32.to_be_bytes());
    // The main picture (a baseline primary picture), then the gain map, of no type MPF names.
    mpf.extend(0x0003_0000u32.to_be_bytes());
    mpf.extend((total as u32).to_be_bytes());
    mpf.extend([0; 8]);
    mpf.extend(0u32.to_be_bytes());
    mpf.extend((gain.len() as u32).to_be_bytes());
    mpf.extend(((total - tiff_at) as u32).to_be_bytes());
    mpf.extend([0; 4]);
    head.extend(app(0xE2, b"MPF\0", &mpf));
    debug_assert_eq!(head.len() + primary.len(), total);
    primary.splice(at..at, head);
    primary.extend_from_slice(gain);
    primary
}

/// `data` (a JPEG) without its gain map, so that it shows as its ordinary picture everywhere.
pub fn without_gain_map(mut data: Vec<u8>) -> Vec<u8> {
    let src = Src::mem(data.clone());
    if let Some(g) = gain_map(&src, &container::parse_metadata(&src)) {
        data.truncate(g.pos as usize);
    }
    data
}

/// `primary`, made from `source` without its metadata ([`crate::edit::stripped`]),
/// with `source`'s gain map put back after it when it has one that it
/// carries the meaning of.
pub fn kept(primary: Vec<u8>, source: &[u8]) -> Vec<u8> {
    let src = Src::mem(source.to_vec());
    match gain_map(&src, &container::parse_metadata(&src)) {
        Some(GainRef { pos, len, meaning: Some(_) }) => match source.get(pos as usize..(pos + len) as usize) {
            Some(gain) => assemble(primary, gain),
            None => primary,
        },
        _ => primary,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::Rgb;

    const HLG: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/assets/hlg.heic");
    const PQ: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/assets/pq.heic");
    const PLAIN: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/assets/turned.heic");

    fn assert_close(a: &Gain, b: &Gain) {
        let near = |x: f32, y: f32| (x - y).abs() < 1e-5;
        let all = |x: [f32; 3], y: [f32; 3]| x.iter().zip(y).all(|(x, y)| near(*x, y));
        assert!(
            all(a.min, b.min) && all(a.max, b.max) && all(a.gamma, b.gamma) && all(a.base_offset, b.base_offset) && all(a.alternate_offset, b.alternate_offset),
            "{a:?} {b:?}"
        );
        assert!(near(a.base_headroom, b.base_headroom) && near(a.alternate_headroom, b.alternate_headroom) && a.base_space == b.base_space, "{a:?} {b:?}");
    }

    #[test]
    fn what_a_gain_map_means_survives_both_forms() {
        let each = Gain {
            min: [-0.5, 0.0, 0.25],
            max: [2.5, 2.25, 3.0],
            gamma: [1.0, 1.25, 1.0],
            base_offset: [OFFSET; 3],
            alternate_offset: [OFFSET, 0.0, OFFSET],
            base_headroom: 0.0,
            alternate_headroom: 2.5,
            base_space: true,
        };
        let one = Gain { min: [0.0; 3], max: [2.0; 3], gamma: [1.0; 3], alternate_offset: [OFFSET; 3], ..each.clone() };
        for g in [&each, &one] {
            assert_close(&Gain::from_iso(&g.iso()).unwrap(), g);
            assert_close(&Gain::from_xmp(&g.xmp()).unwrap(), g);
        }
        assert!(one.iso().len() < each.iso().len(), "one set of values when the channels agree");
        // A version a reader must know more to read.
        let mut newer = one.iso();
        newer[1] = 1;
        assert_eq!(Gain::from_iso(&newer), None);
        // An HDR picture with an ordinary one as its alternative.
        let mut turned = one.iso();
        turned[4] |= 4;
        assert_eq!(Gain::from_iso(&turned), None);
        assert_eq!(Gain::from_xmp(&one.xmp().replace("BaseRenditionIsHDR=\"False\"", "BaseRenditionIsHDR=\"True\"")), None);
    }

    /// A grey picture with a window in its top right quarter, white in the
    /// ordinary picture and four times as bright in the HDR one (given at
    /// half the size).
    fn scene() -> (RgbImage, Vec<f32>, u32, u32) {
        let window = |x: u32, y: u32, half: u32| x >= 48 / half && y < 32 / half;
        let sdr = RgbImage::from_fn(96, 64, |x, y| if window(x, y, 1) { Rgb([255; 3]) } else { Rgb([128; 3]) });
        let grey = crate::color::srgb_to_linear(128.0 / 255.0);
        let hdr = (0..32).flat_map(|y| (0..48).flat_map(move |x| [if window(x, y, 2) { 4.0 } else { grey }; 3])).collect();
        (sdr, hdr, 48, 32)
    }

    /// The light of `pic` at `(x, y)` of its ordinary picture, shown with the gain in full.
    fn light(pic: &Picture, x: u32, y: u32) -> f32 {
        let (gx, gy) = (x * pic.gain.width() / pic.base.width(), y * pic.gain.height() / pic.base.height());
        let base = crate::color::srgb_to_linear(pic.base.get_pixel(x, y)[1] as f32 / 255.0);
        let m = &pic.meaning;
        (base + m.base_offset[1]) * m.factor(pic.gain.get_pixel(gx, gy)[1], 1) - m.alternate_offset[1]
    }

    #[test]
    fn a_photo_made_from_two_renditions_reads_back_as_one() {
        let (sdr, hdr, w, h) = scene();
        let pic = Picture::from_renditions(sdr.clone(), &hdr, w, h);
        assert!((light(&pic, 80, 8) - 4.0).abs() < 0.1, "{}", light(&pic, 80, 8));
        let data = pic.encode(90, |d| d).unwrap();
        let src = Src::mem(data.clone());
        let p = container::parse(&src);
        assert_eq!(of(&src, &p), Some(Hdr::GainMap));
        assert!(p.previews.iter().all(|j| j.width == 96), "the gain map is no preview");
        let back = Picture::from_jpeg(&data, u32::MAX).unwrap().unwrap();
        assert_eq!(back.base.dimensions(), (96, 64));
        assert_close(&back.meaning, &pic.meaning);
        for (x, y, want) in [(80, 8, 4.0), (20, 50, crate::color::srgb_to_linear(128.0 / 255.0))] {
            assert!((light(&back, x, y) / want - 1.0).abs() < 0.1, "{x},{y}: {} for {want}", light(&back, x, y));
        }
        // An ordinary JPEG is no HDR photo.
        let plain = preview::encode_jpeg(&sdr, 90).unwrap();
        assert!(Picture::from_jpeg(&plain, u32::MAX).unwrap().is_none());
        assert_eq!(of(&Src::mem(plain.clone()), &container::parse(&Src::mem(plain))), None);
    }

    #[test]
    fn a_turned_and_cut_photo_keeps_its_gain_in_place() {
        let (sdr, hdr, w, h) = scene();
        let pic = Picture::from_renditions(sdr, &hdr, w, h).map(|img| crate::edit::turn(img, 1)).map(|img| crate::edit::cut(&img, [0.0, 0.5, 1.0, 0.5], 0.0));
        // The window, top right, is at the bottom right once turned a quarter; the cut keeps the bottom half.
        assert_eq!(pic.base.dimensions(), (64, 48));
        assert!((light(&pic, 56, 40) - 4.0).abs() < 0.1);
        assert!(light(&pic, 8, 8) < 0.5);
        let small = pic.shrink(32);
        assert_eq!(small.base.dimensions(), (32, 24));
        assert!((light(&small, 30, 22) - 4.0).abs() < 0.2);
    }

    #[test]
    fn what_a_watermark_covers_shows_as_it_is() {
        let (sdr, hdr, w, h) = scene();
        let mut pic = Picture::from_renditions(sdr, &hdr, w, h);
        let before = pic.base.clone();
        for x in 60..90 {
            for y in 4..12 {
                pic.base.put_pixel(x, y, Rgb([0, 0, 0]));
            }
        }
        pic.settle(&before);
        assert!(light(&pic, 70, 8) < 0.02, "the mark is not lit up: {}", light(&pic, 70, 8));
        assert!((light(&pic, 80, 24) - 4.0).abs() < 0.1, "the rest of the window still is");
    }

    #[test]
    fn stripping_a_gain_map_jpeg_keeps_its_hdr() {
        let (sdr, hdr, w, h) = scene();
        let data = Picture::from_renditions(sdr, &hdr, w, h).encode(90, |d| d).unwrap();
        let bare = crate::edit::stripped(&data, 1).unwrap();
        assert_eq!(of(&Src::mem(bare.clone()), &container::parse(&Src::mem(bare.clone()))), None);
        let out = kept(bare, &data);
        let plain = without_gain_map(out.clone());
        assert!(Picture::from_jpeg(&plain, u32::MAX).unwrap().is_none());
        assert_eq!(preview::decode_jpeg(&plain, u32::MAX).unwrap(), preview::decode_jpeg(&out, u32::MAX).unwrap());
        let back = Picture::from_jpeg(&out, u32::MAX).unwrap().unwrap();
        assert!((light(&back, 80, 8) - 4.0).abs() < 0.2);
    }

    #[cfg(target_vendor = "apple")]
    #[test]
    fn the_system_shows_a_gain_map_we_write_in_hdr() {
        let (sdr, hdr, w, h) = scene();
        let data = Picture::from_renditions(sdr, &hdr, w, h).encode(90, |d| d).unwrap();
        let path = std::env::temp_dir().join(format!("sift-gain-map-{}.jpg", std::process::id()));
        std::fs::write(&path, data).unwrap();
        let (light, lw, _) = crate::apple::light(&path, 96).unwrap();
        let _ = std::fs::remove_file(&path);
        let at = |x: usize, y: usize| light[(y * lw as usize + x) * 3 + 1];
        assert!((at(80, 8) / 4.0 - 1.0).abs() < 0.15, "window {}", at(80, 8));
        assert!(at(20, 50) < 0.3, "grey {}", at(20, 50));
    }

    #[test]
    fn heif_files_coded_for_hdr_screens_are_told_apart() {
        assert_eq!(kind(Path::new(HLG)), Some(Hdr::Hlg));
        assert_eq!(kind(Path::new(PQ)), Some(Hdr::Pq));
        assert_eq!(kind(Path::new(PLAIN)), None);
    }

    /// The light of the two halves of the HDR HEIF test pictures (grey, and
    /// 1000 cd/m², 4.9 times paper white), each decoder's way.
    #[cfg(any(target_vendor = "apple", feature = "heif"))]
    #[test]
    fn an_hdr_heif_becomes_a_picture_with_a_gain_map() {
        type Read = fn(&Path, u32) -> Result<Option<Picture>>;
        let ways: &[(&str, Read)] = &[
            #[cfg(target_vendor = "apple")]
            ("system", rendered),
            #[cfg(feature = "heif")]
            ("bundled", crate::heif::bundled_hdr),
        ];
        for &(way, read) in ways {
            for path in [HLG, PQ] {
                let pic = read(Path::new(path), 64).unwrap().unwrap();
                assert_eq!(pic.base.dimensions(), (64, 40), "{way} {path}");
                let (left, right) = (light(&pic, 8, 20), light(&pic, 56, 20));
                assert!((3.5..6.5).contains(&right), "{way} {path}: bright half {right}");
                assert!(left < 0.8, "{way} {path}: grey half {left}");
            }
        }
        #[cfg(feature = "heif")]
        assert!(crate::heif::bundled_hdr(Path::new(PLAIN), 64).unwrap().is_none());
    }
}
