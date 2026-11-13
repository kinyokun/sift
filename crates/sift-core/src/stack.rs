//! Shots that belong together: the frames of one burst, and the frames of a
//! pixel-shift series (with the file merged from them, when it sits beside
//! them). Each file gives a [`Hint`]; [`group`] turns the hints of a folder
//! into [`Stack`]s.
//!
//! Recent Nikon bodies number every series themselves (MakerNote 0x0056: a
//! series id and the frame's position in it; 0x005E is filled in for
//! pixel-shift frames), so their stacks are exact. Other cameras are grouped
//! by capture time: frames of one camera that follow each other closely,
//! helped by the drive mode where the file records it (Nikon, Canon).

use crate::container::{self, Parsed};
use crate::io::{u16_at, Src};
use crate::{formats, Result};
use serde::Serialize;
use std::path::Path;

/// Frames of one camera-numbered series are at most this far apart. The id is
/// 16 bits wide and comes round again, so it only counts between neighbours.
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
#[derive(Clone, Debug, Default, PartialEq)]
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
        1 | 3 | 4 | 5 | 10 => Some(true),
        _ => None,
    };
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
    match make.as_deref().map(str::to_ascii_uppercase) {
        Some(m) if m.starts_with("NIKON") => {
            nikon(p, &mut hint);
        }
        Some(m) if m.starts_with("CANON") => {
            canon(p, &mut hint);
        }
        _ => {}
    }
    hint.camera = match (make, model) {
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
}
