//! The movie half of a Live Photo, put together without the system: the
//! frames of an H.264 movie (MP4 or QuickTime) are copied as they are coded
//! into a QuickTime file that also carries what Photos looks for. That is the
//! content identifier among the movie's metadata (`moov/meta`, key
//! `com.apple.quicktime.content.identifier`) and a second track of timed
//! metadata (`mebx`) whose only sample, `com.apple.quicktime.still-image-time`,
//! sits at the moment the still shows. The boxes are laid out the way
//! AVFoundation writes them (`live.m`).

use super::{index, IDENTIFIER_KEY};
use crate::io::u32_at;
use crate::{Error, Result};
use std::fs::File;
use std::io::{BufWriter, Read, Seek, SeekFrom, Write};
use std::path::Path;

const STILL_TIME_KEY: &[u8] = b"com.apple.quicktime.still-image-time";
/// The sample that marks the still: its length, the number of its key in the
/// track's key table, and an 8-bit value. The value says nothing (the
/// sample's place in time does); iPhones write -1.
const STILL_TIME_SAMPLE: [u8; 9] = [0, 0, 0, 9, 0, 0, 0, 1, 0xFF];
/// BT.709 primaries, transfer function and matrix, as a `colr` box numbers them.
#[cfg(any(windows, test))]
pub const BT709: [u16; 3] = [1, 1, 1];
/// Seconds from 1904, where QuickTime starts counting, to 1970.
const TO_1904: u64 = 2_082_844_800;
const IDENTITY: [u32; 9] = [0x1_0000, 0, 0, 0, 0x1_0000, 0, 0, 0, 0x4000_0000];
/// More frames than any burst has; keeps a damaged table from being believed.
const MAX_SAMPLES: usize = 1 << 20;
/// The fixed part of a video sample description; boxes such as `avcC` follow.
const VISUAL_ENTRY: usize = 8 + 78;
/// Where the media data starts: after `ftyp`, `wide` and the header of `mdat`.
const DATA_START: u64 = 20 + 8 + 8;

fn be32(data: &[u8], at: usize) -> Result<u32> {
    u32_at(data, at, false).ok_or(Error::Malformed)
}

fn be64(data: &[u8], at: usize) -> Result<u64> {
    Ok((be32(data, at)? as u64) << 32 | be32(data, at + 4)? as u64)
}

/// The boxes that follow one another in `data`, as type and payload.
fn boxes(data: &[u8]) -> Vec<([u8; 4], &[u8])> {
    let mut out = Vec::new();
    let mut at = 0;
    while let Some(head) = data.get(at..at + 8) {
        let mut size = u32::from_be_bytes([head[0], head[1], head[2], head[3]]) as u64;
        let mut header = 8;
        if size == 1 {
            let Ok(long) = be64(data, at + 8) else { break };
            (size, header) = (long, 16);
        } else if size == 0 {
            size = (data.len() - at) as u64;
        }
        if size < header || size > (data.len() - at) as u64 {
            break;
        }
        out.push(([head[4], head[5], head[6], head[7]], &data[at + header as usize..at + size as usize]));
        at += size as usize;
    }
    out
}

fn child<'a>(data: &'a [u8], kind: &[u8; 4]) -> Option<&'a [u8]> {
    boxes(data).into_iter().find(|(k, _)| k == kind).map(|(_, body)| body)
}

fn descend<'a>(data: &'a [u8], path: &[&[u8; 4]]) -> Option<&'a [u8]> {
    path.iter().try_fold(data, |inside, kind| child(inside, kind))
}

fn boxed(kind: &[u8; 4], body: &[u8]) -> Vec<u8> {
    let mut out = ((body.len() + 8) as u32).to_be_bytes().to_vec();
    out.extend_from_slice(kind);
    out.extend_from_slice(body);
    out
}

/// A box whose payload starts with a version and flags.
fn full(kind: &[u8; 4], flags: u32, body: &[u8]) -> Vec<u8> {
    boxed(kind, &[&flags.to_be_bytes(), body].concat())
}

fn words(values: &[u32]) -> Vec<u8> {
    values.iter().flat_map(|v| v.to_be_bytes()).collect()
}

/// What is taken over from the video track of the source.
struct Video {
    timescale: u32,
    language: u16,
    /// The end of the track header: how the frames are turned and how large they are shown.
    geometry: Vec<u8>,
    /// The sample description (`stsd`), which holds the decoder's settings.
    description: Vec<u8>,
    /// Frame durations, display order, key frames: copied as they are.
    tables: Vec<([u8; 4], Vec<u8>)>,
    sizes: Vec<u32>,
    /// Where the frames lie in the source, in decoding order, as position and
    /// length; neighbours are joined.
    runs: Vec<(u64, u64)>,
    duration: u32,
    /// The media time the movie starts at (not 0 when frames are reordered).
    start: u32,
}

impl Video {
    fn read(moov: &[u8], length: u64) -> Result<Video> {
        let trak = boxes(moov)
            .into_iter()
            .filter(|(kind, _)| kind == b"trak")
            .map(|(_, body)| body)
            .find(|trak| descend(trak, &[b"mdia", b"hdlr"]).is_some_and(|h| h.get(8..12) == Some(b"vide")))
            .ok_or(Error::Unsupported("the movie has no video track"))?;
        let tkhd = child(trak, b"tkhd").filter(|t| t.len() >= 84).ok_or(Error::Malformed)?;
        let mdhd = descend(trak, &[b"mdia", b"mdhd"]).ok_or(Error::Malformed)?;
        // Version 1 of the header has 64-bit times and duration.
        let wide = mdhd.first() == Some(&1);
        let timescale = be32(mdhd, if wide { 20 } else { 12 })?;
        let language = (be32(mdhd, if wide { 32 } else { 20 })? >> 16) as u16;
        let stbl = descend(trak, &[b"mdia", b"minf", b"stbl"]).ok_or(Error::Malformed)?;
        let table = |kind: &[u8; 4]| child(stbl, kind);

        let description = table(b"stsd").ok_or(Error::Malformed)?;
        if be32(description, 4)? != 1 || !matches!(description.get(12..16), Some(b"avc1" | b"hvc1")) {
            return Err(Error::Unsupported("only an H.264 movie can become a Live Photo"));
        }

        let stsz = table(b"stsz").ok_or(Error::Malformed)?;
        let (uniform, count) = (be32(stsz, 4)?, be32(stsz, 8)? as usize);
        if count == 0 || timescale == 0 {
            return Err(Error::Unsupported("the movie has no frames"));
        }
        if count > MAX_SAMPLES {
            return Err(Error::Malformed);
        }
        let sizes: Vec<u32> = if uniform != 0 { vec![uniform; count] } else { (0..count).map(|i| be32(stsz, 12 + i * 4)).collect::<Result<_>>()? };

        let stsc = table(b"stsc").ok_or(Error::Malformed)?;
        let groups: Vec<(u32, u32)> = (0..be32(stsc, 4)? as usize).map(|i| Ok((be32(stsc, 8 + i * 12)?, be32(stsc, 12 + i * 12)?))).collect::<Result<_>>()?;
        let chunks: Vec<u64> = match (table(b"stco"), table(b"co64")) {
            (Some(stco), _) => (0..be32(stco, 4)? as usize).map(|i| Ok(be32(stco, 8 + i * 4)? as u64)).collect::<Result<_>>()?,
            (None, Some(co64)) => (0..be32(co64, 4)? as usize).map(|i| be64(co64, 8 + i * 8)).collect::<Result<_>>()?,
            (None, None) => return Err(Error::Malformed),
        };
        let mut runs: Vec<(u64, u64)> = Vec::new();
        let mut rest = sizes.iter();
        for (number, &position) in (1u32..).zip(&chunks) {
            let holds = groups.iter().rev().find(|(first, _)| *first <= number).ok_or(Error::Malformed)?.1;
            let mut at = position;
            for &size in rest.by_ref().take(holds as usize) {
                match runs.last_mut() {
                    Some((from, len)) if *from + *len == at => *len += size as u64,
                    _ => runs.push((at, size as u64)),
                }
                at += size as u64;
            }
        }
        if rest.next().is_some() || runs.iter().any(|(from, len)| from.checked_add(*len).is_none_or(|end| end > length)) {
            return Err(Error::Malformed);
        }

        let stts = table(b"stts").ok_or(Error::Malformed)?;
        let (mut timed, mut duration) = (0u64, 0u64);
        for i in 0..be32(stts, 4)? as usize {
            let (frames, each) = (be32(stts, 8 + i * 8)? as u64, be32(stts, 12 + i * 8)? as u64);
            timed += frames;
            duration += frames * each;
        }
        if timed != count as u64 || duration == 0 {
            return Err(Error::Malformed);
        }
        let duration = u32::try_from(duration).map_err(|_| Error::Unsupported("the movie is too long for a Live Photo"))?;

        let tables = [b"stts", b"ctts", b"stss", b"sdtp"].into_iter().filter_map(|kind| Some((*kind, table(kind)?.to_vec()))).collect();
        Ok(Video { timescale, language, geometry: tkhd[tkhd.len() - 44..].to_vec(), description: description.to_vec(), tables, sizes, runs, duration, start: Self::start(trak, stbl) })
    }

    /// Frames that are shown in another order than they are coded carry an
    /// offset each, and the movie starts at the smallest one. An edit list
    /// says so outright.
    fn start(trak: &[u8], stbl: &[u8]) -> u32 {
        let edited = descend(trak, &[b"edts", b"elst"]).and_then(|elst| {
            let wide = elst.first() == Some(&1);
            (0..be32(elst, 4).ok()? as usize).find_map(|i| {
                let from = if wide { be64(elst, 16 + i * 20).ok()? as i64 } else { be32(elst, 12 + i * 12).ok()? as i32 as i64 };
                u32::try_from(from).ok()
            })
        });
        let offset = || {
            let ctts = child(stbl, b"ctts")?;
            (0..be32(ctts, 4).ok()? as usize).filter_map(|i| be32(ctts, 12 + i * 8).ok()).map(|offset| (offset as i32).max(0) as u32).min()
        };
        edited.or_else(offset).unwrap_or(0)
    }

    /// The sample description, naming `colour` when it names none itself.
    fn described(&self, colour: Option<[u16; 3]>) -> Vec<u8> {
        let mut out = self.description.clone();
        let Some(colour) = colour else { return out };
        let Some(extensions) = out.get(8 + VISUAL_ENTRY..) else { return out };
        let present = boxes(extensions);
        if present.iter().any(|(kind, _)| kind == b"colr") {
            return out;
        }
        // After the last box, ahead of the four zero bytes QuickTime may end an entry with.
        let at = 8 + VISUAL_ENTRY + present.iter().map(|(_, body)| body.len() + 8).sum::<usize>();
        let colr = boxed(b"colr", &[&b"nclc"[..], &colour.map(u16::to_be_bytes).concat()].concat());
        let size = u32::from_be_bytes([out[8], out[9], out[10], out[11]]) + colr.len() as u32;
        out[8..12].copy_from_slice(&size.to_be_bytes());
        out.splice(at..at, colr);
        out
    }
}

/// A QuickTime handler box: what a track holds, or where its data is.
fn handler(class: &[u8; 4], kind: &[u8; 4], name: &str) -> Vec<u8> {
    let mut body = [&class[..], kind, &[0u8; 12]].concat();
    body.push(name.len() as u8);
    body.extend_from_slice(name.as_bytes());
    full(b"hdlr", 0, &body)
}

/// The tables of a track whose samples lie one after another from `position` on.
fn placed(count: u32, position: u32) -> Vec<u8> {
    [full(b"stsc", 0, &words(&[1, 1, count, 1])), full(b"stco", 0, &words(&[1, position]))].concat()
}

/// A track's media: `header` is the media information header of its kind and
/// `tables` the content of the sample table.
#[allow(clippy::too_many_arguments)]
fn media(now: u32, timescale: u32, duration: u32, language: u16, kind: &[u8; 4], name: &str, header: Vec<u8>, tables: &[u8]) -> Vec<u8> {
    let mut mdhd = words(&[now, now, timescale, duration]);
    mdhd.extend_from_slice(&language.to_be_bytes());
    mdhd.extend_from_slice(&[0, 0]);
    // The samples are in this file.
    let dinf = boxed(b"dinf", &full(b"dref", 0, &[&1u32.to_be_bytes()[..], &full(b"alis", 1, &[])].concat()));
    let minf = [header, handler(b"dhlr", b"alis", "Data"), dinf, boxed(b"stbl", tables)].concat();
    boxed(b"mdia", &[full(b"mdhd", 0, &mdhd), handler(b"mhlr", kind, name), boxed(b"minf", &minf)].concat())
}

/// A track. `edits` say what plays when, as length (in the movie's time) and
/// media time; a media time of -1 is a gap.
fn track(id: u32, now: u32, duration: u32, geometry: &[u8], edits: &[(u32, i32)], media: &[u8]) -> Vec<u8> {
    let mut tkhd = words(&[now, now, id, 0, duration, 0, 0, 0, 0]);
    tkhd.extend_from_slice(geometry);
    let mut elst = (edits.len() as u32).to_be_bytes().to_vec();
    for &(length, from) in edits {
        elst.extend(words(&[length, from as u32, 0x1_0000]));
    }
    // Flags: enabled, part of the movie, its preview and its poster.
    boxed(b"trak", &[&full(b"tkhd", 0xF, &tkhd)[..], &boxed(b"edts", &full(b"elst", 0, &elst)), media].concat())
}

/// Writes the frames of `source` to `dest` as the movie of a Live Photo with
/// this content identifier, whose still shows the moment `moment` seconds in.
/// `colour` (primaries, transfer function, matrix) is noted for a source that
/// does not say how its colours are coded. Sound is not taken over.
pub fn remux(source: &Path, dest: &Path, identifier: &str, moment: f64, colour: Option<[u16; 3]>) -> Result<()> {
    let mut input = File::open(source)?;
    let length = input.metadata()?.len();
    let moov = index(&mut input)?.ok_or(Error::Unsupported("the movie has no index"))?;
    let video = Video::read(&moov, length)?;

    let frames: u64 = video.sizes.iter().map(|s| *s as u64).sum();
    let mark_position = DATA_START + frames;
    let data = u32::try_from(8 + frames + STILL_TIME_SAMPLE.len() as u64).map_err(|_| Error::Unsupported("the movie is too large for a Live Photo"))?;

    // One clock for the whole movie, the video's, so that every frame keeps its time.
    let (scale, total) = (video.timescale, video.duration);
    let now = (std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs()) + TO_1904) as u32;
    // The mark lasts a thirtieth of a second, as the system's does.
    let mark_length = (scale / 30).clamp(1, total);
    // Rounded down: on a coarse clock the next tick may be the next frame.
    let mark_at = ((moment.max(0.0) * scale as f64) as u64).min((total - mark_length) as u64) as u32;

    let mut tables = full(b"stsd", 0, &video.described(colour)[4..]);
    for (kind, body) in &video.tables {
        tables.extend(boxed(kind, body));
    }
    tables.extend(placed(video.sizes.len() as u32, DATA_START as u32));
    tables.extend(full(b"stsz", 0, &[words(&[0, video.sizes.len() as u32]), words(&video.sizes)].concat()));
    let header = full(b"vmhd", 1, &[0, 0x40, 0x80, 0, 0x80, 0, 0x80, 0]);
    let pictures = media(now, scale, total, video.language, b"vide", "Video", header, &tables);

    // The key table of the metadata track: key 1 is the still's time, a signed byte (type 65).
    let key = boxed(&1u32.to_be_bytes(), &[boxed(b"keyd", &[b"mdta", STILL_TIME_KEY].concat()), boxed(b"dtyp", &words(&[0, 65]))].concat());
    let mebx = boxed(b"mebx", &[&[0u8; 6][..], &1u16.to_be_bytes(), &boxed(b"keys", &key)].concat());
    let tables = [
        full(b"stsd", 0, &[&1u32.to_be_bytes()[..], &mebx].concat()),
        full(b"stts", 0, &words(&[1, 1, mark_length])),
        placed(1, mark_position as u32),
        full(b"stsz", 0, &words(&[STILL_TIME_SAMPLE.len() as u32, 1])),
    ]
    .concat();
    let header = boxed(b"gmhd", &full(b"gmin", 0, &[0, 0x40, 0x80, 0, 0x80, 0, 0x80, 0, 0, 0, 0, 0]));
    let mark = media(now, scale, mark_length, 0x55C4, b"meta", "Metadata", header, &tables);

    let mut mvhd = words(&[now, now, scale, total, 0x1_0000]);
    mvhd.extend_from_slice(&[1, 0]);
    mvhd.extend_from_slice(&[0; 10]);
    mvhd.extend(words(&IDENTITY));
    mvhd.extend_from_slice(&[0; 24]);
    mvhd.extend_from_slice(&3u32.to_be_bytes());
    let mut no_size = words(&IDENTITY);
    no_size.extend_from_slice(&[0; 8]);
    // QuickTime's `meta` box has no version in front, unlike the one in an MP4.
    let meta = [
        full(b"hdlr", 0, &[&[0u8; 4][..], b"mdta", &[0u8; 14]].concat()),
        full(b"keys", 0, &[&1u32.to_be_bytes()[..], &boxed(b"mdta", IDENTIFIER_KEY)].concat()),
        boxed(b"ilst", &boxed(&1u32.to_be_bytes(), &boxed(b"data", &[&words(&[1, 0])[..], identifier.as_bytes()].concat()))),
    ]
    .concat();
    let moov = boxed(
        b"moov",
        &[
            full(b"mvhd", 0, &mvhd),
            track(1, now, total, &video.geometry, &[(total, video.start as i32)], &pictures),
            track(2, now, mark_at + mark_length, &no_size, &[(mark_at, -1), (mark_length, 0)], &mark),
            boxed(b"meta", &meta),
        ]
        .concat(),
    );

    let mut out = BufWriter::with_capacity(1 << 20, File::create(dest)?);
    out.write_all(&boxed(b"ftyp", b"qt  \0\0\0\0qt  "))?;
    out.write_all(&boxed(b"wide", &[]))?;
    out.write_all(&data.to_be_bytes())?;
    out.write_all(b"mdat")?;
    for &(from, len) in &video.runs {
        input.seek(SeekFrom::Start(from))?;
        if std::io::copy(&mut (&mut input).take(len), &mut out)? != len {
            return Err(Error::Malformed);
        }
    }
    out.write_all(&STILL_TIME_SAMPLE)?;
    out.write_all(&moov)?;
    let file = out.into_inner().map_err(|e| e.into_error())?;
    crate::io::sync(&file)?;
    Ok(())
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;

    /// A small MP4 such as an encoder writes: `frames` are the coded frames,
    /// stored in chunks of two with other data between them. With `reordered`
    /// the frames carry display offsets and the index comes first.
    pub fn mp4(frames: &[&[u8]], reordered: bool) -> Vec<u8> {
        let count = frames.len() as u32;
        let mut tkhd = words(&[0, 0, 1, 0, count * 100, 0, 0, 0, 0]);
        tkhd.extend(words(&IDENTITY));
        tkhd.extend(words(&[640 << 16, 480 << 16]));
        let mut entry = vec![0u8; 78];
        entry[7] = 1;
        entry[24..28].copy_from_slice(&[2, 0x80, 1, 0xE0]);
        entry.extend(boxed(b"avcC", &[1, 100, 0, 31, 0xFF, 0xE0, 0]));
        let ftyp = boxed(b"ftyp", b"isom\0\0\x02\0isomiso2avc1mp41");
        let index = |first: u32| {
            let mut at = first;
            let mut chunks = Vec::new();
            for pair in frames.chunks(2) {
                chunks.push(at);
                at += pair.iter().map(|f| f.len() as u32).sum::<u32>() + 5;
            }
            let mut tables = full(b"stsd", 0, &[&1u32.to_be_bytes()[..], &boxed(b"avc1", &entry)].concat());
            tables.extend(full(b"stts", 0, &words(&[1, count, 100])));
            if reordered {
                tables.extend(full(b"ctts", 0, &words(&[2, 1, 200, count - 1, 100])));
            }
            tables.extend(full(b"stss", 0, &words(&[1, 1])));
            tables.extend(full(b"stsc", 0, &words(&[1, 1, 2, 1])));
            tables.extend(full(b"stsz", 0, &[words(&[0, count]), words(&frames.iter().map(|f| f.len() as u32).collect::<Vec<_>>())].concat()));
            if reordered {
                tables.extend(full(b"co64", 0, &[words(&[chunks.len() as u32]), chunks.iter().flat_map(|c| (*c as u64).to_be_bytes()).collect()].concat()));
            } else {
                tables.extend(full(b"stco", 0, &[words(&[chunks.len() as u32]), words(&chunks)].concat()));
            }
            let dinf = boxed(b"dinf", &full(b"dref", 0, &[&1u32.to_be_bytes()[..], &full(b"url ", 1, &[])].concat()));
            let minf = [full(b"vmhd", 1, &[0; 8]), dinf, boxed(b"stbl", &tables)].concat();
            let hdlr = full(b"hdlr", 0, &[&[0u8; 4][..], b"vide", &[0u8; 12], b"VideoHandler\0"].concat());
            let mdia = [full(b"mdhd", 0, &[words(&[0, 0, 1000, count * 100]), vec![0x55, 0xC4, 0, 0]].concat()), hdlr, boxed(b"minf", &minf)].concat();
            let edts = boxed(b"edts", &full(b"elst", 0, &words(&[1, count * 100, 100, 0x1_0000])));
            let trak = [full(b"tkhd", 3, &tkhd), if reordered { edts } else { Vec::new() }, boxed(b"mdia", &mdia)].concat();
            boxed(b"moov", &[full(b"mvhd", 0, &[words(&[0, 0, 1000, count * 100, 0x1_0000]), vec![0; 76], words(&[2])].concat()), boxed(b"trak", &trak)].concat())
        };
        let mut data = Vec::new();
        for pair in frames.chunks(2) {
            data.extend(pair.concat());
            data.extend_from_slice(b"other");
        }
        let mdat = boxed(b"mdat", &data);
        if reordered {
            let first = (ftyp.len() + index(0).len() + 8) as u32;
            [ftyp, index(first), mdat].concat()
        } else {
            let first = ftyp.len() as u32 + 8;
            [ftyp, mdat, index(first)].concat()
        }
    }

    fn folder(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("sift-mov-{name}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    const ID: &str = "58813388-F52F-4098-9F81-35A7A9D1CF94";
    const FRAMES: [&[u8]; 5] = [b"key frame, the largest", b"second", b"third!", b"4th", b"the fifth frame"];

    /// How many frames a movie holds and how many seconds it lasts.
    #[cfg(windows)]
    pub fn length(movie: &Path) -> (usize, f64) {
        let moov = index(&mut File::open(movie).unwrap()).unwrap().unwrap();
        let video = Video::read(&moov, std::fs::metadata(movie).unwrap().len()).unwrap();
        (video.sizes.len(), video.duration as f64 / video.timescale as f64)
    }

    #[test]
    fn the_frames_are_copied_and_the_marks_added() {
        let dir = folder("copy");
        for reordered in [false, true] {
            let (source, dest) = (dir.join("in.mp4"), dir.join("out.mov"));
            std::fs::write(&source, mp4(&FRAMES, reordered)).unwrap();
            remux(&source, &dest, ID, 0.25, Some(BT709)).unwrap();
            let out = std::fs::read(&dest).unwrap();
            assert_eq!(&out[4..12], b"ftypqt  ");
            assert_eq!(super::super::movie_identifier(&dest).unwrap().as_deref(), Some(ID));

            let moov = index(&mut File::open(&dest).unwrap()).unwrap().unwrap();
            let video = Video::read(&moov, out.len() as u64).unwrap();
            // The same frames, byte for byte, in one piece.
            assert_eq!(video.runs, [(DATA_START, FRAMES.concat().len() as u64)]);
            assert_eq!(&out[DATA_START as usize..][..FRAMES.concat().len()], FRAMES.concat());
            assert_eq!(video.sizes, FRAMES.map(|f| f.len() as u32));
            assert_eq!((video.timescale, video.duration, video.start), (1000, 500, if reordered { 100 } else { 0 }));
            assert_eq!(video.tables.iter().any(|(kind, _)| kind == b"ctts"), reordered);
            // The decoder's settings are kept and the colours named.
            let entry = &video.description[8..];
            assert_eq!(be32(entry, 0).unwrap() as usize, entry.len());
            let extensions = boxes(&entry[VISUAL_ENTRY..]);
            assert_eq!(extensions[0], (*b"avcC", &[1, 100, 0, 31, 0xFF, 0xE0, 0][..]));
            assert_eq!(extensions[1], (*b"colr", &b"nclc\0\x01\0\x01\0\x01"[..]));

            // The second track holds one sample, a quarter of a second in.
            let mark = boxes(&moov).into_iter().filter(|(kind, _)| kind == b"trak").nth(1).unwrap().1;
            assert_eq!(&descend(mark, &[b"mdia", b"hdlr"]).unwrap()[4..12], b"mhlrmeta");
            assert_eq!(descend(mark, &[b"edts", b"elst"]).unwrap(), words(&[0, 2, 250, u32::MAX, 0x1_0000, 33, 0, 0x1_0000]));
            let stbl = descend(mark, &[b"mdia", b"minf", b"stbl"]).unwrap();
            let stsd = child(stbl, b"stsd").unwrap();
            assert_eq!(&stsd[12..16], b"mebx");
            assert!(crate::io::find(stsd, &[b"mdta", STILL_TIME_KEY].concat()).is_some());
            let at = be32(child(stbl, b"stco").unwrap(), 8).unwrap() as usize;
            assert_eq!(out[at..at + 9], STILL_TIME_SAMPLE);
            assert_eq!(at + 9, DATA_START as usize - 8 + be32(&out, DATA_START as usize - 8).unwrap() as usize);
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn the_mark_stays_inside_the_movie() {
        let dir = folder("mark");
        let (source, dest) = (dir.join("in.mp4"), dir.join("out.mov"));
        std::fs::write(&source, mp4(&FRAMES, false)).unwrap();
        for (moment, edits) in [(9.0, [467, u32::MAX, 0x1_0000, 33, 0, 0x1_0000]), (-1.0, [0, u32::MAX, 0x1_0000, 33, 0, 0x1_0000])] {
            remux(&source, &dest, ID, moment, None).unwrap();
            let moov = index(&mut File::open(&dest).unwrap()).unwrap().unwrap();
            let mark = boxes(&moov).into_iter().filter(|(kind, _)| kind == b"trak").nth(1).unwrap().1;
            assert_eq!(descend(mark, &[b"edts", b"elst"]).unwrap()[8..], words(&edits));
            // No colour is noted unless asked for.
            let video = Video::read(&moov, u64::MAX).unwrap();
            assert_eq!(boxes(&video.description[8 + VISUAL_ENTRY..]).len(), 1);
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_movie_that_cannot_be_copied_is_refused() {
        let dir = folder("refuse");
        let (source, dest) = (dir.join("in.mp4"), dir.join("out.mov"));
        let whole = mp4(&FRAMES, false);
        let refused = |bytes: &[u8]| {
            std::fs::write(&source, bytes).unwrap();
            remux(&source, &dest, ID, 0.0, None).unwrap_err().to_string()
        };
        assert_eq!(refused(b"not a movie at all"), "the movie has no index");
        // Sound only.
        let at = crate::io::find(&whole, b"vide").unwrap();
        assert_eq!(refused(&[&whole[..at], b"soun", &whole[at + 4..]].concat()), "the movie has no video track");
        // Another codec.
        let at = crate::io::find(&whole, b"avc1\0").unwrap();
        assert_eq!(refused(&[&whole[..at], b"mp4v", &whole[at + 4..]].concat()), "only an H.264 movie can become a Live Photo");
        // Frames that the file does not hold: the table points past its end.
        let at = crate::io::find(&whole, b"stco").unwrap() + 12;
        assert_eq!(refused(&[&whole[..at], &[0x7F], &whole[at + 1..]].concat()), Error::Malformed.to_string());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
