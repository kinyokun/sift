//! The camera's electronic level at the moment of the shot, from the maker
//! note: how far the camera was rolled (degrees clockwise, as the
//! photographer sees it from behind) and pitched (degrees up). Tag layouts
//! follow ExifTool's tables (Nikon OrientationInfo in the encrypted ShotInfo,
//! Canon LevelInfo, FujiFilm RollAngle, Panasonic RollAngle/PitchAngle, also
//! in Leica's cameras made by Panasonic, Olympus CameraSettings, Pentax
//! LevelInfo, Apple AccelerationVector).

use crate::container::{self, MakerNote, Parsed};
use crate::io::{u16_at, u32_at};
use serde::Serialize;

#[derive(Serialize, Clone, Copy, Debug, PartialEq)]
pub struct Level {
    /// Degrees the camera was turned clockwise about the lens axis. A body
    /// held upright for a portrait frame may say about 90 or -90 here.
    pub roll: f32,
    /// Degrees the lens pointed above the horizon, where the camera says.
    pub pitch: Option<f32>,
}

fn level(roll: f32, pitch: Option<f32>) -> Option<Level> {
    let sane = |a: f32| a.is_finite() && a.abs() <= 180.0;
    // Adding zero turns -0.0, which some negated records give, into 0.0.
    (sane(roll) && pitch.is_none_or(sane)).then_some(Level { roll: roll + 0.0, pitch: pitch.filter(|p| p.abs() <= 90.0).map(|p| p + 0.0) })
}

/// Nikon's key tables for the encrypted parts of its MakerNote (as in
/// rawler's NEF decoder, LGPL-2.1, and ExifTool), picked by the serial
/// number and the shutter count.
const SERIAL_MAP: [u8; 256] = [
    0xc1, 0xbf, 0x6d, 0x0d, 0x59, 0xc5, 0x13, 0x9d, 0x83, 0x61, 0x6b, 0x4f, 0xc7, 0x7f, 0x3d, 0x3d, 0x53, 0x59, 0xe3, 0xc7, 0xe9, 0x2f, 0x95, 0xa7, 0x95, 0x1f,
    0xdf, 0x7f, 0x2b, 0x29, 0xc7, 0x0d, 0xdf, 0x07, 0xef, 0x71, 0x89, 0x3d, 0x13, 0x3d, 0x3b, 0x13, 0xfb, 0x0d, 0x89, 0xc1, 0x65, 0x1f, 0xb3, 0x0d, 0x6b, 0x29,
    0xe3, 0xfb, 0xef, 0xa3, 0x6b, 0x47, 0x7f, 0x95, 0x35, 0xa7, 0x47, 0x4f, 0xc7, 0xf1, 0x59, 0x95, 0x35, 0x11, 0x29, 0x61, 0xf1, 0x3d, 0xb3, 0x2b, 0x0d, 0x43,
    0x89, 0xc1, 0x9d, 0x9d, 0x89, 0x65, 0xf1, 0xe9, 0xdf, 0xbf, 0x3d, 0x7f, 0x53, 0x97, 0xe5, 0xe9, 0x95, 0x17, 0x1d, 0x3d, 0x8b, 0xfb, 0xc7, 0xe3, 0x67, 0xa7,
    0x07, 0xf1, 0x71, 0xa7, 0x53, 0xb5, 0x29, 0x89, 0xe5, 0x2b, 0xa7, 0x17, 0x29, 0xe9, 0x4f, 0xc5, 0x65, 0x6d, 0x6b, 0xef, 0x0d, 0x89, 0x49, 0x2f, 0xb3, 0x43,
    0x53, 0x65, 0x1d, 0x49, 0xa3, 0x13, 0x89, 0x59, 0xef, 0x6b, 0xef, 0x65, 0x1d, 0x0b, 0x59, 0x13, 0xe3, 0x4f, 0x9d, 0xb3, 0x29, 0x43, 0x2b, 0x07, 0x1d, 0x95,
    0x59, 0x59, 0x47, 0xfb, 0xe5, 0xe9, 0x61, 0x47, 0x2f, 0x35, 0x7f, 0x17, 0x7f, 0xef, 0x7f, 0x95, 0x95, 0x71, 0xd3, 0xa3, 0x0b, 0x71, 0xa3, 0xad, 0x0b, 0x3b,
    0xb5, 0xfb, 0xa3, 0xbf, 0x4f, 0x83, 0x1d, 0xad, 0xe9, 0x2f, 0x71, 0x65, 0xa3, 0xe5, 0x07, 0x35, 0x3d, 0x0d, 0xb5, 0xe9, 0xe5, 0x47, 0x3b, 0x9d, 0xef, 0x35,
    0xa3, 0xbf, 0xb3, 0xdf, 0x53, 0xd3, 0x97, 0x53, 0x49, 0x71, 0x07, 0x35, 0x61, 0x71, 0x2f, 0x43, 0x2f, 0x11, 0xdf, 0x17, 0x97, 0xfb, 0x95, 0x3b, 0x7f, 0x6b,
    0xd3, 0x25, 0xbf, 0xad, 0xc7, 0xc5, 0xc5, 0xb5, 0x8b, 0xef, 0x2f, 0xd3, 0x07, 0x6b, 0x25, 0x49, 0x95, 0x25, 0x49, 0x6d, 0x71, 0xc7,
];
const COUNT_MAP: [u8; 256] = [
    0xa7, 0xbc, 0xc9, 0xad, 0x91, 0xdf, 0x85, 0xe5, 0xd4, 0x78, 0xd5, 0x17, 0x46, 0x7c, 0x29, 0x4c, 0x4d, 0x03, 0xe9, 0x25, 0x68, 0x11, 0x86, 0xb3, 0xbd, 0xf7,
    0x6f, 0x61, 0x22, 0xa2, 0x26, 0x34, 0x2a, 0xbe, 0x1e, 0x46, 0x14, 0x68, 0x9d, 0x44, 0x18, 0xc2, 0x40, 0xf4, 0x7e, 0x5f, 0x1b, 0xad, 0x0b, 0x94, 0xb6, 0x67,
    0xb4, 0x0b, 0xe1, 0xea, 0x95, 0x9c, 0x66, 0xdc, 0xe7, 0x5d, 0x6c, 0x05, 0xda, 0xd5, 0xdf, 0x7a, 0xef, 0xf6, 0xdb, 0x1f, 0x82, 0x4c, 0xc0, 0x68, 0x47, 0xa1,
    0xbd, 0xee, 0x39, 0x50, 0x56, 0x4a, 0xdd, 0xdf, 0xa5, 0xf8, 0xc6, 0xda, 0xca, 0x90, 0xca, 0x01, 0x42, 0x9d, 0x8b, 0x0c, 0x73, 0x43, 0x75, 0x05, 0x94, 0xde,
    0x24, 0xb3, 0x80, 0x34, 0xe5, 0x2c, 0xdc, 0x9b, 0x3f, 0xca, 0x33, 0x45, 0xd0, 0xdb, 0x5f, 0xf5, 0x52, 0xc3, 0x21, 0xda, 0xe2, 0x22, 0x72, 0x6b, 0x3e, 0xd0,
    0x5b, 0xa8, 0x87, 0x8c, 0x06, 0x5d, 0x0f, 0xdd, 0x09, 0x19, 0x93, 0xd0, 0xb9, 0xfc, 0x8b, 0x0f, 0x84, 0x60, 0x33, 0x1c, 0x9b, 0x45, 0xf1, 0xf0, 0xa3, 0x94,
    0x3a, 0x12, 0x77, 0x33, 0x4d, 0x44, 0x78, 0x28, 0x3c, 0x9e, 0xfd, 0x65, 0x57, 0x16, 0x94, 0x6b, 0xfb, 0x59, 0xd0, 0xc8, 0x22, 0x36, 0xdb, 0xd2, 0x63, 0x98,
    0x43, 0xa1, 0x04, 0x87, 0x86, 0xf7, 0xa6, 0x26, 0xbb, 0xd6, 0x59, 0x4d, 0xbf, 0x6a, 0x2e, 0xaa, 0x2b, 0xef, 0xe6, 0x78, 0xb6, 0x4e, 0xe0, 0x2f, 0xdc, 0x7c,
    0xbe, 0x57, 0x19, 0x32, 0x7e, 0x2a, 0xd0, 0xb8, 0xba, 0x29, 0x00, 0x3c, 0x52, 0x7d, 0xa8, 0x49, 0x3b, 0x2d, 0xeb, 0x25, 0x49, 0xfa, 0xa3, 0xaa, 0x39, 0xa7,
    0xc5, 0xa7, 0x50, 0x11, 0x36, 0xfb, 0xc6, 0x67, 0x4a, 0xf5, 0xa5, 0x12, 0x65, 0x7e, 0xb0, 0xdf, 0xaf, 0x4e, 0xb3, 0x61, 0x7f, 0x2f,
];

/// Undoes Nikon's encryption of `data` from byte `start` on.
fn nikon_decrypt(data: &mut [u8], start: usize, serial: u32, count: u32) {
    let key = count.to_le_bytes().iter().fold(0, |k, b| k ^ b);
    let ci = SERIAL_MAP[(serial & 0xFF) as usize] as u32;
    let mut cj = COUNT_MAP[key as usize] as u32;
    for (k, b) in data.iter_mut().skip(start).enumerate() {
        cj = (cj + ci * (0x60 + k as u32)) & 0xFF;
        *b ^= cj as u8;
    }
}

/// Where a ShotInfo record (MakerNote 0x0091) of the Z cameras keeps the
/// offset of its OrientationInfo, by the record's version.
fn orientation_pointer(version: &[u8]) -> Option<usize> {
    Some(match version {
        // Z 6 and Z 7, Z 50, Z 5, Z 6II and Z 7II, Z fc, Z 30.
        b"0800" | b"0801" | b"0802" | b"0803" | b"0804" | b"0807" => 0x98,
        // Z f, Z6III.
        b"0808" | b"0809" => 0x88,
        // Z 9, Z 8.
        b"0805" | b"0806" => 0x84,
        _ => return None,
    })
}

/// Nikon OrientationInfo: roll and pitch as unsigned 16.16 fixed-point
/// degrees, 0 to 360.
fn nikon(p: &Parsed) -> Option<Level> {
    let mn = p.maker.as_ref()?;
    let (t, off) = container::nikon_maker_tiff(mn.parent.src, mn)?;
    let ifd = t.ifd(off)?;
    // ExifTool's key: the serial number when it is all digits, 0x60 otherwise.
    let serial = ifd.get(0x001D).and_then(|e| t.ascii(e)).map_or(0x60, |s| s.parse::<u64>().map_or(0x60, |n| (n & 0xFF) as u32));
    let count = t.uint(ifd.get(0x00A7)?)?;
    let e = ifd.get(0x0091)?;
    let head = t.src.read(t.data_pos(e), 0x9C.min(t.byte_len(e)))?;
    let at = orientation_pointer(head.get(..4)?)?;
    let mut head = head;
    nikon_decrypt(&mut head, 4, serial, count);
    let start = u32_at(&head, at, true)? as usize;
    if start < 4 || start + 8 > t.byte_len(e) {
        return None;
    }
    let mut data = t.src.read(t.data_pos(e), start + 8)?;
    nikon_decrypt(&mut data, 4, serial, count);
    let angle = |at: usize| {
        let v = u32_at(&data, at, true)? as f32 / 65536.0;
        Some(if v > 180.0 { v - 360.0 } else { v })
    };
    level(angle(start)?, angle(start + 4))
}

/// Canon LevelInfo (MakerNote 0x4059): 32-bit words after the record's
/// length, the roll and pitch in tenths of a degree from 0 to 3600, the roll
/// counted anticlockwise.
fn canon(mn: &MakerNote) -> Option<Level> {
    let t = mn.parent;
    let d = t.data(t.ifd_at(mn.pos)?.get(0x4059)?)?;
    let tenths = |word: usize| {
        let v = u32_at(&d, word * 4, t.le)? as i32;
        Some((if v > 1800 { v - 3600 } else { v }) as f32 / 10.0)
    };
    level(-tenths(4)?, tenths(5))
}

/// FujiFilm RollAngle (MakerNote 0x144D, the X-T5 and later), a signed rational.
fn fuji(mn: &MakerNote) -> Option<Level> {
    let (t, ifd) = crate::af::fuji::maker_ifd(mn)?;
    let d = t.data(ifd.get(0x144D).filter(|e| e.typ == 10)?)?;
    let (n, den) = (u32_at(&d, 0, t.le)? as i32, u32_at(&d, 4, t.le)? as i32);
    (den != 0).then(|| n as f32 / den as f32).and_then(|roll| level(roll, None))
}

/// Panasonic RollAngle and PitchAngle (MakerNote 0x0090, 0x0091), signed
/// tenths of a degree, the pitch counted downwards.
fn panasonic(mn: &MakerNote, make: &str) -> Option<Level> {
    let (t, ifd) = crate::af::panasonic::maker_ifd(mn, make)?;
    let tenths = |tag: u16| ifd.get(tag).and_then(|e| t.uint(e)).map(|v| v as u16 as i16 as f32 / 10.0);
    level(tenths(0x0090)?, tenths(0x0091).map(|p| -p))
}

/// Olympus RollAngle and PitchAngle (0x0903, 0x0904 of the CameraSettings
/// IFD): signed tenths of a degree, the roll counted anticlockwise, then 1
/// when the level gauge was on.
fn olympus(mn: &MakerNote) -> Option<Level> {
    let (t, ifd) = container::olympus_maker_ifd(mn)?;
    let settings = t.ifd(t.uint(ifd.get(0x2020)?)?)?;
    let tenths = |tag: u16| {
        let v = t.uints(settings.get(tag)?);
        (v.get(1) == Some(&1)).then(|| v[0] as u16 as i16 as f32 / 10.0)
    };
    level(-tenths(0x0903)?, tenths(0x0904))
}

/// Pentax LevelInfo (MakerNote 0x022B), signed half degrees, both counted
/// the other way round: bytes on most bodies, 16-bit words on the K-3 Mark III.
fn pentax(p: &Parsed, model: &str) -> Option<Level> {
    let (_, t, ifd) = crate::pentax::maker_ifd(p.maker.as_ref()?)?;
    let d = t.data(ifd.get(0x022B)?)?;
    let (roll, pitch) = if model == "PENTAX K-3 Mark III" {
        (u16_at(&d, 3, t.le)? as i16 as f32, u16_at(&d, 5, t.le)? as i16 as f32)
    } else {
        // An orientation of 0: the body measured nothing.
        if *d.first()? & 0x0F == 0 {
            return None;
        }
        (*d.get(1)? as i8 as f32, *d.get(2)? as i8 as f32)
    };
    level(-roll / 2.0, Some(-pitch / 2.0))
}

/// Apple AccelerationVector (MakerNote 0x0008): the pull the phone felt, in
/// g, along its own axes seen from the screen: x to the left, y down and z
/// out of the back, where the main cameras look. Pictures of the front camera
/// (CameraType 0x002E, 6) look the other way and are left alone.
fn apple(mn: &MakerNote) -> Option<Level> {
    let (t, ifd) = container::apple_maker_ifd(mn)?;
    if ifd.get(0x002E).and_then(|e| t.uint(e)) == Some(6) {
        return None;
    }
    let e = ifd.get(0x0008).filter(|e| e.typ == 10 && e.count == 3)?;
    let d = t.data(e)?;
    let v: Vec<f32> = (0..3)
        .map(|i| {
            let (n, den) = (u32_at(&d, i * 8, t.le)? as i32, u32_at(&d, i * 8 + 4, t.le)? as i32);
            (den != 0).then(|| n as f32 / den as f32)
        })
        .collect::<Option<_>>()?;
    from_gravity(v[0], v[1], v[2])
}

/// The level from which way is up along the phone's axes, held still.
fn from_gravity(x: f32, y: f32, z: f32) -> Option<Level> {
    let g = (x * x + y * y + z * z).sqrt();
    if !(0.5..=1.5).contains(&g) {
        return None;
    }
    // Upright on its end the phone feels (0, -1, 0); turned clockwise, "up" swings to its left.
    level(x.atan2(-y).to_degrees(), Some((z / g).asin().to_degrees()))
}

pub fn read(p: &Parsed, make: &str, model: &str) -> Option<Level> {
    let mn = p.maker.as_ref()?;
    let make = make.to_ascii_uppercase();
    if make.starts_with("NIKON") {
        nikon(p)
    } else if make.starts_with("CANON") {
        canon(mn)
    } else if make.starts_with("FUJIFILM") {
        fuji(mn)
    } else if make.starts_with("PANASONIC") || make.starts_with("LEICA") {
        panasonic(mn, &make)
    } else if make.starts_with("OLYMPUS") || make.starts_with("OM DIGITAL") {
        olympus(mn)
    } else if make.starts_with("PENTAX") || make.starts_with("RICOH") || make.starts_with("ASAHI") {
        pentax(p, model)
    } else if make.starts_with("APPLE") {
        apple(mn)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gravity_gives_clockwise_roll_and_upward_pitch() {
        let near = |l: Level, roll: f32, pitch: f32| (l.roll - roll).abs() < 0.01 && (l.pitch.unwrap() - pitch).abs() < 0.01;
        let s = 10f32.to_radians().sin();
        let c = 10f32.to_radians().cos();
        assert!(near(from_gravity(0.0, -1.0, 0.0).unwrap(), 0.0, 0.0));
        // Turned clockwise, "up" swings to the phone's left side.
        assert!(near(from_gravity(s, -c, 0.0).unwrap(), 10.0, 0.0));
        // On its side, top to the right, for a landscape frame.
        assert!(near(from_gravity(1.0, 0.0, 0.0).unwrap(), 90.0, 0.0));
        // Tilted back so the lens looks up, "up" leans out of the back.
        assert!(near(from_gravity(0.0, -c, s).unwrap(), 0.0, 10.0));
        assert!(from_gravity(0.0, 0.1, 0.0).is_none());
    }

    #[test]
    fn zero_is_never_negative() {
        let l = level(-0.0, Some(-0.0)).unwrap();
        assert!(l.roll.is_sign_positive() && l.pitch.unwrap().is_sign_positive());
        assert!(level(f32::NAN, None).is_none());
        assert_eq!(level(10.0, Some(120.0)).unwrap().pitch, None);
    }
}
