//! Colour. Everything here works on sRGB values, the colours screens,
//! browsers and files that say nothing about theirs are taken to have. What a
//! picture says about its own (an ICC profile; for a camera's Adobe RGB often
//! a note in its EXIF alone; for HEIF the primaries and transfer function it
//! names) is applied when its values are read, so thumbnails, looks, the
//! histogram and exports all start from the colours the picture has.
//!
//! A JPEG handed over as it is keeps its profile, which browsers apply. They
//! do not read EXIF, so one that is Adobe RGB by its EXIF alone is given a
//! profile that says so ([`profiled`]).
// HDR comes from the bundled HEIF decoder alone.
#![cfg_attr(not(feature = "heif"), allow(dead_code))]

use crate::io::Src;
use crate::{exif, jpeg};
use image::RgbImage;

type Matrix = [[f32; 3]; 3];

const IDENTITY: Matrix = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];
/// Linear Display P3 to linear sRGB.
const FROM_P3: Matrix = [[1.2249401, -0.2249404, 0.0], [-0.0420569, 1.0420571, 0.0], [-0.0196376, -0.0786361, 1.0982735]];
/// Linear BT.2020 to linear sRGB.
const FROM_BT2020: Matrix = [[1.660491, -0.5876411, -0.0728499], [-0.1245505, 1.1328999, -0.0083494], [-0.0181508, -0.1005789, 1.1187297]];
/// CIE XYZ as ICC profiles give it (adapted to D50) to linear sRGB.
const FROM_XYZ: Matrix = [[3.133_856, -1.6168667, -0.4906146], [-0.9787684, 1.9161415, 0.033454], [0.0719453, -0.2289914, 1.4052427]];
/// The colorants of Adobe RGB (1998), CIE XYZ adapted to D50 as ICC profiles give them.
const ADOBE_RGB: [[f32; 3]; 3] = [[0.60974, 0.31111, 0.01947], [0.20528, 0.62567, 0.06087], [0.14919, 0.06322, 0.74457]];
/// Its gamma, 563/256, as ICC profiles write it.
const ADOBE_GAMMA: u16 = 0x0233;

/// How a file's values become light.
enum Transfer {
    /// An ordinary picture: the light each of the 256 values stands for, 0 to 1.
    Sdr(Box<[f32; 256]>),
    /// HDR as light on a screen (PQ, SMPTE ST 2084), as Canon writes it.
    Pq,
    /// HDR as light of the scene (HLG, ARIB STD-B67), as Nikon and Sony write it.
    Hlg,
}

pub struct Space {
    transfer: Transfer,
    /// From the file's linear RGB to linear sRGB.
    matrix: Matrix,
}

fn srgb_to_linear(v: f32) -> f32 {
    if v <= 0.04045 {
        v / 12.92
    } else {
        ((v + 0.055) / 1.055).powf(2.4)
    }
}

fn linear_to_srgb(v: f32) -> f32 {
    if v <= 0.0031308 {
        v * 12.92
    } else {
        1.055 * v.powf(1.0 / 2.4) - 0.055
    }
}

/// The sRGB value of linear light (0 to 1; anything outside is clipped).
fn encode(v: f32) -> u8 {
    const STEPS: usize = 1 << 14;
    static TABLE: std::sync::OnceLock<Vec<u8>> = std::sync::OnceLock::new();
    let table = TABLE.get_or_init(|| (0..=STEPS).map(|i| (linear_to_srgb(i as f32 / STEPS as f32) * 255.0).round() as u8).collect());
    // The cast sends NaN and negative values to 0.
    table[((v * STEPS as f32 + 0.5) as usize).min(STEPS)]
}

fn multiply(a: &Matrix, b: &Matrix) -> Matrix {
    std::array::from_fn(|i| std::array::from_fn(|j| (0..3).map(|k| a[i][k] * b[k][j]).sum()))
}

fn apply(m: &Matrix, v: [f32; 3]) -> [f32; 3] {
    std::array::from_fn(|i| m[i][0] * v[0] + m[i][1] * v[1] + m[i][2] * v[2])
}

fn be32(d: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_be_bytes(d.get(at..at + 4)?.try_into().ok()?))
}

/// An ICC `s15Fixed16Number`.
fn fixed(d: &[u8], at: usize) -> Option<f32> {
    Some(be32(d, at)? as i32 as f32 / 65536.0)
}

/// A tone curve of an ICC profile (`curv` or `para`) as the light of each 8-bit value.
fn icc_curve(tag: &[u8]) -> Option<[f32; 256]> {
    let at = |i: usize| i as f32 / 255.0;
    match tag.get(..4)? {
        b"curv" => {
            let n = be32(tag, 8)? as usize;
            let entry = |i: usize| Some(u16::from_be_bytes(tag.get(12 + i * 2..14 + i * 2)?.try_into().ok()?) as f32);
            match n {
                0 => Some(std::array::from_fn(at)),
                1 => {
                    let gamma = entry(0)? / 256.0;
                    Some(std::array::from_fn(|i| at(i).powf(gamma)))
                }
                _ => {
                    entry(n - 1)?;
                    Some(std::array::from_fn(|i| {
                        let x = at(i) * (n - 1) as f32;
                        let low = (x as usize).min(n - 2);
                        let (a, b) = (entry(low).unwrap_or(0.0), entry(low + 1).unwrap_or(0.0));
                        (a + (b - a) * (x - low as f32)) / 65535.0
                    }))
                }
            }
        }
        b"para" => {
            let kind = u16::from_be_bytes(tag.get(8..10)?.try_into().ok()?) as usize;
            let count = *[1, 3, 4, 5, 7].get(kind)?;
            let mut v = [0f32; 7];
            for (i, slot) in v.iter_mut().take(count).enumerate() {
                *slot = fixed(tag, 12 + i * 4)?;
            }
            let [g, a, b, c, d, e, f] = v;
            Some(std::array::from_fn(|i| {
                let x = at(i);
                let y = match kind {
                    0 => x.powf(g),
                    1 if x >= -b / a => (a * x + b).powf(g),
                    1 => 0.0,
                    2 if x >= -b / a => (a * x + b).powf(g) + c,
                    2 => c,
                    3 if x >= d => (a * x + b).powf(g),
                    3 => c * x,
                    _ if x >= d => (a * x + b).powf(g) + e,
                    _ => c * x + f,
                };
                y.clamp(0.0, 1.0)
            }))
        }
        _ => None,
    }
}

/// What a display profile says (three colorants and a tone curve, the
/// kind cameras and phones embed): the curve and the way to linear sRGB.
fn icc_space(profile: &[u8]) -> Option<(Box<[f32; 256]>, Matrix)> {
    let tag = |name: &[u8; 4]| {
        (0..be32(profile, 128)?.min(256) as usize).find_map(|i| {
            let entry = 132 + i * 12;
            let (offset, size) = (be32(profile, entry + 4)? as usize, be32(profile, entry + 8)? as usize);
            (profile.get(entry..entry + 4)? == name).then(|| profile.get(offset..offset.checked_add(size)?)).flatten()
        })
    };
    let colorant = |name: &[u8; 4]| {
        let t = tag(name)?;
        (t.get(..4)? == b"XYZ ").then_some(())?;
        Some([fixed(t, 8)?, fixed(t, 12)?, fixed(t, 16)?])
    };
    let (r, g, b) = (colorant(b"rXYZ")?, colorant(b"gXYZ")?, colorant(b"bXYZ")?);
    let to_xyz: Matrix = std::array::from_fn(|i| [r[i], g[i], b[i]]);
    Some((Box::new(icc_curve(tag(b"rTRC")?)?), multiply(&FROM_XYZ, &to_xyz)))
}

impl Space {
    /// From what the file names (primaries and transfer function as
    /// numbered in ITU-T H.273, 2 for neither) and the ICC profile it carries.
    pub fn of(primaries: u16, transfer: u16, profile: Option<&[u8]>) -> Space {
        let matrix = match primaries {
            9 => FROM_BT2020,
            12 => FROM_P3,
            _ => IDENTITY,
        };
        match transfer {
            16 => return Space { transfer: Transfer::Pq, matrix },
            18 => return Space { transfer: Transfer::Hlg, matrix },
            _ => {}
        }
        // A profile says more than the numbers: phones name no primaries and embed Display P3.
        if let Some((curve, matrix)) = profile.and_then(icc_space) {
            return Space { transfer: Transfer::Sdr(curve), matrix };
        }
        Space { transfer: Transfer::Sdr(Box::new(std::array::from_fn(|i| srgb_to_linear(i as f32 / 255.0)))), matrix }
    }

    /// What an ICC display profile says (three colorants and a tone curve,
    /// the kind cameras, phones and editors embed). `None` for one this
    /// cannot read (made of lookup tables); its picture is taken as sRGB.
    pub fn from_icc(profile: &[u8]) -> Option<Space> {
        icc_space(profile).map(|(curve, matrix)| Space { transfer: Transfer::Sdr(curve), matrix })
    }

    pub fn adobe_rgb() -> Space {
        Space::from_icc(&adobe_rgb_profile()).expect("the profile written here is read back")
    }

    /// What the values of a JPEG stand for: what its ICC profile says, else
    /// Adobe RGB when its EXIF block (TIFF header first, as decoders hand it
    /// out) says so, or `adobe_rgb`, what the file it is embedded in says:
    /// cameras put no EXIF in most of the previews they embed. `None` for sRGB.
    pub fn of_jpeg(profile: Option<&[u8]>, exif_block: Option<&[u8]>, adobe_rgb: bool) -> Option<Space> {
        match profile {
            Some(profile) => Space::from_icc(profile),
            None if adobe_rgb || exif_block.is_some_and(|b| exif::adobe_rgb_at(&Src::mem(b.to_vec()), 0)) => Some(Space::adobe_rgb()),
            None => None,
        }
    }

    pub fn is_hdr(&self) -> bool {
        !matches!(self.transfer, Transfer::Sdr(_))
    }

    /// Whether the values are sRGB already, near enough that converting would only add rounding.
    fn is_srgb(&self) -> bool {
        let Transfer::Sdr(curve) = &self.transfer else { return false };
        let same_curve = curve.iter().enumerate().all(|(i, v)| (v - srgb_to_linear(i as f32 / 255.0)).abs() < 0.004);
        same_curve && self.matrix.iter().flatten().zip(IDENTITY.iter().flatten()).all(|(a, b)| (a - b).abs() < 0.004)
    }

    /// Turns an ordinary picture's values into sRGB in place.
    pub fn to_srgb(&self, img: &mut RgbImage) {
        let Transfer::Sdr(curve) = &self.transfer else { return };
        if self.is_srgb() {
            return;
        }
        let convert = |part: &mut [u8]| {
            for p in part.as_chunks_mut::<3>().0 {
                *p = apply(&self.matrix, p.map(|v| curve[v as usize])).map(encode);
            }
        };
        // A full-size photo is converted on every core; a thumbnail is not worth a thread.
        let threads = std::thread::available_parallelism().map_or(1, |n| n.get());
        let data: &mut [u8] = img;
        let chunk = (data.len() / 3).div_ceil(threads).max(1 << 18) * 3;
        std::thread::scope(|s| {
            let mut parts = data.chunks_mut(chunk);
            let first = parts.next();
            for part in parts {
                s.spawn(move || convert(part));
            }
            first.map(convert);
        });
    }

    /// Brings HDR values (16 bits a channel) into sRGB as macOS does when
    /// it draws such a picture for an ordinary screen, so a picture looks
    /// the same everywhere: each channel goes through [`shown`], then
    /// the colours are brought to sRGB and what lies outside is clipped.
    pub fn tone_map(&self, deep: &[u16]) -> Vec<u8> {
        const STEPS: usize = 4095;
        let hlg = matches!(self.transfer, Transfer::Hlg);
        // What a coded value stands for: with PQ the light on a screen, so it is
        // brought into range right here; with HLG the light of the scene, 0 to 1.
        let coded: Vec<f32> = (0..=STEPS)
            .map(|i| {
                let v = i as f32 / STEPS as f32;
                if hlg {
                    let (a, b, c) = (0.178_832_77, 0.284_668_92, 0.559_910_7);
                    if v <= 0.5 {
                        v * v / 3.0
                    } else {
                        (((v - c) / a).exp() + b) / 12.0
                    }
                } else {
                    let (m1, m2, c1, c2, c3) = (0.159_301_76, 78.84375, 0.835_937_5, 18.851_563, 18.6875);
                    let root = v.powf(1.0 / m2);
                    shown(10000.0 * ((root - c1).max(0.0) / (c2 - c3 * root)).powf(1.0 / m1))
                }
            })
            .collect();
        // Functions of a value from 0 to 1, tabulated by its fourth root to be fine near black.
        let table = |f: &dyn Fn(f32) -> f32| -> Vec<f32> { (0..=STEPS).map(|i| f((i as f32 / STEPS as f32).powi(4))).collect() };
        let look_up = |table: &[f32], v: f32| {
            let x = v.clamp(0.0, 1.0).sqrt().sqrt() * STEPS as f32;
            let i = (x as usize).min(STEPS - 1);
            table[i] + (table[i + 1] - table[i]) * (x - i as f32)
        };
        // HLG as a 1000 cd/m² screen shows it (ITU-R BT.2100): the brighter
        // a part of the scene, the more light it gets (system gamma 1.2).
        let (gain, screen) = if hlg { (table(&|scene| scene.powf(0.2)), table(&|light| shown(light * 1000.0))) } else { (Vec::new(), Vec::new()) };
        let mut out = Vec::with_capacity(deep.len());
        for p in deep.as_chunks::<3>().0 {
            let mut rgb = p.map(|v| coded[(v as usize * STEPS + 32767) / 65535]);
            if hlg {
                let gain = look_up(&gain, 0.2627 * rgb[0] + 0.678 * rgb[1] + 0.0593 * rgb[2]);
                rgb = rgb.map(|v| look_up(&screen, v * gain));
            }
            out.extend(apply(&self.matrix, rgb).map(encode));
        }
        out
    }
}

/// A display profile (ICC version 2) of three colorants (CIE XYZ adapted to
/// D50) and one gamma for every channel (`u8Fixed8Number`).
fn display_profile(description: &str, colorants: [[f32; 3]; 3], gamma: u16) -> Vec<u8> {
    let fixed = |v: f32| ((v * 65536.0).round() as i32).to_be_bytes();
    let xyz = |v: [f32; 3]| {
        let mut t = b"XYZ \0\0\0\0".to_vec();
        v.iter().for_each(|x| t.extend(fixed(*x)));
        t
    };
    let mut desc = b"desc\0\0\0\0".to_vec();
    desc.extend((description.len() as u32 + 1).to_be_bytes());
    desc.extend(description.as_bytes());
    // Its NUL, then no Unicode description (language, count) and no ScriptCode one (code, count, 67 bytes).
    desc.extend([0u8; 1 + 8 + 3 + 67]);
    let mut curve = b"curv\0\0\0\0\0\0\0\x01".to_vec();
    curve.extend(gamma.to_be_bytes());
    let tags: [(&[u8; 4], Vec<u8>); 7] = [
        (b"desc", desc),
        (b"cprt", b"text\0\0\0\0No copyright, use freely\0".to_vec()),
        // Version 2 display profiles give the screen's own white here: D65.
        (b"wtpt", xyz([0.95045, 1.0, 1.08905])),
        (b"rXYZ", xyz(colorants[0])),
        (b"gXYZ", xyz(colorants[1])),
        (b"bXYZ", xyz(colorants[2])),
        (b"rTRC", curve),
    ];
    // gTRC and bTRC point at the red curve.
    let count = tags.len() + 2;
    let start = 128 + 4 + count * 12;
    let (mut table, mut body) = (Vec::new(), Vec::new());
    for (name, data) in &tags {
        let at = start + body.len();
        let names: &[&[u8; 4]] = if *name == b"rTRC" { &[b"rTRC", b"gTRC", b"bTRC"] } else { &[name] };
        for name in names {
            table.extend(*name);
            table.extend((at as u32).to_be_bytes());
            table.extend((data.len() as u32).to_be_bytes());
        }
        body.extend(data);
        body.resize(body.len().next_multiple_of(4), 0);
    }
    let size = start + body.len();
    let mut out = Vec::with_capacity(size);
    out.extend((size as u32).to_be_bytes());
    // No preferred CMM, version 2.1, a display's RGB to XYZ, no date.
    out.extend([0, 0, 0, 0, 2, 0x10, 0, 0]);
    out.extend(b"mntrRGB XYZ ");
    out.extend([0; 12]);
    out.extend(b"acsp");
    // Platform, flags, maker, model, attributes (8 bytes), rendering intent (perceptual).
    out.extend([0; 28]);
    // The illuminant of the connection space, D50, as the ICC spells it.
    out.extend([0, 0, 0xF6, 0xD6, 0, 1, 0, 0, 0, 0, 0xD3, 0x2D]);
    // Creator, profile ID (16 bytes), reserved (28).
    out.extend([0; 48]);
    out.extend((count as u32).to_be_bytes());
    out.extend(table);
    out.extend(body);
    out
}

/// An ICC profile of Adobe RGB (1998).
pub fn adobe_rgb_profile() -> Vec<u8> {
    display_profile("Adobe RGB (1998) compatible", ADOBE_RGB, ADOBE_GAMMA)
}

/// `data` (a JPEG) with an Adobe RGB profile put in when it has none and
/// says it is Adobe RGB in its EXIF alone, or `adobe_rgb` holds (what the
/// file it comes from says); otherwise as it is.
pub fn profiled(data: Vec<u8>, adobe_rgb: bool) -> Vec<u8> {
    let src = Src::mem(data);
    let at = jpeg::scan(&src, 0)
        .filter(|info| !info.icc && (adobe_rgb || info.exif_base.is_some_and(|base| exif::adobe_rgb_at(&src, base))))
        // After the JFIF and EXIF headers, which come first.
        .map(|info| info.segments.iter().take_while(|s| matches!(s.marker, 0xE0 | 0xE1)).last().map_or(2, |s| (s.pos + s.len as u64) as usize));
    let Src::Mem(mut bytes) = src else { unreachable!() };
    if let Some(at) = at {
        bytes.splice(at..at, jpeg::icc_app2(&adobe_rgb_profile()));
    }
    bytes
}

/// The light (0 to 1) an ordinary screen gives for `nits` cd/m² of an HDR
/// picture. The curve is the one macOS 26 draws HDR pictures into sRGB
/// with, measured on a grey ramp and matched to within one level of 255:
/// a white sheet of paper (203 cd/m², ITU-R BT.2408) gets half the light
/// of white, which leaves room for what is brighter; that rolls off and
/// reaches white at 1000 cd/m².
fn shown(nits: f32) -> f32 {
    let paper = nits / 203.0;
    let plain = 0.5 * paper.powf(0.9234);
    if paper <= 1.0 {
        return plain;
    }
    let t = paper.ln();
    plain.min(0.50777 + t * (0.38754 + t * (-0.07852 + t * 0.01859))).min(1.0)
}
#[cfg(test)]
mod tests {
    use super::*;

    /// A display profile with the given colorants and tone curve tag.
    fn profile(colorants: [[f32; 3]; 3], curve: &[u8]) -> Vec<u8> {
        let xyz = |v: [f32; 3]| {
            let mut t = b"XYZ \0\0\0\0".to_vec();
            v.iter().for_each(|x| t.extend(((x * 65536.0).round() as i32).to_be_bytes()));
            t
        };
        let tags: Vec<(&[u8; 4], Vec<u8>)> = vec![(b"rXYZ", xyz(colorants[0])), (b"gXYZ", xyz(colorants[1])), (b"bXYZ", xyz(colorants[2])), (b"rTRC", curve.to_vec())];
        let mut out = vec![0u8; 128];
        out.extend((tags.len() as u32).to_be_bytes());
        let mut at = 132 + tags.len() * 12;
        for (name, body) in &tags {
            out.extend(*name);
            out.extend((at as u32).to_be_bytes());
            out.extend((body.len() as u32).to_be_bytes());
            at += body.len();
        }
        tags.iter().for_each(|(_, body)| out.extend(body));
        out
    }

    /// The sRGB curve as ICC profiles write it (`para`, kind 3).
    fn srgb_curve() -> Vec<u8> {
        let mut t = b"para\0\0\0\0\0\x03\0\0".to_vec();
        [2.4, 1.0 / 1.055, 0.055 / 1.055, 1.0 / 12.92, 0.04045].iter().for_each(|v: &f32| t.extend(((v * 65536.0).round() as i32).to_be_bytes()));
        t
    }

    const SRGB: [[f32; 3]; 3] = [[0.4360747, 0.2225045, 0.0139322], [0.3850649, 0.7168786, 0.0971045], [0.1430804, 0.0606169, 0.7141733]];
    const DISPLAY_P3: [[f32; 3]; 3] = [[0.51512, 0.2412, -0.00105], [0.29198, 0.69225, 0.04189], [0.1571, 0.06657, 0.78407]];

    #[test]
    fn a_profile_of_srgb_changes_nothing() {
        let space = Space::of(2, 2, Some(&profile(SRGB, &srgb_curve())));
        assert!(space.is_srgb());
        assert!(Space::of(1, 13, None).is_srgb());
        assert!(Space::of(2, 2, Some(b"not a profile")).is_srgb());
    }

    #[test]
    fn display_p3_is_brought_to_srgb_by_profile_and_by_name() {
        let by_profile = Space::of(2, 2, Some(&profile(DISPLAY_P3, &srgb_curve())));
        let by_name = Space::of(12, 13, None);
        assert!(!by_profile.is_srgb() && !by_name.is_srgb());
        for (a, b) in by_profile.matrix.iter().flatten().zip(by_name.matrix.iter().flatten()) {
            assert!((a - b).abs() < 0.002, "{:?} {:?}", by_profile.matrix, by_name.matrix);
        }
        // Grey stays grey, P3's red is beyond sRGB's and clips, a skin tone gets more saturated numbers.
        let mut img = RgbImage::from_raw(3, 1, vec![128, 128, 128, 255, 0, 0, 200, 150, 120]).unwrap();
        by_profile.to_srgb(&mut img);
        assert_eq!(img.as_raw()[..6], [128, 128, 128, 255, 0, 0]);
        let skin = &img.as_raw()[6..];
        assert!(skin[0] > 200 && skin[1] < 150 && skin[2] < 120, "{skin:?}");
    }

    /// Swatches of Adobe RGB values and what ColorSync makes of them in sRGB
    /// with Apple's own Adobe RGB (1998) profile (`sips -m`).
    const SWATCHES: [[u8; 3]; 8] = [[255, 0, 0], [0, 255, 0], [0, 0, 255], [128, 128, 128], [200, 150, 120], [30, 90, 160], [250, 240, 20], [10, 10, 10]];
    const COLORSYNC: [[u8; 3]; 8] = [[255, 0, 0], [0, 255, 0], [0, 0, 255], [129, 129, 129], [217, 151, 119], [0, 90, 164], [254, 240, 0], [2, 2, 2]];

    fn near(got: &[[u8; 3]], want: &[[u8; 3]], by: u8) -> bool {
        got.iter().zip(want).all(|(a, b)| a.iter().zip(b).all(|(x, y)| x.abs_diff(*y) <= by))
    }

    #[test]
    fn adobe_rgb_is_brought_to_srgb_as_colorsync_does() {
        let mut img = RgbImage::from_fn(8, 1, |x, _| image::Rgb(SWATCHES[x as usize]));
        Space::adobe_rgb().to_srgb(&mut img);
        let got: Vec<[u8; 3]> = img.pixels().map(|p| p.0).collect();
        assert!(near(&got, &COLORSYNC, 1), "{got:?}");
        // A picture too big for one thread comes out the same.
        let mut big = RgbImage::from_fn(1024, 1024, |x, y| image::Rgb(SWATCHES[((x + y) % 8) as usize]));
        Space::adobe_rgb().to_srgb(&mut big);
        assert!(big.enumerate_pixels().all(|(x, y, p)| p.0 == got[((x + y) % 8) as usize]));
    }

    #[test]
    fn the_adobe_rgb_profile_is_well_formed() {
        let icc = adobe_rgb_profile();
        assert_eq!(be32(&icc, 0), Some(icc.len() as u32));
        assert_eq!(&icc[36..40], b"acsp");
        let count = be32(&icc, 128).unwrap() as usize;
        assert_eq!(count, 9);
        for i in 0..count {
            let (at, size) = (be32(&icc, 136 + i * 12).unwrap() as usize, be32(&icc, 140 + i * 12).unwrap() as usize);
            assert!(at % 4 == 0 && at + size <= icc.len(), "tag {i}");
        }
        let space = Space::from_icc(&icc).unwrap();
        let to_xyz: Matrix = std::array::from_fn(|i| [ADOBE_RGB[0][i], ADOBE_RGB[1][i], ADOBE_RGB[2][i]]);
        let want = multiply(&FROM_XYZ, &to_xyz);
        assert!(space.matrix.iter().flatten().zip(want.iter().flatten()).all(|(a, b)| (a - b).abs() < 1e-4));
    }

    /// An EXIF block (TIFF header first) with a ColorSpace and, given one,
    /// an interoperability index.
    fn exif_block(space: u16, index: Option<&[u8; 3]>) -> Vec<u8> {
        let entry = |tag: u16, typ: u16, count: u32, value: [u8; 4]| [&tag.to_le_bytes()[..], &typ.to_le_bytes(), &count.to_le_bytes(), &value].concat();
        let mut b = b"II*\0\x08\0\0\0".to_vec();
        // IFD0 holds where the Exif IFD is: right after it, at 26; that one
        // where the interoperability IFD is: after it, at 56.
        b.extend([&1u16.to_le_bytes()[..], &entry(0x8769, 4, 1, 26u32.to_le_bytes()), &[0; 4]].concat());
        b.extend([&2u16.to_le_bytes()[..], &entry(0xA001, 3, 1, (space as u32).to_le_bytes()), &entry(0xA005, 4, 1, 56u32.to_le_bytes()), &[0; 4]].concat());
        if let Some(&[a, b2, c]) = index {
            b.extend([&1u16.to_le_bytes()[..], &entry(0x0001, 2, 4, [a, b2, c, 0]), &[0; 4]].concat());
        }
        b
    }

    /// `jpeg` with an EXIF block, after its JFIF header.
    fn with_exif(jpeg: &[u8], block: &[u8]) -> Vec<u8> {
        let at = 4 + u16::from_be_bytes([jpeg[4], jpeg[5]]) as usize;
        assert_eq!(jpeg[2..4], [0xFF, 0xE0]);
        let mut segment = vec![0xFF, 0xE1];
        segment.extend(((2 + 6 + block.len()) as u16).to_be_bytes());
        segment.extend(b"Exif\0\0");
        segment.extend(block);
        [&jpeg[..at], &segment, &jpeg[at..]].concat()
    }

    /// The swatches as a JPEG, each a 16x16 square so they come back as they went in.
    fn swatch_jpeg() -> Vec<u8> {
        crate::preview::encode_jpeg(&RgbImage::from_fn(128, 16, |x, _| image::Rgb(SWATCHES[x as usize / 16])), 100).unwrap()
    }

    fn swatches_of(img: &RgbImage) -> Vec<[u8; 3]> {
        (0..8).map(|i| img.get_pixel(i * 16 + 8, 8).0).collect()
    }

    #[test]
    fn a_jpeg_is_read_as_its_profile_its_exif_or_its_file_says() {
        use crate::preview::{decode_embedded, decode_jpeg};
        let plain = swatch_jpeg();
        let read = |data: &[u8]| swatches_of(&decode_jpeg(data, u32::MAX).unwrap());
        assert!(near(&read(&plain), &SWATCHES, 1));
        // sRGB by EXIF, also "uncalibrated" with DCF's sRGB index.
        for srgb in [exif_block(1, None), exif_block(1, Some(b"R98")), exif_block(0xFFFF, Some(b"R98"))] {
            assert!(near(&read(&with_exif(&plain, &srgb)), &SWATCHES, 1));
        }
        // Adobe RGB by EXIF, the file it is in, or a profile.
        let by_exif = [exif_block(0xFFFF, Some(b"R03")), exif_block(0xFFFF, None), exif_block(2, None)].map(|b| with_exif(&plain, &b));
        for tagged in by_exif.into_iter().chain([profiled(plain.clone(), true)]) {
            assert!(near(&read(&tagged), &COLORSYNC, 2), "{:?}", read(&tagged));
        }
        assert!(near(&swatches_of(&decode_embedded(&plain, u32::MAX, true).unwrap()), &COLORSYNC, 2));
        // A profile outranks EXIF and the file.
        let srgb = jpeg::icc_app2(&profile(SRGB, &srgb_curve()));
        let mut both = with_exif(&plain, &exif_block(0xFFFF, Some(b"R03")));
        both.splice(2..2, srgb);
        assert!(near(&swatches_of(&decode_embedded(&both, u32::MAX, true).unwrap()), &SWATCHES, 1));
    }

    #[test]
    fn a_profile_is_put_in_only_where_exif_alone_says_adobe_rgb() {
        let plain = swatch_jpeg();
        assert_eq!(profiled(plain.clone(), false), plain);
        let srgb = with_exif(&plain, &exif_block(0xFFFF, Some(b"R98")));
        assert_eq!(profiled(srgb.clone(), false), srgb);
        let tagged = with_exif(&plain, &exif_block(0xFFFF, Some(b"R03")));
        let given = profiled(tagged.clone(), false);
        let info = jpeg::scan(&Src::mem(given.clone()), 0).unwrap();
        assert!(info.icc);
        // After the JFIF and EXIF headers, the picture untouched.
        assert_eq!(info.segments.iter().map(|s| s.marker).take(3).collect::<Vec<_>>(), [0xE0, 0xE1, 0xE2]);
        let added = given.len() - tagged.len();
        let exif_end = (info.segments[1].pos + info.segments[1].len as u64) as usize;
        assert_eq!([&given[..exif_end], &given[exif_end + added..]].concat(), tagged);
        // Once is enough.
        assert_eq!(profiled(given.clone(), true), given);
        // Leaving EXIF out of a copy keeps what it said.
        let stripped = crate::edit::stripped(&tagged, 1).unwrap();
        assert!(jpeg::scan(&Src::mem(stripped.clone()), 0).unwrap().icc);
        let read = |data: &[u8]| crate::preview::decode_jpeg(data, u32::MAX).unwrap();
        assert_eq!(read(&stripped), read(&tagged));
    }

    /// The system's decoder reads the profile written here and turns its
    /// colours into sRGB as this module does. (It takes no notice of DCF's
    /// index, only of the primaries Panasonic also writes into EXIF.)
    #[cfg(target_vendor = "apple")]
    #[test]
    fn the_system_reads_these_jpegs_alike() {
        let dir = std::env::temp_dir().join(format!("sift-color-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let plain = swatch_jpeg();
        for (name, data) in [("profile", profiled(plain.clone(), true)), ("plain", plain.clone())] {
            let path = dir.join(format!("{name}.jpg"));
            std::fs::write(&path, &data).unwrap();
            let system = swatches_of(&crate::apple::pixels(&path, u32::MAX).unwrap());
            let ours = swatches_of(&crate::preview::decode_jpeg(&data, u32::MAX).unwrap());
            assert!(near(&ours, &system, 1), "{name}: {ours:?} vs {system:?}");
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn tone_curves_of_profiles_are_read() {
        let gamma = icc_curve(b"curv\0\0\0\0\0\0\0\x01\x02\x33").unwrap();
        assert!((gamma[128] - (128.0f32 / 255.0).powf(2.2)).abs() < 0.001);
        let table = icc_curve(b"curv\0\0\0\0\0\0\0\x03\0\0\x40\0\xFF\xFF").unwrap();
        assert!((table[0], table[255]) == (0.0, 1.0) && (table[64] - 0.1255).abs() < 0.001, "{}", table[64]);
        let srgb = icc_curve(&srgb_curve()).unwrap();
        assert!((0..256).all(|i| (srgb[i] - srgb_to_linear(i as f32 / 255.0)).abs() < 0.001));
        assert!(icc_curve(b"curv\0\0\0\0\0\0\0\x09\0\0").is_none());
    }

    #[test]
    fn hdr_is_brought_into_range_as_macos_does() {
        let (pq, hlg) = (Space::of(9, 16, None), Space::of(9, 18, None));
        assert!(pq.is_hdr() && hlg.is_hdr());
        let code = |v: f32| (v * 65535.0).round() as u16;
        let near = |got: &[u8], want: [u8; 12]| got.iter().zip(want).all(|(a, b)| a.abs_diff(b) <= 2);
        // Greys and pure colours of ramps as macOS 26 draws them into sRGB.
        for (v, grey, red, green, blue) in [(0.25, 35, 47, 38, 38), (0.5, 135, 172, 143, 144), (0.594, 197, 248, 208, 207), (0.688, 236, 255, 249, 249)] {
            let v = code(v);
            let got = pq.tone_map(&[v, v, v, v, 0, 0, 0, v, 0, 0, 0, v]);
            assert!(near(&got, [grey, grey, grey, red, 0, 0, 0, green, 0, 0, 0, blue]), "PQ {v}: {got:?}");
        }
        for (v, grey, red, green, blue) in [(0.25, 48, 56, 50, 39), (0.5, 104, 118, 107, 86), (0.75, 188, 212, 193, 158), (0.876, 228, 255, 237, 213)] {
            let v = code(v);
            let got = hlg.tone_map(&[v, v, v, v, 0, 0, 0, v, 0, 0, 0, v]);
            assert!(near(&got, [grey, grey, grey, red, 0, 0, 0, green, 0, 0, 0, blue]), "HLG {v}: {got:?}");
        }
        for space in [&pq, &hlg] {
            let levels: Vec<u8> = (0..=64).map(|i| space.tone_map(&[i * 1023; 3])[0]).collect();
            assert!(levels[0] == 0 && levels[64] == 255 && levels.windows(2).all(|w| w[0] <= w[1]), "{levels:?}");
        }
    }
}
