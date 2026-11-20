//! Shots that belong together: the frames of one burst, and the frames of a
//! pixel-shift series (with the file merged from them, when it sits beside
//! them). Each file gives a [`Hint`]; [`group`] turns the hints of a folder
//! into [`Stack`]s.
//!
//! Recent Nikon bodies number every series themselves (MakerNote 0x0056: a
//! series id and the frame's position in it; 0x005E is filled in for
//! pixel-shift frames), so their stacks are exact; so are iPhone bursts (one
//! BurstUUID for every frame) and Sony pixel-shift series. Other cameras are
//! grouped by capture time: frames of one camera that follow each other
//! closely, helped by the drive mode (Canon, Sony, Fujifilm, Panasonic and
//! Leica, Olympus and OM System, Pentax and Ricoh) and the frame's number in
//! its burst, where the file records them.

use crate::container::{self, MakerNote, Parsed};
use crate::io::{u16_at, u32_at, Src};
use crate::{formats, Result};
use serde::{Deserialize, Serialize};
use std::path::Path;

/// Frames of one camera-numbered series are at most this far apart. Nikon's
/// ids are 16 bits wide and come round again, and burst frame numbers start
/// again at 1, so they only count between neighbours.
const SERIES_GAP_MS: i64 = 60_000;
/// Largest pause between frames of a burst when the file says the drive mode
/// was continuous (slow continuous modes go down to about one frame a second).
const CONTINUOUS_GAP_MS: i64 = 1_200;
/// The same when the drive mode is unknown: quicker than a finger presses twice.
const UNKNOWN_GAP_MS: i64 = 500;

#[derive(Serialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Burst,
    /// Frames shot with the sensor moved between them, to be merged later.
    PixelShift,
}

/// What one file says about the series it was shot in.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Hint {
    /// Make and model; only frames of one camera form a stack.
    pub camera: Option<String>,
    /// Capture time in milliseconds (an arbitrary but common origin).
    pub time_ms: Option<i64>,
    /// Series id given by the camera; `None` when it gave none.
    pub series: Option<u32>,
    /// Position inside the series, from 1.
    pub seq: Option<u32>,
    pub pixel_shift: bool,
    /// The file is the result of merging a pixel-shift series (`.NEFX`).
    pub merged: bool,
    /// Drive mode, where the file records it.
    pub continuous: Option<bool>,
}

/// A group of shots, as indices into the list of hints, in shooting order.
/// A merged pixel-shift file comes first.
#[derive(Clone, Debug, PartialEq)]
pub struct Stack {
    pub kind: Kind,
    pub members: Vec<usize>,
}

/// Days since 1970-01-01 of a proleptic Gregorian date.
fn days(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y.rem_euclid(400);
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
    era * 146_097 + yoe * 365 + yoe / 4 - yoe / 100 + doy - 719_468
}

/// Milliseconds of an EXIF date (`YYYY:MM:DD HH:MM:SS`) and its sub-second digits.
fn time_ms(date: &str, subsec: Option<&str>) -> Option<i64> {
    let n: Vec<i64> = date.split(|c: char| !c.is_ascii_digit()).filter(|s| !s.is_empty()).map(|s| s.parse().ok()).collect::<Option<_>>()?;
    let [y, mo, d, h, mi, s] = n[..] else { return None };
    if y < 1970 || !(1..=12).contains(&mo) || !(1..=31).contains(&d) {
        return None;
    }
    // "5", "50" and "500" all mean half a second.
    let frac = subsec.map(str::trim).filter(|s| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit())).map_or(0, |s| {
        let digits: String = s.chars().chain("000".chars()).take(3).collect();
        digits.parse().unwrap_or(0)
    });
    Some(((days(y, mo, d) * 24 + h) * 60 + mi) * 60_000 + s * 1000 + frac)
}

/// Nikon: drive mode, series id, frame number and the pixel-shift record.
fn nikon(p: &Parsed, hint: &mut Hint) -> Option<()> {
    let mn = p.maker.as_ref()?;
    let (t, off) = container::nikon_maker_tiff(mn.parent.src, mn)?;
    let ifd = t.ifd(off)?;
    // ShootingMode: bit 0 is continuous release.
    if let Some(mode) = ifd.get(0x0089).and_then(|e| t.uint(e)) {
        hint.continuous = Some(mode & 1 == 1);
    }
    // `0100`, the series id (in the MakerNote's byte order), two bytes, the frame number.
    if let Some(d) = ifd.get(0x0056).and_then(|e| t.data(e)).filter(|d| d.len() >= 9 && d.starts_with(b"01")) {
        let id = u16_at(&d, 4, t.le).unwrap_or(0);
        if id != 0 {
            hint.series = Some(id as u32);
            hint.seq = Some(d[8] as u32).filter(|n| *n > 0);
        }
    }
    // Zeroes except on pixel-shift frames, where the second byte counts the frames.
    if let Some(d) = ifd.get(0x005E).and_then(|e| t.data(e)).filter(|d| d.len() >= 6 && d.starts_with(b"01")) {
        if d[4] != 0 && d[5] != 0 {
            hint.pixel_shift = true;
            hint.seq = Some(d[5] as u32);
        }
    }
    Some(())
}

/// Canon CameraSettings (MakerNote 0x0001), word 5: ContinuousDrive.
fn canon(p: &Parsed, hint: &mut Hint) -> Option<()> {
    let mn = p.maker.as_ref()?;
    let t = &mn.parent;
    let e = t.ifd_at(mn.pos)?;
    let v = container::u16s(&t.data(e.get(0x0001)?)?, t.le);
    hint.continuous = match *v.get(5)? {
        0 | 6 | 9 => Some(false),
        1 | 3 | 4 | 5 | 8 | 10 => Some(true),
        _ => None,
    };
    Some(())
}

/// Sony ReleaseMode (MakerNote 0xB049), SequenceNumber (0xB04A, the frame's
/// number in its burst) and PixelShiftInfo (0x202F: a group id, then the
/// frame's number and the number of frames).
fn sony(p: &Parsed, hint: &mut Hint) -> Option<()> {
    let (t, ifd) = container::sony_maker_ifd(p)?;
    let one = |tag: u16| ifd.get(tag).and_then(|e| t.uint(e));
    hint.continuous = match one(0xB049) {
        Some(0) => Some(false),
        // Continuous, and exposure, white balance and DRO bracketing.
        Some(2 | 5 | 6 | 8) => Some(true),
        _ => None,
    };
    hint.seq = one(0xB04A).filter(|n| *n > 0 && *n != 0xFFFF);
    if let Some(d) = ifd.get(0x202F).and_then(|e| t.data(e)).filter(|d| d.len() >= 6) {
        let group = u32_at(&d, 0, t.le).unwrap_or(0);
        if group != 0 && d[4] != 0 && d[5] != 0 {
            hint.pixel_shift = true;
            hint.series = Some(group);
            hint.seq = Some(d[4] as u32);
        }
    }
    Some(())
}

/// FujiFilm AutoBracketing (MakerNote 0x1100; 6 is a pixel-shift series),
/// SequenceNumber (0x1101) and DriveSettings (0x1103, the drive mode in its
/// low byte).
fn fuji(mn: &MakerNote, hint: &mut Hint) -> Option<()> {
    let (t, ifd) = crate::af::fuji::maker_ifd(mn)?;
    let one = |tag: u16| ifd.get(tag).and_then(|e| t.uint(e));
    let bracket = one(0x1100);
    hint.seq = one(0x1101).filter(|n| *n > 0);
    hint.pixel_shift = bracket == Some(6);
    hint.continuous = match (one(0x1103).map(|v| v & 0xFF), bracket) {
        // Pixel-shift frames are shot in single-frame mode, one by one.
        (_, Some(1 | 2 | 6)) => Some(true),
        (Some(0), _) => Some(false),
        (Some(1 | 2), _) => Some(true),
        _ => None,
    };
    Some(())
}

/// Panasonic BurstMode (MakerNote 0x002A) and SequenceNumber (0x002B), also
/// in Leica's cameras made by Panasonic.
fn panasonic(mn: &MakerNote, make: &str, hint: &mut Hint) -> Option<()> {
    let (t, ifd) = crate::af::panasonic::maker_ifd(mn, make)?;
    let one = |tag: u16| ifd.get(tag).and_then(|e| t.uint(e));
    hint.continuous = match one(0x002A) {
        Some(0) => Some(false),
        // On, exposure, focus, white balance and aperture bracketing, unlimited, on with flash.
        Some(1..=4 | 8 | 17 | 18) => Some(true),
        _ => None,
    };
    hint.seq = one(0x002B).filter(|n| *n > 0);
    Some(())
}

/// Olympus and OM System DriveMode (0x0600 of the CameraSettings IFD,
/// MakerNote 0x2020): the mode, 0 for a single shot, then the frame's number.
fn olympus(mn: &MakerNote, hint: &mut Hint) -> Option<()> {
    let (t, ifd) = container::olympus_maker_ifd(mn)?;
    let settings = t.ifd(t.uint(ifd.get(0x2020)?)?)?;
    let v = t.uints(settings.get(0x0600)?);
    hint.continuous = match *v.first()? {
        0 => Some(false),
        // Continuous, and exposure, white balance and other bracketing.
        1..=5 => Some(true),
        _ => None,
    };
    hint.seq = v.get(1).copied().filter(|n| *n > 0);
    Some(())
}

/// Apple BurstUUID (MakerNote 0x000B): the same for every frame of a burst.
fn apple(mn: &MakerNote, hint: &mut Hint) -> Option<()> {
    let (t, ifd) = container::apple_maker_ifd(mn)?;
    let id = t.ascii(ifd.get(0x000B)?)?;
    // FNV-1a: a series id has to be a number.
    hint.series = Some(id.bytes().fold(0x811C_9DC5u32, |h, b| (h ^ b as u32).wrapping_mul(0x0100_0193)));
    hint.continuous = Some(true);
    Some(())
}

pub fn hint(p: &Parsed, path: &Path) -> Hint {
    let text = |tag: u16| [&p.exif, &p.ifd0].into_iter().flatten().find_map(|(t, i)| i.get(tag).and_then(|e| t.ascii(e)));
    let (make, model) = (text(0x010F), text(0x0110));
    let mut hint = Hint {
        time_ms: text(0x9003).or_else(|| text(0x0132)).and_then(|d| time_ms(&d, text(0x9291).or_else(|| text(0x9290)).as_deref())),
        merged: formats::ext_of(path) == "nefx",
        ..Hint::default()
    };
    let mn = p.maker.as_ref();
    match make.as_deref().map(str::to_ascii_uppercase) {
        Some(m) if m.starts_with("NIKON") => {
            nikon(p, &mut hint);
        }
        Some(m) if m.starts_with("CANON") => {
            canon(p, &mut hint);
        }
        Some(m) if m.starts_with("SONY") => {
            sony(p, &mut hint);
        }
        Some(m) if m.starts_with("FUJIFILM") => {
            mn.and_then(|mn| fuji(mn, &mut hint));
        }
        Some(m) if m.starts_with("PANASONIC") || m.starts_with("LEICA") => {
            mn.and_then(|mn| panasonic(mn, &m, &mut hint));
        }
        Some(m) if m.starts_with("OLYMPUS") || m.starts_with("OM DIGITAL") => {
            mn.and_then(|mn| olympus(mn, &mut hint));
        }
        Some(m) if m.starts_with("PENTAX") || m.starts_with("RICOH") => {
            hint.continuous = crate::pentax::continuous(p);
        }
        Some(m) if m.starts_with("APPLE") => {
            mn.and_then(|mn| apple(mn, &mut hint));
        }
        _ => {}
    }    hint.camera = match (make, model) {
        (None, None) => None,
        (make, model) => Some(format!("{} {}", make.unwrap_or_default(), model.unwrap_or_default())),
    };
    hint
}

pub fn read(path: &Path) -> Result<Hint> {
    let src = Src::open(path)?;
    Ok(hint(&container::parse_metadata(&src), path))
}

/// Whether `b`, the next frame the same camera took after `a`, continues its series.
fn follows(a: &Hint, b: &Hint, gap: i64) -> bool {
    if a.pixel_shift != b.pixel_shift {
        return false;
    }
    match (a.series, b.series) {
        (Some(x), Some(y)) => x == y && gap <= SERIES_GAP_MS,
        // A camera that numbers its series left this frame out of every one.
        (Some(_), None) | (None, Some(_)) => false,
        (None, None) => match (a.continuous, b.continuous) {
            (Some(false), _) | (_, Some(false)) => false,
            // Frame numbers start again at 1 with every burst; frames culled
            // from the middle of one leave gaps in them.
            _ if a.seq.is_some() && b.seq.is_some() => b.seq > a.seq && gap <= SERIES_GAP_MS,
            (Some(true), Some(true)) => gap <= CONTINUOUS_GAP_MS,
            _ => gap <= UNKNOWN_GAP_MS,
        },
    }
}

/// Groups the shots of a folder. Shots that stand alone are in no stack.
pub fn group(hints: &[Hint]) -> Vec<Stack> {
    let mut order: Vec<usize> = (0..hints.len()).filter(|&i| hints[i].time_ms.is_some() && hints[i].camera.is_some()).collect();
    order.sort_by_key(|&i| (hints[i].camera.as_deref(), hints[i].time_ms, i));
    let mut stacks = Vec::new();
    let mut run: Vec<usize> = Vec::new();
    let mut close = |run: &mut Vec<usize>| {
        if run.len() > 1 {
            let kind = if hints[run[0]].pixel_shift { Kind::PixelShift } else { Kind::Burst };
            let mut members = std::mem::take(run);
            members.sort_by_key(|&i| (!hints[i].merged, hints[i].seq.unwrap_or(0), hints[i].time_ms, i));
            stacks.push(Stack { kind, members });
        }
        run.clear();
    };
    for i in order {
        let joins = run.last().is_some_and(|&last| {
            let (a, b) = (&hints[last], &hints[i]);
            a.camera == b.camera && follows(a, b, b.time_ms.unwrap_or(0) - a.time_ms.unwrap_or(0))
        });
        if !joins {
            close(&mut run);
        }
        run.push(i);
    }
    close(&mut run);
    stacks.sort_by_key(|s| s.members.iter().copied().min());
    stacks
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::af::tests::{bytes, file, ifd, long, shorts, text, Entry};

    fn shot(ms: i64) -> Hint {
        Hint { camera: Some("NIKON Z f".into()), time_ms: Some(ms), ..Hint::default() }
    }

    fn numbered(ms: i64, series: u32, seq: u32) -> Hint {
        Hint { series: Some(series), seq: Some(seq), continuous: Some(true), ..shot(ms) }
    }

    #[test]
    fn exif_times_become_milliseconds() {
        let at = |d, s| time_ms(d, s).unwrap();
        assert_eq!(at("2026:10:27 20:49:39", Some("15")) - at("2026:10:27 20:49:39", Some("03")), 120);
        assert_eq!(at("2026:10:27 20:49:40", Some("5")) - at("2026:10:27 20:49:39", None), 1500);
        assert_eq!(at("2026:03:01 00:00:00", None) - at("2026:02:28 23:59:59", None), 1000);
        assert_eq!(at("2024:03:01 00:00:00", None) - at("2024:02:28 00:00:00", None), 2 * 86_400_000);
        assert_eq!(at("1970:01:01 00:00:01", Some("250")), 1250);
        assert!(time_ms("0000:00:00 00:00:00", None).is_none());
        assert!(time_ms("not a date", None).is_none());
    }

    #[test]
    fn a_numbered_series_is_one_stack_whatever_its_pace() {
        // Two bursts a second apart, then a frame shot on its own.
        let hints = vec![numbered(0, 416, 1), numbered(140, 416, 2), numbered(280, 416, 3), numbered(1300, 576, 1), numbered(1440, 576, 2), Hint { continuous: Some(false), ..shot(1500) }];
        let stacks = group(&hints);
        assert_eq!(stacks, vec![Stack { kind: Kind::Burst, members: vec![0, 1, 2] }, Stack { kind: Kind::Burst, members: vec![3, 4] }]);
    }

    #[test]
    fn a_series_id_that_comes_round_again_starts_a_new_stack() {
        let hints = vec![numbered(0, 416, 1), numbered(140, 416, 2), numbered(3_600_000, 416, 1), numbered(3_600_140, 416, 2)];
        assert_eq!(group(&hints).len(), 2);
    }

    #[test]
    fn pixel_shift_frames_follow_the_merged_file() {
        let frame = |ms, seq| Hint { pixel_shift: true, continuous: Some(false), ..numbered(ms, 8482, seq) };
        // Listed by name: DSC_0265.NEF, DSC_0265_merged.NEFX, DSC_0266.NEF, DSC_0267.NEF.
        let hints = vec![frame(0, 1), Hint { merged: true, ..frame(0, 1) }, frame(120, 2), frame(250, 3)];
        assert_eq!(group(&hints), vec![Stack { kind: Kind::PixelShift, members: vec![1, 0, 2, 3] }]);
    }

    #[test]
    fn a_burst_right_after_a_pixel_shift_series_is_its_own_stack() {
        let shift = |ms, seq| Hint { pixel_shift: true, ..numbered(ms, 64, seq) };
        let hints = vec![shift(0, 1), shift(100, 2), numbered(300, 64, 1), numbered(400, 64, 2)];
        let kinds: Vec<Kind> = group(&hints).iter().map(|s| s.kind).collect();
        assert_eq!(kinds, vec![Kind::PixelShift, Kind::Burst]);
    }

    #[test]
    fn without_numbers_the_drive_mode_and_the_clock_decide() {
        let drive = |ms, on| Hint { continuous: Some(on), ..shot(ms) };
        // Continuous at one frame a second holds together; single frames never do.
        assert_eq!(group(&[drive(0, true), drive(1000, true), drive(2000, true), drive(5000, true)]), vec![Stack { kind: Kind::Burst, members: vec![0, 1, 2] }]);
        assert!(group(&[drive(0, false), drive(100, false)]).is_empty());
        // Unknown drive mode: only frames that follow each other at once.
        assert_eq!(group(&[shot(0), shot(300), shot(1200)]), vec![Stack { kind: Kind::Burst, members: vec![0, 1] }]);
    }

    #[test]
    fn two_cameras_shooting_at_once_are_kept_apart() {
        let other = |ms| Hint { camera: Some("Canon EOS R5".into()), ..shot(ms) };
        let stacks = group(&[shot(0), other(50), shot(100), other(150), Hint::default()]);
        assert_eq!(stacks, vec![Stack { kind: Kind::Burst, members: vec![0, 2] }, Stack { kind: Kind::Burst, members: vec![1, 3] }]);
    }
    fn hint_of(file: Vec<u8>) -> Hint {
        let src = Src::mem(file);
        hint(&container::parse_metadata(&src), Path::new("frame"))
    }

    /// A file by `make` whose MakerNote is `header` and an IFD with offsets counted from the note.
    fn noted(make: &str, header: &[u8], entries: &[Entry]) -> Vec<u8> {
        file(make, &[], |_| [header, &ifd(entries, header.len() as u32)].concat())
    }

    #[test]
    fn sony_release_mode_frame_number_and_pixel_shift() {
        // ARW: the IFD starts the note, its offsets counted from the file.
        let sony = |entries: &[Entry]| hint_of(file("SONY", &[], |at| ifd(entries, at)));
        let h = sony(&[shorts(0xB049, &[2]), shorts(0xB04A, &[3])]);
        assert_eq!((h.continuous, h.seq, h.pixel_shift), (Some(true), Some(3), false));
        let h = sony(&[shorts(0xB049, &[0]), shorts(0xB04A, &[0])]);
        assert_eq!((h.continuous, h.seq), (Some(false), None));
        assert_eq!(sony(&[shorts(0xB049, &[65535])]).continuous, None);
        let h = sony(&[shorts(0xB049, &[0]), bytes(0x202F, &[0x34, 0x12, 0, 0, 2, 4])]);
        assert_eq!((h.series, h.seq, h.pixel_shift), (Some(0x1234), Some(2), true));
        assert!(!sony(&[bytes(0x202F, &[0; 6])]).pixel_shift);
    }

    #[test]
    fn fujifilm_drive_settings_and_pixel_shift() {
        let fuji = |entries: &[Entry]| hint_of(noted("FUJIFILM", b"FUJIFILM\x0C\0\0\0", entries));
        // Continuous high at 8 frames a second, third frame.
        let h = fuji(&[shorts(0x1100, &[0]), shorts(0x1101, &[3]), long(0x1103, 0x0800_0002)]);
        assert_eq!((h.continuous, h.seq), (Some(true), Some(3)));
        assert_eq!(fuji(&[long(0x1103, 0)]).continuous, Some(false));
        assert_eq!(fuji(&[long(0x1103, 0), shorts(0x1100, &[1])]).continuous, Some(true));
        let h = fuji(&[shorts(0x1100, &[6]), shorts(0x1101, &[5]), long(0x1103, 0)]);
        assert_eq!((h.continuous, h.seq, h.pixel_shift), (Some(true), Some(5), true));
    }

    #[test]
    fn panasonic_and_leica_burst_mode() {
        let note = |make: &str, header: &[u8], entries: &[Entry]| hint_of(file(make, &[], |at| [header, &ifd(entries, at + header.len() as u32)].concat()));
        let h = note("Panasonic", b"Panasonic\0\0\0", &[shorts(0x002A, &[1]), long(0x002B, 2)]);
        assert_eq!((h.continuous, h.seq), (Some(true), Some(2)));
        let h = note("LEICA CAMERA AG", b"LEICA CAMERA AG\0\0\0", &[shorts(0x002A, &[0]), long(0x002B, 0)]);
        assert_eq!((h.continuous, h.seq), (Some(false), None));
        assert_eq!(note("Panasonic", b"Panasonic\0\0\0", &[shorts(0x002A, &[99])]).continuous, None);
    }

    #[test]
    fn olympus_and_om_system_drive_mode() {
        let note = |make: &str, header: &[u8], drive: &[u16]| {
            let main_at = header.len() as u32;
            let settings_at = main_at + 2 + 12 + 4;
            let main = ifd(&[(0x2020, 13, 1, settings_at.to_le_bytes().to_vec())], main_at);
            hint_of(file(make, &[], |_| [header, &main, &ifd(&[shorts(0x0600, drive)], settings_at)].concat()))
        };
        let h = note("OLYMPUS CORPORATION", b"OLYMPUS\0II\x03\0", &[1, 4, 0, 0, 0]);
        assert_eq!((h.continuous, h.seq), (Some(true), Some(4)));
        let h = note("OM Digital Solutions", b"OM SYSTEM\0\0\0II\x04\0", &[0, 0, 0, 0, 2, 5]);
        assert_eq!((h.continuous, h.seq), (Some(false), None));
        // Exposure bracketing.
        assert_eq!(note("OM Digital Solutions", b"OM SYSTEM\0\0\0II\x04\0", &[2, 1]).continuous, Some(true));
    }

    #[test]
    fn iphone_bursts_share_one_id() {
        let iphone = |id: &str| hint_of(noted("Apple", b"Apple iOS\0\0\x01II", &[text(0x000B, id)]));
        let (a, b, c) = (iphone("D87A1F0C-5E1B-4D6A-9E0B-1A2B3C4D5E6F"), iphone("D87A1F0C-5E1B-4D6A-9E0B-1A2B3C4D5E6F"), iphone("0F9E8D7C-6B5A-4938-8271-605F4E3D2C1B"));
        assert!(a.series.is_some() && a.series == b.series && a.series != c.series);
        assert_eq!(a.continuous, Some(true));
        assert_eq!(hint_of(noted("Apple", b"Apple iOS\0\0\x01II", &[shorts(0x0001, &[12])])).series, None);
    }

    #[test]
    fn frame_numbers_split_bursts_shot_back_to_back() {
        let frame = |ms, seq| Hint { seq: Some(seq), continuous: Some(true), ..shot(ms) };
        // Two bursts a frame apart, then one culled down to its first and last frames.
        let hints = [frame(0, 1), frame(100, 2), frame(200, 3), frame(300, 1), frame(400, 2), frame(5000, 1), frame(9000, 6)];
        let members: Vec<Vec<usize>> = group(&hints).into_iter().map(|s| s.members).collect();
        assert_eq!(members, vec![vec![0, 1, 2], vec![3, 4], vec![5, 6]]);
        // Pixel-shift frames shot seconds apart in single-frame mode.
        let shift = |ms, seq| Hint { pixel_shift: true, ..frame(ms, seq) };
        assert_eq!(group(&[shift(0, 1), shift(4000, 2), shift(8000, 3), frame(8300, 1)]), vec![Stack { kind: Kind::PixelShift, members: vec![0, 1, 2] }]);
    }
}
