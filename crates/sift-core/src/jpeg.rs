//! JPEG marker-segment walker. Works on a JPEG file or on a JPEG embedded at
//! some offset inside a RAW container.

use crate::io::Src;

pub const XMP_HEADER: &[u8] = b"http://ns.adobe.com/xap/1.0/\0";
pub const EXIF_HEADER: &[u8] = b"Exif\0\0";

#[derive(Clone, Copy, Debug)]
pub struct Segment {
    pub marker: u8,
    /// Absolute position of the 0xFF marker byte.
    pub pos: u64,
    /// Total length including the two marker bytes.
    pub len: u32,
}

#[derive(Clone, Debug, Default)]
pub struct JpegInfo {
    pub width: u32,
    pub height: u32,
    /// SOF marker (0xC0 baseline, 0xC2 progressive, 0xC3 lossless, ...).
    pub sof: u8,
    pub components: u8,
    /// Absolute position of the TIFF header inside the EXIF APP1 segment.
    pub exif_base: Option<u64>,
    /// Absolute position and length of the XMP packet (after the namespace header).
    pub xmp: Option<(u64, usize)>,
    /// Absolute position of the TIFF-style header of the Multi-Picture Format
    /// index (APP2 `MPF`), which lists the previews stored after the main image.
    pub mpf_base: Option<u64>,
    pub segments: Vec<Segment>,
}

/// Walks the header segments of a JPEG starting at `start` until SOS.
pub fn scan(src: &Src, start: u64) -> Option<JpegInfo> {
    if src.read(start, 2)? != [0xFF, 0xD8] {
        return None;
    }
    let mut info = JpegInfo::default();
    let mut pos = start + 2;
    for _ in 0..512 {
        let h = src.read(pos, 4)?;
        if h[0] != 0xFF {
            return None;
        }
        let m = h[1];
        if m == 0xFF {
            pos += 1;
            continue;
        }
        if m == 0xD8 || m == 0x01 || (0xD0..=0xD7).contains(&m) {
            pos += 2;
            continue;
        }
        if m == 0xD9 {
            break;
        }
        let len = u16::from_be_bytes([h[2], h[3]]) as u32;
        if len < 2 {
            return None;
        }
        info.segments.push(Segment { marker: m, pos, len: len + 2 });
        if m == 0xDA {
            break;
        }
        let body = pos + 4;
        match m {
            0xE1 => {
                let head = src.read_upto(body, 32);
                if head.starts_with(EXIF_HEADER) && info.exif_base.is_none() {
                    info.exif_base = Some(body + EXIF_HEADER.len() as u64);
                } else if head.starts_with(XMP_HEADER) && info.xmp.is_none() {
                    info.xmp = Some((
                        body + XMP_HEADER.len() as u64,
                        (len as usize).saturating_sub(2 + XMP_HEADER.len()),
                    ));
                }
            }
            0xE2 => {
                if src.read_upto(body, 4) == b"MPF\0" && info.mpf_base.is_none() {
                    info.mpf_base = Some(body + 4);
                }
            }
            0xC0..=0xCF if m != 0xC4 && m != 0xC8 && m != 0xCC => {
                let d = src.read(body, 6)?;
                info.height = u16::from_be_bytes([d[1], d[2]]) as u32;
                info.width = u16::from_be_bytes([d[3], d[4]]) as u32;
                info.components = d[5];
                info.sof = m;
            }
            _ => {}
        }
        pos += 2 + len as u64;
    }
    (info.width > 0 && info.height > 0).then_some(info)
}

/// True for the JPEG flavours a browser can display (baseline/extended/progressive DCT).
pub fn is_displayable(info: &JpegInfo) -> bool {
    matches!(info.sof, 0xC0..=0xC2) && matches!(info.components, 1 | 3)
}

/// A minimal EXIF APP1 segment carrying only an Orientation tag.
pub fn orientation_app1(orientation: u8) -> Vec<u8> {
    let mut tiff = Vec::with_capacity(26);
    tiff.extend_from_slice(b"II*\0");
    tiff.extend_from_slice(&8u32.to_le_bytes());
    tiff.extend_from_slice(&1u16.to_le_bytes());
    tiff.extend_from_slice(&0x0112u16.to_le_bytes());
    tiff.extend_from_slice(&3u16.to_le_bytes());
    tiff.extend_from_slice(&1u32.to_le_bytes());
    tiff.extend_from_slice(&(orientation as u32).to_le_bytes());
    tiff.extend_from_slice(&0u32.to_le_bytes());
    let mut seg = vec![0xFF, 0xE1];
    seg.extend_from_slice(&((2 + EXIF_HEADER.len() + tiff.len()) as u16).to_be_bytes());
    seg.extend_from_slice(EXIF_HEADER);
    seg.extend_from_slice(&tiff);
    seg
}
