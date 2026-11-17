//! HEIF pictures (`.heic` from phones, `.hif` from cameras) as upright sRGB
//! pixels, the form everything else here works on.
//!
//! Apple platforms decode them with the system (`apple::pixels`), which
//! manages colour and brings HDR into range on its own. Elsewhere, and for a
//! file the system gives up on, the bundled decoder does it (feature `heif`).
//! That one hands out the values as coded, so what the file says about its
//! colours (an ICC profile, or the primaries and transfer function it names)
//! is applied here (`color`).

use crate::{Error, Result};
use image::RgbImage;
use std::path::Path;

/// Upright sRGB pixels of the HEIF picture at `path`, with a long edge of at
/// most `edge`.
pub fn decode(path: &Path, edge: u32) -> Result<RgbImage> {
    #[cfg(target_vendor = "apple")]
    let system = crate::apple::pixels(path, edge);
    #[cfg(not(target_vendor = "apple"))]
    let system: Result<RgbImage> = Err(Error::Unsupported("HEIF pictures cannot be decoded in this build"));
    #[cfg(feature = "heif")]
    if system.is_err() {
        return bundled(path, edge);
    }
    system
}

/// [`decode`] by the bundled decoder alone.
#[cfg(feature = "heif")]
pub fn bundled(path: &Path, edge: u32) -> Result<RgbImage> {
    use heic_decoder::{ColorInfo, RgbOptions};
    use image::imageops;
    let failed = |e: heic_decoder::HeifError| Error::Decode(e.to_string());
    let picture = heic_decoder::decode(&std::fs::read(path)?).map_err(failed)?;
    // A file that does not name its YCbCr matrix is read as BT.601, as the
    // system's decoder on Apple platforms reads it (iPhones named none up to iOS 11).
    let coded = picture.color_info.map(|c| if c.matrix_coefficients == 2 { ColorInfo { matrix_coefficients: 6, ..c } } else { c });
    let options = RgbOptions { color: coded, ..RgbOptions::default() };
    let (primaries, transfer) = coded.map_or((2, 2), |c| (c.color_primaries, c.transfer_characteristics));
    let space = color::Space::of(primaries, transfer, picture.metadata.icc_profile.as_deref());
    let (w, h) = (picture.width as u32, picture.height as u32);
    let long = w.max(h).max(1) as u64;
    let scaled = |side: u32| if long > edge as u64 { ((side as u64 * edge as u64 + long / 2) / long).max(1) as u32 } else { side };
    let (fw, fh) = (scaled(w), scaled(h));
    if !space.is_hdr() {
        let rgb = picture.to_rgb8_with_options(&options).map_err(failed)?;
        drop(picture);
        let mut img = RgbImage::from_raw(w, h, rgb.data).ok_or(Error::Malformed)?;
        if (fw, fh) != (w, h) {
            img = imageops::thumbnail(&img, fw, fh);
        }
        space.to_srgb(&mut img);
        return Ok(img);
    }
    // HDR keeps its depth until it is brought into the range of a screen.
    let rgb = picture.to_rgb16_with_options(&options).map_err(failed)?;
    drop(picture);
    let mut deep = image::ImageBuffer::<image::Rgb<u16>, Vec<u16>>::from_raw(w, h, rgb.data).ok_or(Error::Malformed)?;
    if (fw, fh) != (w, h) {
        deep = imageops::thumbnail(&deep, fw, fh);
    }
    RgbImage::from_raw(fw, fh, space.tone_map(deep.as_raw())).ok_or(Error::Malformed)
}

/// From the colours a file's values stand for to sRGB.
#[cfg(feature = "heif")]
mod color {
    use image::RgbImage;

    type Matrix = [[f32; 3]; 3];

    const IDENTITY: Matrix = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];
    /// Linear Display P3 to linear sRGB.
    const FROM_P3: Matrix = [[1.2249401, -0.2249404, 0.0], [-0.0420569, 1.0420571, 0.0], [-0.0196376, -0.0786361, 1.0982735]];
    /// Linear BT.2020 to linear sRGB.
    const FROM_BT2020: Matrix = [[1.660491, -0.5876411, -0.0728499], [-0.1245505, 1.1328999, -0.0083494], [-0.0181508, -0.1005789, 1.1187297]];
    /// CIE XYZ as ICC profiles give it (adapted to D50) to linear sRGB.
    const FROM_XYZ: Matrix = [[3.133_856, -1.6168667, -0.4906146], [-0.9787684, 1.9161415, 0.033454], [0.0719453, -0.2289914, 1.4052427]];

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
            for p in img.pixels_mut() {
                p.0 = apply(&self.matrix, p.0.map(|v| curve[v as usize])).map(encode);
            }
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
}

#[cfg(all(test, feature = "heif"))]
mod tests {
    use super::*;

    /// Four plain quarters, stored 64x40 and lying on its side, with the
    /// instruction to show it turned a quarter (drawn for this test; x265, heif-enc).
    const TURNED: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/assets/turned.heic");

    fn quarters(img: &RgbImage) -> [[bool; 3]; 4] {
        let (w, h) = img.dimensions();
        [(w / 4, h / 4), (3 * w / 4, h / 4), (w / 4, 3 * h / 4), (3 * w / 4, 3 * h / 4)].map(|(x, y)| img.get_pixel(x, y).0.map(|v| v > 128))
    }

    const UPRIGHT: [[bool; 3]; 4] = [[false, true, false], [true, true, false], [true, false, false], [false, false, true]];

    #[test]
    fn the_bundled_decoder_gives_upright_pixels() {
        let img = bundled(Path::new(TURNED), u32::MAX).unwrap();
        assert_eq!(img.dimensions(), (40, 64));
        assert_eq!(quarters(&img), UPRIGHT);
        let small = bundled(Path::new(TURNED), 32).unwrap();
        assert_eq!(small.dimensions(), (20, 32));
        assert_eq!(quarters(&small), UPRIGHT);
    }

    #[test]
    fn whichever_decoder_is_used_agrees_with_the_index() {
        let img = decode(Path::new(TURNED), u32::MAX).unwrap();
        assert_eq!(img.dimensions(), (40, 64));
        assert_eq!(quarters(&img), UPRIGHT);
        let src = crate::io::Src::open(Path::new(TURNED)).unwrap();
        let index = crate::container::parse(&src).heif.unwrap();
        assert_eq!((index.width, index.height, index.orientation), (64, 40, 8));
    }
}
