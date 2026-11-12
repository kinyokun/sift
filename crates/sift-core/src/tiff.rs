//! Minimal TIFF/EXIF IFD reader over a [`Src`]. `base` is the absolute
//! position of the TIFF header, so the same code walks a RAW file, an EXIF
//! block inside a JPEG, a CR3 CMT box or a vendor MakerNote.

use crate::io::{u16_at, u32_at, Src};

pub const T_ORIENTATION: u16 = 0x0112;
pub const T_SUBIFDS: u16 = 0x014A;
pub const T_XMP: u16 = 0x02BC;
pub const T_EXIF_IFD: u16 = 0x8769;
pub const T_MAKERNOTE: u16 = 0x927C;

/// Values larger than this are never read in one piece (previews are located, not loaded).
const MAX_VALUE: usize = 8 * 1024 * 1024;

#[derive(Clone, Copy)]
pub struct Tiff<'a> {
    pub src: &'a Src,
    pub base: u64,
    pub le: bool,
}

#[derive(Clone, Debug)]
pub struct Entry {
    pub tag: u16,
    pub typ: u16,
    pub count: u32,
    pub raw: [u8; 4],
    /// Absolute position of this 12-byte entry in the source.
    pub pos: u64,
}

#[derive(Clone, Debug, Default)]
pub struct Ifd {
    pub entries: Vec<Entry>,
    pub next: u32,
}

impl Ifd {
    pub fn get(&self, tag: u16) -> Option<&Entry> {
        self.entries.iter().find(|e| e.tag == tag)
    }
}

pub fn type_size(typ: u16) -> usize {
    match typ {
        1 | 2 | 6 | 7 => 1,
        3 | 8 => 2,
        4 | 9 | 11 | 13 => 4,
        5 | 10 | 12 => 8,
        _ => 1,
    }
}

impl<'a> Tiff<'a> {
    /// Opens a TIFF header at `base`. Only the byte-order mark is checked so
    /// that ORF (`IIRO`) and RW2 (`IIU\0`) headers are accepted too.
    pub fn open(src: &'a Src, base: u64) -> Option<(Tiff<'a>, u32)> {
        let h = src.read(base, 8)?;
        let le = match &h[..2] {
            b"II" => true,
            b"MM" => false,
            _ => return None,
        };
        let ifd0 = u32_at(&h, 4, le)?;
        Some((Tiff { src, base, le }, ifd0))
    }

    pub fn ifd(&self, off: u32) -> Option<Ifd> {
        self.ifd_at(self.base + off as u64)
    }

    /// Reads an IFD at an absolute source position.
    pub fn ifd_at(&self, pos: u64) -> Option<Ifd> {
        let n = u16_at(&self.src.read(pos, 2)?, 0, self.le)? as usize;
        if n == 0 || n > 600 {
            return None;
        }
        let tbl = self.src.read(pos + 2, n * 12)?;
        let mut entries = Vec::with_capacity(n);
        for i in 0..n {
            let e = &tbl[i * 12..i * 12 + 12];
            entries.push(Entry {
                tag: u16_at(e, 0, self.le)?,
                typ: u16_at(e, 2, self.le)?,
                count: u32_at(e, 4, self.le)?,
                raw: e[8..12].try_into().ok()?,
                pos: pos + 2 + (i * 12) as u64,
            });
        }
        let next = self
            .src
            .read(pos + 2 + (n * 12) as u64, 4)
            .and_then(|b| u32_at(&b, 0, self.le))
            .unwrap_or(0);
        Some(Ifd { entries, next })
    }

    pub fn byte_len(&self, e: &Entry) -> usize {
        type_size(e.typ).saturating_mul(e.count as usize)
    }

    /// Absolute position of the entry's value bytes.
    pub fn data_pos(&self, e: &Entry) -> u64 {
        if self.byte_len(e) <= 4 {
            e.pos + 8
        } else {
            self.base + u32_at(&e.raw, 0, self.le).unwrap_or(0) as u64
        }
    }

    pub fn data(&self, e: &Entry) -> Option<Vec<u8>> {
        let n = self.byte_len(e);
        if n <= 4 {
            return Some(e.raw[..n].to_vec());
        }
        if n > MAX_VALUE {
            return None;
        }
        self.src.read(self.data_pos(e), n)
    }

    /// Integer values of a BYTE/SHORT/LONG entry.
    pub fn uints(&self, e: &Entry) -> Vec<u32> {
        let Some(d) = self.data(e) else { return Vec::new() };
        match e.typ {
            1 | 7 => d.iter().map(|&b| b as u32).collect(),
            3 | 8 => (0..d.len() / 2).filter_map(|i| u16_at(&d, i * 2, self.le).map(u32::from)).collect(),
            4 | 9 | 13 => (0..d.len() / 4).filter_map(|i| u32_at(&d, i * 4, self.le)).collect(),
            _ => Vec::new(),
        }
    }

    pub fn uint(&self, e: &Entry) -> Option<u32> {
        self.uints(e).first().copied()
    }

    pub fn ascii(&self, e: &Entry) -> Option<String> {
        let d = self.data(e)?;
        let end = d.iter().position(|&b| b == 0).unwrap_or(d.len());
        let s = String::from_utf8_lossy(&d[..end]).trim().to_string();
        (!s.is_empty()).then_some(s)
    }

    /// First value of a RATIONAL/SRATIONAL (or plain integer) entry.
    pub fn number(&self, e: &Entry) -> Option<f64> {
        let d = self.data(e)?;
        match e.typ {
            5 => {
                let (n, den) = (u32_at(&d, 0, self.le)?, u32_at(&d, 4, self.le)?);
                (den != 0).then(|| n as f64 / den as f64)
            }
            10 => {
                let (n, den) = (u32_at(&d, 0, self.le)? as i32, u32_at(&d, 4, self.le)? as i32);
                (den != 0).then(|| n as f64 / den as f64)
            }
            8 => u16_at(&d, 0, self.le).map(|v| v as i16 as f64),
            9 => u32_at(&d, 0, self.le).map(|v| v as i32 as f64),
            1 | 3 | 4 => self.uint(e).map(f64::from),
            _ => None,
        }
    }
}
