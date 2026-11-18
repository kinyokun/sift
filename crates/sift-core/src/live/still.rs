//! The still half of a Live Photo, made without the system: a copy of a JPEG
//! whose EXIF block carries the content identifier in an Apple maker note
//! (key 17), which is where Photos looks for it. The picture is not decoded:
//! outside the EXIF block every byte stays as it was, and inside it only the
//! maker note changes. A maker note of the camera's is replaced, as ImageIO
//! replaces it (`live.m`): a still has one maker note, and Photos only reads
//! Apple's.

use crate::io::{u16_at, u32_at, Src};
use crate::jpeg::EXIF_HEADER;
use crate::tiff::{type_size, T_EXIF_IFD, T_MAKERNOTE, T_ORIENTATION};
use crate::{container, Error, Result};
use std::ops::Range;
use std::path::Path;

/// The longest TIFF block an APP1 segment holds: its length field counts
/// itself and the `Exif` header too.
const MAX_BLOCK: usize = 0xFFFF - 2 - EXIF_HEADER.len();
const UNDEFINED: u16 = 7;
const LONG: u16 = 4;

/// Apple's maker note with the content identifier as its only entry, byte
/// for byte what ImageIO writes. It is big-endian whatever the EXIF block is,
/// and its offsets count from its own start, so it can sit anywhere.
fn maker_note(identifier: &str) -> Vec<u8> {
    let mut note = b"Apple iOS\0\0\x01MM\0\x01".to_vec();
    let value = note.len() + 12 + 4;
    note.extend_from_slice(&entry(false, 0x0011, 2, identifier.len() as u32 + 1, long(false, value as u32)));
    note.extend_from_slice(&[0; 4]);
    note.extend_from_slice(identifier.as_bytes());
    note.push(0);
    note
}

fn short(le: bool, v: u16) -> [u8; 2] {
    if le { v.to_le_bytes() } else { v.to_be_bytes() }
}

fn long(le: bool, v: u32) -> [u8; 4] {
    if le { v.to_le_bytes() } else { v.to_be_bytes() }
}

fn entry(le: bool, tag: u16, typ: u16, count: u32, value: [u8; 4]) -> [u8; 12] {
    let mut e = [0u8; 12];
    e[..2].copy_from_slice(&short(le, tag));
    e[2..4].copy_from_slice(&short(le, typ));
    e[4..8].copy_from_slice(&long(le, count));
    e[8..].copy_from_slice(&value);
    e
}

/// A directory of a TIFF block: where it is and its entries as stored.
struct Dir {
    at: usize,
    entries: Vec<[u8; 12]>,
    next: u32,
}

impl Dir {
    fn read(block: &[u8], at: usize, le: bool) -> Option<Dir> {
        let n = u16_at(block, at, le)? as usize;
        let entries: Vec<[u8; 12]> = block.get(at + 2..at + 2 + n * 12)?.as_chunks::<12>().0.to_vec();
        (n > 0).then_some(Dir { at, entries, next: u32_at(block, at + 2 + n * 12, le).unwrap_or(0) })
    }

    /// Index of the entry with this tag.
    fn find(&self, tag: u16, le: bool) -> Option<usize> {
        self.entries.iter().position(|e| u16_at(e, 0, le) == Some(tag))
    }

    /// Position in the block of the entry at `index`.
    fn slot(&self, index: usize) -> usize {
        self.at + 2 + index * 12
    }

    /// The directory with one more entry, in tag order as TIFF asks.
    fn with(&self, added: [u8; 12], le: bool) -> Vec<u8> {
        let mut entries = self.entries.clone();
        entries.push(added);
        entries.sort_by_key(|e| u16_at(e, 0, le));
        directory(le, &entries, self.next)
    }
}

fn directory(le: bool, entries: &[[u8; 12]], next: u32) -> Vec<u8> {
    let mut out = short(le, entries.len() as u16).to_vec();
    out.extend(entries.iter().flatten());
    out.extend_from_slice(&long(le, next));
    out
}

/// Where the next value can go: the end of the block, on a word boundary.
fn end(block: &mut Vec<u8>) -> u32 {
    if block.len() % 2 == 1 {
        block.push(0);
    }
    block.len() as u32
}

/// The TIFF block with `note` as its maker note. Values keep their places,
/// since everything in a TIFF block is found by its offset: the note goes
/// where the old one was when it fits there, and what else is needed (a
/// directory that gains an entry, a longer note) is added at the end.
/// `None` when the block cannot be read.
fn amended(block: &[u8], note: &[u8]) -> Option<Vec<u8>> {
    let le = match block.get(..2)? {
        b"II" => true,
        b"MM" => false,
        _ => return None,
    };
    let mut out = block.to_vec();
    let ifd0 = Dir::read(block, u32_at(block, 4, le)? as usize, le)?;
    let pointer = ifd0.find(T_EXIF_IFD, le);
    let exif = pointer.and_then(|i| Dir::read(block, u32_at(&ifd0.entries[i], 8, le)? as usize, le));
    let made = |at: u32| entry(le, T_MAKERNOTE, UNDEFINED, note.len() as u32, long(le, at));

    if let Some((exif, i)) = exif.as_ref().and_then(|exif| Some((exif, exif.find(T_MAKERNOTE, le)?))) {
        let old = &exif.entries[i];
        let length = type_size(u16_at(old, 2, le)?).saturating_mul(u32_at(old, 4, le)? as usize);
        let at = u32_at(old, 8, le)? as usize;
        // The camera's note, when it lies in the block, makes room for Apple's or is blanked.
        let place = at.checked_add(length).filter(|end| length > 4 && *end <= block.len()).map(|end| at..end);
        let at = match place {
            Some(place) if place.len() >= note.len() => {
                out[place.clone()].fill(0);
                out[at..at + note.len()].copy_from_slice(note);
                at as u32
            }
            place => {
                if let Some(place) = place {
                    out[place].fill(0);
                }
                let at = end(&mut out);
                out.extend_from_slice(note);
                at
            }
        };
        let slot = exif.slot(i);
        out[slot..slot + 12].copy_from_slice(&made(at));
        return Some(out);
    }

    // The EXIF directory gets an entry for the note, which makes it longer:
    // it is written again at the end, and what pointed to it is pointed there.
    let slot = match pointer {
        Some(i) => ifd0.slot(i) + 8,
        None => {
            // No EXIF directory yet, so the first directory gains its pointer the same way.
            let at = end(&mut out);
            let pointing = entry(le, T_EXIF_IFD, LONG, 1, [0; 4]);
            let first = ifd0.with(pointing, le);
            let index = ifd0.entries.iter().filter(|e| u16_at(*e, 0, le) < Some(T_EXIF_IFD)).count();
            out.extend_from_slice(&first);
            out[4..8].copy_from_slice(&long(le, at));
            at as usize + 2 + index * 12 + 8
        }
    };
    let at = end(&mut out);
    let entries = exif.as_ref().map_or(1, |exif| exif.entries.len() + 1);
    let note_at = at + 2 + entries as u32 * 12 + 4;
    out.extend(match &exif {
        Some(exif) => exif.with(made(note_at), le),
        None => directory(le, &[made(note_at)], 0),
    });
    out.extend_from_slice(note);
    out[slot..slot + 4].copy_from_slice(&long(le, at));
    Some(out)
}

/// A TIFF block with nothing but the orientation, if any, and the maker note.
fn fresh(orientation: Option<u16>, note: &[u8]) -> Vec<u8> {
    let mut block = b"MM\0*\0\0\0\x08".to_vec();
    let turned = orientation.map(|o| entry(false, T_ORIENTATION, 3, 1, [(o >> 8) as u8, o as u8, 0, 0]));
    let first = turned.iter().len() as u32 + 1;
    let exif = 8 + 2 + first * 12 + 4;
    let entries: Vec<[u8; 12]> = turned.into_iter().chain([entry(false, T_EXIF_IFD, LONG, 1, long(false, exif))]).collect();
    block.extend(directory(false, &entries, 0));
    block.extend(directory(false, &[entry(false, T_MAKERNOTE, UNDEFINED, note.len() as u32, long(false, exif + 2 + 12 + 4))], 0));
    block.extend_from_slice(note);
    block
}

/// The orientation a TIFF block records.
pub fn orientation(block: &[u8]) -> Option<u16> {
    let le = block.get(..2)? == b"II";
    let ifd0 = Dir::read(block, u32_at(block, 4, le)? as usize, le)?;
    u16_at(&ifd0.entries[ifd0.find(T_ORIENTATION, le)?], 8, le).filter(|o| (1..=8).contains(o))
}

/// What a camera records about a shot that its picture should keep once it
/// is out of the RAW file: who made it and when, with what, and how it was
/// exposed. Tags of the first directory, then of the EXIF directory.
const SHOT_FIRST: [u16; 6] = [0x010F, 0x0110, 0x0131, 0x0132, 0x013B, 0x8298];
const SHOT_EXIF: [u16; 26] = [
    0x829A, 0x829D, 0x8822, 0x8827, 0x8830, 0x8832, 0x9000, 0x9003, 0x9004, 0x9010, 0x9011, 0x9012, 0x9204, 0x9207, 0x9209, 0x920A, 0x9290, 0x9291, 0x9292, 0xA402, 0xA403, 0xA405, 0xA406, 0xA432, 0xA433, 0xA434,
];
const T_TAKEN: u16 = 0x9003;
/// None of those values is longer (lens names, a copyright line).
const MAX_SHOT_VALUE: usize = 512;

/// A value of a directory: tag, type, count, and its bytes in big-endian order.
type Value = (u16, u16, u32, Vec<u8>);

/// A big-endian TIFF block of a first directory and, when it has values, an
/// EXIF directory.
fn shot_block(mut first: Vec<Value>, mut exif: Vec<Value>) -> Vec<u8> {
    first.sort_by_key(|v| v.0);
    exif.sort_by_key(|v| v.0);
    let pointer = !exif.is_empty() as usize;
    let exif_at = 8 + 2 + (first.len() + pointer) * 12 + 4;
    let values_at = exif_at + pointer * (2 + exif.len() * 12 + 4);
    let mut values = Vec::new();
    let mut placed = |(tag, typ, count, value): &Value| {
        let mut held = [0u8; 4];
        match value.len() {
            n if n <= 4 => held[..n].copy_from_slice(value),
            _ => {
                held = long(false, (values_at + values.len()) as u32);
                values.extend_from_slice(value);
                values.resize(values.len() + values.len() % 2, 0);
            }
        }
        entry(false, *tag, *typ, *count, held)
    };
    let mut entries: Vec<[u8; 12]> = first.iter().map(&mut placed).collect();
    let exif: Vec<[u8; 12]> = exif.iter().map(&mut placed).collect();
    let mut block = b"MM\0*\0\0\0\x08".to_vec();
    if pointer == 1 {
        // Every tag of the first directory sorts before the pointer.
        entries.push(entry(false, T_EXIF_IFD, LONG, 1, long(false, exif_at as u32)));
    }
    block.extend(directory(false, &entries, 0));
    if pointer == 1 {
        block.extend(directory(false, &exif, 0));
    }
    block.extend_from_slice(&values);
    block
}

/// The block for the picture of `source`: what that file records about the
/// shot, and `orientation`, how the picture is turned. `None` when the file
/// records none of it.
fn shot(source: &Path, orientation: Option<u16>) -> Option<Vec<u8>> {
    let src = Src::open(source).ok()?;
    let p = container::parse(&src);
    let value = |tag: &u16| -> Option<Value> {
        let (tiff, e) = [&p.exif, &p.ifd0].into_iter().flatten().find_map(|(t, i)| i.get(*tag).map(|e| (t, e)))?;
        let mut data = tiff.data(e).filter(|d| (1..=12).contains(&e.typ) && !d.is_empty() && d.len() <= MAX_SHOT_VALUE)?;
        if tiff.le {
            let unit = match e.typ {
                3 | 8 => 2,
                4 | 5 | 9 | 10 | 11 => 4,
                12 => 8,
                _ => 1,
            };
            data.chunks_exact_mut(unit).for_each(|part| part.reverse());
        }
        Some((*tag, e.typ, e.count, data))
    };
    let mut first: Vec<Value> = SHOT_FIRST.iter().filter_map(value).collect();
    let exif: Vec<Value> = SHOT_EXIF.iter().filter_map(value).collect();
    if first.is_empty() && exif.is_empty() {
        return None;
    }
    first.extend(orientation.map(|o| (T_ORIENTATION, 3, 1, o.to_be_bytes().to_vec())));
    Some(shot_block(first, exif))
}

/// Whether a TIFF block says when its picture was taken.
fn dated(block: &[u8]) -> bool {
    let read = || {
        let le = block.get(..2)? == b"II";
        let ifd0 = Dir::read(block, u32_at(block, 4, le)? as usize, le)?;
        let exif = ifd0.find(T_EXIF_IFD, le).and_then(|i| Dir::read(block, u32_at(&ifd0.entries[i], 8, le)? as usize, le));
        Some(ifd0.find(T_TAKEN, le).is_some() || exif.is_some_and(|exif| exif.find(T_TAKEN, le).is_some()))
    };
    read().unwrap_or(false)
}

/// `jpeg`, the picture a RAW file embeds, with what `source` (that file)
/// records about the shot as its EXIF block. Such a picture comes with its
/// orientation at most, and a still made of it would be a photo taken by no
/// camera at no time, which Photos files under the day it was imported. A
/// picture that already says when it was taken is left as it is, and so is
/// one whose source records nothing.
pub fn described(jpeg: Vec<u8>, source: &Path) -> Vec<u8> {
    let (segment, insert) = exif_segment(&jpeg);
    let old = segment.clone().map(|s| &jpeg[s.start + 4 + EXIF_HEADER.len()..s.end]);
    if old.is_some_and(dated) {
        return jpeg;
    }
    match shot(source, old.and_then(orientation)) {
        Some(block) => with_block(&jpeg, &block, segment.unwrap_or(insert..insert)),
        None => jpeg,
    }
}

/// `jpeg` with `block` as its EXIF segment at `place`: over the segment
/// there, or put in where there was none.
fn with_block(jpeg: &[u8], block: &[u8], place: Range<usize>) -> Vec<u8> {
    let mut out = Vec::with_capacity(jpeg.len() + block.len() + 16);
    out.extend_from_slice(&jpeg[..place.start]);
    out.extend_from_slice(&[0xFF, 0xE1]);
    out.extend_from_slice(&((2 + EXIF_HEADER.len() + block.len()) as u16).to_be_bytes());
    out.extend_from_slice(EXIF_HEADER);
    out.extend_from_slice(block);
    out.extend_from_slice(&jpeg[place.end..]);
    out
}

/// The EXIF segment of a JPEG (marker to end), and where one goes when there
/// is none: after the JFIF header that must come first.
fn exif_segment(jpeg: &[u8]) -> (Option<Range<usize>>, usize) {
    let (mut at, mut insert) = (2, 2);
    while let Some(&[0xFF, marker]) = jpeg.get(at..at + 2) {
        match marker {
            0xFF => at += 1,
            0x01 | 0xD0..=0xD8 => at += 2,
            0xD9 | 0xDA => break,
            _ => {
                let Some(length) = u16_at(jpeg, at + 2, false).map(|l| l as usize + 2).filter(|l| *l >= 4 && at + l <= jpeg.len()) else { break };
                if marker == 0xE1 && jpeg[at + 4..at + length].starts_with(EXIF_HEADER) {
                    return (Some(at..at + length), insert);
                }
                at += length;
                if marker == 0xE0 && insert + length == at {
                    insert = at;
                }
            }
        }
    }
    (None, insert)
}

/// The EXIF block (TIFF) a still of `jpeg` carries, with `identifier` in an
/// Apple maker note, and where in the JPEG its segment goes: over the one
/// there is, or after the header when there is none.
fn block(jpeg: &[u8], identifier: &str) -> Result<(Vec<u8>, Range<usize>)> {
    if !jpeg.starts_with(&[0xFF, 0xD8]) {
        return Err(Error::Malformed);
    }
    let note = maker_note(identifier);
    let (segment, insert) = exif_segment(jpeg);
    Ok(match segment {
        Some(segment) => {
            let old = &jpeg[segment.start + 4 + EXIF_HEADER.len()..segment.end];
            // A block that cannot be read, or has no room left, is replaced by a
            // new one that keeps the orientation.
            let block = amended(old, &note).filter(|block| block.len() <= MAX_BLOCK).unwrap_or_else(|| fresh(orientation(old), &note));
            (block, segment)
        }
        None => (fresh(None, &note), insert..insert),
    })
}

/// What [`tagged`] puts in the JPEG, on its own: the EXIF block of a still
/// that is written as another kind of file (`heic`).
#[cfg(not(any(target_os = "ios", target_os = "android")))]
pub fn exif(jpeg: &[u8], identifier: &str) -> Result<Vec<u8>> {
    block(jpeg, identifier).map(|(block, _)| block)
}

/// A copy of `jpeg` that carries `identifier` in an Apple maker note, which
/// makes it the still of the Live Photo whose movie has the same identifier.
/// The picture and everything else the file records are kept as they are.
pub fn tagged(jpeg: &[u8], identifier: &str) -> Result<Vec<u8>> {
    let (block, place) = block(jpeg, identifier)?;
    Ok(with_block(jpeg, &block, place))
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;
    use crate::io::{find, Src};
    use crate::{container, exif};

    const ID: &str = "58813388-F52F-4098-9F81-35A7A9D1CF94";

    /// A small JPEG as an encoder writes it, without EXIF.
    pub fn picture() -> Vec<u8> {
        let img = image::RgbImage::from_fn(16, 16, |x, y| image::Rgb([(x * 16) as u8, (y * 16) as u8, 128]));
        crate::preview::encode_jpeg(&img, 90).unwrap()
    }

    /// `jpeg` with `block` as its EXIF, in front of everything else.
    pub fn with_exif(jpeg: &[u8], block: &[u8]) -> Vec<u8> {
        let mut out = vec![0xFF, 0xD8, 0xFF, 0xE1];
        out.extend_from_slice(&((2 + EXIF_HEADER.len() + block.len()) as u16).to_be_bytes());
        out.extend_from_slice(EXIF_HEADER);
        out.extend_from_slice(block);
        out.extend_from_slice(&jpeg[2..]);
        out
    }

    /// A camera's EXIF block: maker, orientation, capture time, a thumbnail
    /// directory after the first, and `note` as the maker note if any.
    pub fn camera(le: bool, note: Option<&[u8]>) -> Vec<u8> {
        let mut block = if le { b"II*\0".to_vec() } else { b"MM\0*".to_vec() };
        block.extend_from_slice(&long(le, 8));
        let turned = [short(le, 6), [0; 2]].concat().try_into().unwrap();
        // First directory at 8 (3 entries, 42 bytes), EXIF directory at 50, values after both.
        let exif_entries = 1 + note.iter().len() as u32;
        let values = 50 + 2 + exif_entries * 12 + 4;
        let ifd0 = [entry(le, 0x010F, 2, 6, long(le, values)), entry(le, T_ORIENTATION, 3, 1, turned), entry(le, T_EXIF_IFD, LONG, 1, long(le, 50))];
        block.extend(directory(le, &ifd0, values + 26));
        let mut exif = vec![entry(le, 0x9003, 2, 20, long(le, values + 6))];
        if let Some(note) = note {
            exif.push(entry(le, T_MAKERNOTE, UNDEFINED, note.len() as u32, long(le, values + 26 + 18)));
        }
        block.extend(directory(le, &exif, 0));
        block.extend_from_slice(b"Canon\0");
        block.extend_from_slice(b"2024:10:20 07:50:17\0");
        // The thumbnail's directory: one entry, a compression of 6.
        block.extend(directory(le, &[entry(le, 0x0103, 3, 1, [short(le, 6), [0; 2]].concat().try_into().unwrap())], 0));
        block.extend_from_slice(note.unwrap_or_default());
        block
    }

    /// What the reader of this crate finds in a still: maker, orientation,
    /// capture time, maker note.
    fn read(jpeg: &[u8]) -> (Option<String>, u8, Option<String>, Vec<u8>) {
        let src = Src::mem(jpeg.to_vec());
        let p = container::parse(&src);
        let read = exif::read(&p);
        let note = p.maker.and_then(|note| src.read(note.pos, note.len)).unwrap_or_default();
        (read.make, exif::orientation(&p), read.datetime, note)
    }

    /// The JPEG without its EXIF segment.
    fn rest(jpeg: &[u8]) -> Vec<u8> {
        let segment = exif_segment(jpeg).0.unwrap();
        [&jpeg[..segment.start], &jpeg[segment.end..]].concat()
    }

    #[test]
    fn the_note_is_what_imageio_writes() {
        let note = maker_note(ID);
        assert_eq!(note.len(), 69);
        assert_eq!(note[..32], *b"Apple iOS\0\0\x01MM\0\x01\0\x11\0\x02\0\0\0\x25\0\0\0\x20\0\0\0\0");
        assert_eq!(note[32..], *format!("{ID}\0").as_bytes());
    }

    #[test]
    fn a_picture_without_exif_gets_a_block_after_its_header() {
        let plain = picture();
        let out = tagged(&plain, ID).unwrap();
        // JFIF first, then EXIF, then the picture as it was.
        assert_eq!(out[..20], plain[..20]);
        assert_eq!(&out[20..22], [0xFF, 0xE1]);
        assert_eq!(rest(&out), plain);
        assert_eq!(read(&out), (None, 1, None, maker_note(ID)));
        assert!(tagged(b"\x89PNG", ID).is_err());
    }

    #[test]
    fn the_cameras_note_gives_way_and_the_rest_stays() {
        for le in [true, false] {
            let plain = picture();
            // A note longer than Apple's, as cameras write them.
            let nikon = [&b"Nikon\0\x02\x11\0\0"[..], &[0xAB; 190]].concat();
            let before = with_exif(&plain, &camera(le, Some(&nikon)));
            assert_eq!(read(&before).3, nikon);
            let out = tagged(&before, ID).unwrap();
            assert_eq!(out.len(), before.len());
            assert_eq!(rest(&out), rest(&before));
            assert_eq!(read(&out), (Some("Canon".into()), 6, Some("2024-10-20 07:50:17".into()), maker_note(ID)));
            assert!(find(&out, b"Nikon").is_none() && find(&out, &[0xAB; 8]).is_none());
            // Outside the note, only the length its entry gives differs.
            let at = find(&before, &nikon).unwrap();
            assert_eq!((0..out.len()).filter(|i| out[*i] != before[*i] && !(at..at + nikon.len()).contains(i)).count(), 1);

            // A note too short to hold Apple's: blanked, and Apple's added at the end.
            let before = with_exif(&plain, &camera(le, Some(b"tiny maker note")));
            let out = tagged(&before, ID).unwrap();
            assert_eq!(out.len(), before.len() + 1 + 69);
            assert_eq!(read(&out), (Some("Canon".into()), 6, Some("2024-10-20 07:50:17".into()), maker_note(ID)));
            assert!(find(&out, b"tiny").is_none());
            assert_eq!(rest(&out), rest(&before));
        }
    }

    #[test]
    fn a_block_without_a_note_gains_one() {
        for le in [true, false] {
            let plain = picture();
            let before = with_exif(&plain, &camera(le, None));
            let out = tagged(&before, ID).unwrap();
            assert_eq!(read(&out), (Some("Canon".into()), 6, Some("2024-10-20 07:50:17".into()), maker_note(ID)));
            assert_eq!(rest(&out), rest(&before));
            // The thumbnail's directory still follows the first.
            let block = &out[exif_segment(&out).0.unwrap()][10..];
            let first = Dir::read(block, u32_at(block, 4, le).unwrap() as usize, le).unwrap();
            assert_eq!(Dir::read(block, first.next as usize, le).unwrap().find(0x0103, le), Some(0));
        }
        // Nothing but an orientation, as the previews of RAW files have.
        let before = [&[0xFF, 0xD8][..], &crate::jpeg::orientation_app1(8), &picture()[2..]].concat();
        let out = tagged(&before, ID).unwrap();
        assert_eq!(read(&out), (None, 8, None, maker_note(ID)));
        assert_eq!(rest(&out), rest(&before));
        let block = &out[exif_segment(&out).0.unwrap()][10..];
        let first = Dir::read(block, u32_at(block, 4, true).unwrap() as usize, true).unwrap();
        assert_eq!(first.entries.iter().map(|e| u16_at(e, 0, true).unwrap()).collect::<Vec<_>>(), [T_ORIENTATION, T_EXIF_IFD]);
    }

    /// A file holding `block`, as far as the reader goes a RAW file. Each name is used by one test.
    fn raw(name: &str, block: &[u8]) -> std::path::PathBuf {
        let path = std::env::temp_dir().join(format!("sift-still-{}-{name}", std::process::id()));
        std::fs::write(&path, block).unwrap();
        path
    }

    #[test]
    fn the_picture_of_a_raw_file_gets_what_the_file_records() {
        let taken = Some("2024-10-20 07:50:17".to_string());
        for le in [true, false] {
            let source = raw(if le { "le.nef" } else { "be.nef" }, &camera(le, Some(b"the camera's own maker note")));
            // The preview of a RAW file as it comes: turned, and nothing else.
            let preview = [&[0xFF, 0xD8][..], &crate::jpeg::orientation_app1(8), &picture()[2..]].concat();
            let out = described(preview.clone(), &source);
            // Camera and time are the file's, the turn is the picture's own, and the camera's note stays behind.
            assert_eq!(read(&out), (Some("Canon".into()), 8, taken.clone(), vec![]));
            assert_eq!(rest(&out), rest(&preview));
            // A preview that is not turned has no EXIF block at all.
            let out = described(picture(), &source);
            assert_eq!(read(&out), (Some("Canon".into()), 1, taken.clone(), vec![]));
            assert_eq!(rest(&out), picture());
            assert_eq!(read(&tagged(&out, ID).unwrap()), (Some("Canon".into()), 1, taken.clone(), maker_note(ID)));
            // A picture that says when it was taken already is left as it is.
            let dated = with_exif(&picture(), &camera(!le, None));
            assert_eq!(described(dated.clone(), &source), dated);
            std::fs::remove_file(source).unwrap();
        }
        // Nothing to add: no such file, and a file that records nothing.
        assert_eq!(described(picture(), Path::new("/no/such/file.nef")), picture());
        let empty = raw("empty.nef", &picture());
        assert_eq!(described(picture(), &empty), picture());
        std::fs::remove_file(empty).unwrap();
    }

    #[test]
    fn numbers_keep_their_value_whatever_the_byte_order() {
        for le in [true, false] {
            // An EXIF directory with an exposure time (a fraction, stored after it) and a sensitivity (a short number, stored in its entry).
            let mut block = if le { b"II*\0".to_vec() } else { b"MM\0*".to_vec() };
            block.extend_from_slice(&long(le, 8));
            block.extend(directory(le, &[entry(le, T_EXIF_IFD, LONG, 1, long(le, 26))], 0));
            let sensitivity = [short(le, 200), [0; 2]].concat().try_into().unwrap();
            block.extend(directory(le, &[entry(le, 0x829A, 5, 1, long(le, 56)), entry(le, 0x8827, 3, 1, sensitivity)], 0));
            block.extend([long(le, 1), long(le, 250)].concat());
            let source = raw(if le { "le.arw" } else { "be.arw" }, &block);
            let src = Src::mem(described(picture(), &source));
            let read = exif::read(&container::parse(&src));
            assert!((read.shutter.unwrap() - 1.0 / 250.0).abs() < 1e-9);
            assert_eq!(read.iso, Some(200));
            std::fs::remove_file(source).unwrap();
        }
    }

    #[test]
    fn a_block_that_is_full_or_damaged_is_replaced() {
        let plain = picture();
        // A block with no room for another directory: the orientation is kept.
        let mut full = camera(true, None);
        full.resize(MAX_BLOCK - 20, 0);
        let out = tagged(&with_exif(&plain, &full), ID).unwrap();
        assert_eq!(read(&out), (None, 6, None, maker_note(ID)));
        assert_eq!(out.len(), plain.len() + 4 + 6 + fresh(Some(6), &maker_note(ID)).len());
        let out = tagged(&with_exif(&plain, b"II*\0\xFF\xFF\xFF\xFFgarbage"), ID).unwrap();
        assert_eq!(read(&out), (None, 1, None, maker_note(ID)));
        assert_eq!(rest(&out), plain);
    }
}
