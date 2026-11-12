//! Container detection and structure walk. Produces the locations of the
//! EXIF IFDs, MakerNote, XMP packet and every embedded JPEG without touching
//! sensor data.

use crate::io::{u16_at, u32_at, Src};
use crate::jpeg;
use crate::tiff::{Ifd, Tiff, T_EXIF_IFD, T_MAKERNOTE, T_SUBIFDS, T_XMP};

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Format {
    Jpeg,
    /// TIFF-based RAW (NEF, ARW, CR2, DNG, PEF, ORF, RW2, ...).
    Tiff,
    Cr3,
    Raf,
    /// Anything else the `image` crate can decode (PNG, WebP, BMP, ...).
    Other,
}

#[derive(Clone, Copy, Debug, serde::Serialize)]
pub struct JpegRef {
    pub offset: u64,
    pub len: u64,
    pub width: u32,
    pub height: u32,
}

/// Where the XMP packet lives, which decides how it may be rewritten.
#[derive(Clone, Copy, Debug)]
pub struct XmpRegion {
    pub pos: u64,
    pub len: usize,
}

#[derive(Clone, Copy)]
pub struct MakerNote<'a> {
    /// The TIFF block the MakerNote sits in (offset base for Canon and Sony).
    pub parent: Tiff<'a>,
    /// Absolute position of the first MakerNote byte.
    pub pos: u64,
    pub len: usize,
}

pub struct Parsed<'a> {
    pub format: Format,
    pub ifd0: Option<(Tiff<'a>, Ifd)>,
    pub exif: Option<(Tiff<'a>, Ifd)>,
    pub maker: Option<MakerNote<'a>>,
    /// Displayable embedded JPEGs, smallest first.
    pub previews: Vec<JpegRef>,
    pub xmp: Option<XmpRegion>,
    /// JPEG files only: the segment table, needed to rewrite metadata.
    pub jpeg: Option<jpeg::JpegInfo>,
}

impl<'a> Parsed<'a> {
    fn empty(format: Format) -> Self {
        Parsed { format, ifd0: None, exif: None, maker: None, previews: Vec::new(), xmp: None, jpeg: None }
    }

    pub fn largest_preview(&self) -> Option<JpegRef> {
        self.previews.last().copied()
    }

    /// Smallest preview whose long edge is at least `edge`, else the largest.
    pub fn preview_for(&self, edge: u32) -> Option<JpegRef> {
        self.previews
            .iter()
            .find(|p| p.width.max(p.height) >= edge)
            .or(self.previews.last())
            .copied()
    }
}

thread_local! {
    /// Set while only metadata is wanted, so embedded previews are not probed.
    static SKIP_PREVIEWS: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Like [`parse`], but leaves `previews` empty. Finding and measuring the
/// embedded JPEGs is most of the work; ratings and flags do not need them.
pub fn parse_metadata(src: &Src) -> Parsed<'_> {
    SKIP_PREVIEWS.with(|s| s.set(true));
    let parsed = parse(src);
    SKIP_PREVIEWS.with(|s| s.set(false));
    parsed
}

pub fn parse(src: &Src) -> Parsed<'_> {
    let head = src.read_upto(0, 32);
    if head.starts_with(&[0xFF, 0xD8]) {
        return parse_jpeg(src);
    }
    if head.len() >= 12 && &head[4..8] == b"ftyp" && &head[8..11] == b"crx" {
        return parse_cr3(src);
    }
    if head.starts_with(b"FUJIFILMCCD-RAW") {
        return parse_raf(src);
    }
    if head.starts_with(b"II") || head.starts_with(b"MM") {
        if let Some(p) = parse_tiff(src) {
            return p;
        }
    }
    Parsed::empty(Format::Other)
}

/// Fills `exif` and `maker` from an IFD0 that belongs to `tiff`.
fn attach_exif<'a>(p: &mut Parsed<'a>, tiff: Tiff<'a>, ifd0: Ifd) {
    let exif = ifd0.get(T_EXIF_IFD).and_then(|e| tiff.uint(e)).and_then(|off| tiff.ifd(off));
    if let Some(ex) = &exif {
        if let Some(mn) = ex.get(T_MAKERNOTE) {
            p.maker = Some(MakerNote { parent: tiff, pos: tiff.data_pos(mn), len: tiff.byte_len(mn) });
        }
    }
    if p.maker.is_none() {
        if let Some(mn) = ifd0.get(T_MAKERNOTE) {
            p.maker = Some(MakerNote { parent: tiff, pos: tiff.data_pos(mn), len: tiff.byte_len(mn) });
        }
    }
    p.exif = exif.map(|e| (tiff, e));
    p.ifd0 = Some((tiff, ifd0));
}

fn parse_jpeg(src: &Src) -> Parsed<'_> {
    let mut p = Parsed::empty(Format::Jpeg);
    let Some(info) = jpeg::scan(src, 0) else { return p };
    if let Some(base) = info.exif_base {
        if let Some((tiff, off)) = Tiff::open(src, base) {
            if let Some(ifd0) = tiff.ifd(off) {
                attach_exif(&mut p, tiff, ifd0);
            }
        }
    }
    p.xmp = info.xmp.map(|(pos, len)| XmpRegion { pos, len });
    p.previews.push(JpegRef { offset: 0, len: src.len(), width: info.width, height: info.height });
    if let Some(base) = info.mpf_base {
        mpf_previews(src, base, &mut p.previews);
    }
    p.previews.sort_by_key(|j| (j.width as u64 * j.height as u64, j.len));
    p.jpeg = Some(info);
    p
}

/// Cameras append screen-size previews after the main JPEG and list them in
/// a Multi-Picture Format index (CIPA DC-007). Each MP Entry is 16 bytes:
/// attributes, size, offset from the MPF header, two dependent-image numbers.
fn mpf_previews(src: &Src, base: u64, out: &mut Vec<JpegRef>) -> Option<()> {
    const MP_ENTRY: u16 = 0xB002;
    let (tiff, ifd_off) = Tiff::open(src, base)?;
    let ifd = tiff.ifd(ifd_off)?;
    let entries = tiff.data(ifd.get(MP_ENTRY)?)?;
    for e in entries.as_chunks::<16>().0.iter().take(8) {
        // Only "large thumbnail" entries are previews; phones also store depth
        // and gain maps here, which must not be mistaken for the picture.
        let kind = u32_at(e, 0, tiff.le)? & 0x00FF_FFFF;
        let (len, off) = (u32_at(e, 4, tiff.le)? as u64, u32_at(e, 8, tiff.le)? as u64);
        if matches!(kind, 0x01_0001 | 0x01_0002) && off > 0 {
            push_preview(src, out, base + off, len);
        }
    }
    Some(())
}

fn push_preview(src: &Src, out: &mut Vec<JpegRef>, offset: u64, len: u64) {
    if SKIP_PREVIEWS.with(|s| s.get()) {
        return;
    }
    if len < 512 || offset.checked_add(len).is_none_or(|end| end > src.len()) {
        return;
    }
    if out.iter().any(|p| p.offset == offset) {
        return;
    }
    if let Some(info) = jpeg::scan(src, offset) {
        if jpeg::is_displayable(&info) {
            out.push(JpegRef { offset, len, width: info.width, height: info.height });
        }
    }
}

/// Collects JPEG streams referenced by one IFD.
fn ifd_previews(src: &Src, tiff: &Tiff, ifd: &Ifd, out: &mut Vec<JpegRef>) {
    let val = |tag: u16| ifd.get(tag).and_then(|e| tiff.uint(e));
    if let (Some(off), Some(len)) = (val(0x0201), val(0x0202)) {
        push_preview(src, out, tiff.base + off as u64, len as u64);
    }
    // Single-strip JPEG (CR2 IFD0, DNG preview IFDs). Lossless-JPEG raw strips
    // are filtered out by `is_displayable`.
    if matches!(val(0x0103), Some(6) | Some(7)) {
        let one = |tag: u16| ifd.get(tag).filter(|e| e.count == 1).and_then(|e| tiff.uint(e));
        if let (Some(off), Some(len)) = (one(0x0111), one(0x0117)) {
            push_preview(src, out, tiff.base + off as u64, len as u64);
        }
    }
    // Panasonic RW2 JpgFromRaw.
    if let Some(e) = ifd.get(0x002E).filter(|e| e.typ == 7 && e.count > 4) {
        push_preview(src, out, tiff.data_pos(e), e.count as u64);
    }
}

fn parse_tiff(src: &Src) -> Option<Parsed<'_>> {
    let (tiff, ifd0_off) = Tiff::open(src, 0)?;
    let ifd0 = tiff.ifd(ifd0_off)?;
    let mut p = Parsed::empty(Format::Tiff);

    let mut queue = vec![ifd0_off];
    let mut seen = Vec::new();
    while let Some(off) = queue.pop() {
        if off == 0 || seen.contains(&off) || seen.len() >= 24 {
            continue;
        }
        seen.push(off);
        let Some(ifd) = tiff.ifd(off) else { continue };
        ifd_previews(src, &tiff, &ifd, &mut p.previews);
        if let Some(sub) = ifd.get(T_SUBIFDS) {
            queue.extend(tiff.uints(sub).into_iter().take(8));
        }
        queue.push(ifd.next);
    }

    if let Some(e) = ifd0.get(T_XMP) {
        p.xmp = Some(XmpRegion { pos: tiff.data_pos(e), len: tiff.byte_len(e) });
    }
    attach_exif(&mut p, tiff, ifd0);

    // Nikon keeps a mid-size preview in its MakerNote (PreviewIFD, tag 0x0011).
    if let Some(mn) = p.maker {
        if let Some(inner) = nikon_maker_tiff(src, &mn) {
            if let Some(ifd) = inner.0.ifd(inner.1) {
                if let Some(pv) = ifd.get(0x0011).and_then(|e| inner.0.uint(e)).and_then(|o| inner.0.ifd(o)) {
                    ifd_previews(src, &inner.0, &pv, &mut p.previews);
                }
            }
        }
    }
    p.previews.sort_by_key(|j| (j.width as u64 * j.height as u64, j.len));
    Some(p)
}

/// Nikon type-3 MakerNote: `Nikon\0\x02..` followed by an embedded TIFF header.
pub fn nikon_maker_tiff<'a>(src: &'a Src, mn: &MakerNote<'a>) -> Option<(Tiff<'a>, u32)> {
    let h = src.read(mn.pos, 10)?;
    if !h.starts_with(b"Nikon\0\x02") {
        return None;
    }
    Tiff::open(src, mn.pos + 10)
}

/// Sony MakerNote IFD. JPEGs prefix it with a 12-byte `SONY DSC `/`SONY CAM `
/// header; ARW files start the IFD directly. Offsets are relative to the
/// enclosing TIFF block either way.
pub fn sony_maker_ifd<'a>(p: &Parsed<'a>) -> Option<(Tiff<'a>, Ifd)> {
    let (t0, ifd0) = p.ifd0.as_ref()?;
    let make = ifd0.get(0x010F).and_then(|e| t0.ascii(e))?;
    if !make.to_ascii_uppercase().starts_with("SONY") {
        return None;
    }
    let mn = p.maker.as_ref()?;
    let t = mn.parent;
    let skip = if t.src.read(mn.pos, 4)?.starts_with(b"SONY") { 12 } else { 0 };
    Some((t, t.ifd_at(mn.pos + skip)?))
}

// ---------------------------------------------------------------- ISO BMFF

pub struct BmffBox {
    pub kind: [u8; 4],
    /// Absolute position of the payload (after the size/type header).
    pub body: u64,
    pub body_len: u64,
    pub end: u64,
}

/// Iterates the boxes in `[start, end)` reading only the 8/16-byte headers.
pub fn boxes(src: &Src, start: u64, end: u64) -> Vec<BmffBox> {
    let mut out = Vec::new();
    let end = end.min(src.len());
    let mut pos = start;
    while pos + 8 <= end && out.len() < 256 {
        let Some(h) = src.read(pos, 8) else { break };
        let mut size = u32::from_be_bytes([h[0], h[1], h[2], h[3]]) as u64;
        let mut hdr = 8u64;
        if size == 1 {
            let Some(x) = src.read(pos + 8, 8) else { break };
            size = u64::from_be_bytes(x.try_into().unwrap());
            hdr = 16;
        } else if size == 0 {
            size = end - pos;
        }
        if size < hdr || pos + size > end {
            break;
        }
        out.push(BmffBox { kind: [h[4], h[5], h[6], h[7]], body: pos + hdr, body_len: size - hdr, end: pos + size });
        pos += size;
    }
    out
}

const UUID_CANON: [u8; 16] = [0x85, 0xc0, 0xb6, 0x87, 0x82, 0x0f, 0x11, 0xe0, 0x81, 0x11, 0xf4, 0xce, 0x46, 0x2b, 0x6a, 0x48];
pub const UUID_XMP: [u8; 16] = [0xbe, 0x7a, 0xcf, 0xcb, 0x97, 0xa9, 0x42, 0xe8, 0x9c, 0x71, 0x99, 0x94, 0x91, 0xe3, 0xaf, 0xac];
const UUID_PREVIEW: [u8; 16] = [0xea, 0xf4, 0x2b, 0x5e, 0x1c, 0x98, 0x4b, 0x88, 0xb9, 0xfb, 0xb7, 0xdc, 0x40, 0x6e, 0x4d, 0x16];

fn child<'b>(list: &'b [BmffBox], kind: &[u8; 4]) -> Option<&'b BmffBox> {
    list.iter().find(|b| &b.kind == kind)
}

/// The first CR3 track is a full-resolution JPEG stored in `mdat`.
fn cr3_track_jpeg(src: &Src, trak: &BmffBox, out: &mut Vec<JpegRef>) -> Option<()> {
    let l1 = boxes(src, trak.body, trak.end);
    let mdia = child(&l1, b"mdia")?;
    let l2 = boxes(src, mdia.body, mdia.end);
    let minf = child(&l2, b"minf")?;
    let l3 = boxes(src, minf.body, minf.end);
    let stbl = child(&l3, b"stbl")?;
    let l4 = boxes(src, stbl.body, stbl.end);
    let stsz = child(&l4, b"stsz")?;
    let s = src.read(stsz.body, 12.min(stsz.body_len as usize))?;
    let mut size = u32_at(&s, 4, false)? as u64;
    if size == 0 {
        size = u32_at(&src.read(stsz.body + 12, 4)?, 0, false)? as u64;
    }
    let offset = if let Some(co64) = child(&l4, b"co64") {
        u64::from_be_bytes(src.read(co64.body + 8, 8)?.try_into().ok()?)
    } else {
        u32_at(&src.read(child(&l4, b"stco")?.body + 8, 4)?, 0, false)? as u64
    };
    push_preview(src, out, offset, size);
    Some(())
}

fn parse_cr3(src: &Src) -> Parsed<'_> {
    let mut p = Parsed::empty(Format::Cr3);
    for b in boxes(src, 0, src.len()) {
        match &b.kind {
            b"moov" => {
                for m in boxes(src, b.body, b.end) {
                    if &m.kind == b"trak" {
                        cr3_track_jpeg(src, &m, &mut p.previews);
                    }
                    if &m.kind != b"uuid" || src.read(m.body, 16).as_deref() != Some(&UUID_CANON[..]) {
                        continue;
                    }
                    for c in boxes(src, m.body + 16, m.end) {
                        let block = || Tiff::open(src, c.body).and_then(|(t, off)| t.ifd(off).map(|i| (t, i)));
                        match &c.kind {
                            b"CMT1" => p.ifd0 = block(),
                            b"CMT2" => p.exif = block(),
                            b"CMT3" => {
                                if let Some((t, off)) = Tiff::open(src, c.body) {
                                    p.maker = Some(MakerNote { parent: t, pos: c.body + off as u64, len: c.body_len as usize });
                                }
                            }
                            b"THMB" => {
                                // version 0: jpeg at +16, version 1: at +24
                                if let Some(h) = src.read(c.body, 16) {
                                    let size = u32_at(&h, 8, false).unwrap_or(0) as u64;
                                    let skip = if h[0] == 1 { 24 } else { 16 };
                                    push_preview(src, &mut p.previews, c.body + skip, size);
                                }
                            }
                            _ => {}
                        }
                    }
                }
            }
            b"uuid" => {
                let Some(id) = src.read(b.body, 16) else { continue };
                if id == UUID_XMP {
                    p.xmp = Some(XmpRegion { pos: b.body + 16, len: (b.body_len - 16) as usize });
                } else if id == UUID_PREVIEW {
                    // uuid payload: 8 bytes, then a PRVW box whose JPEG starts 24 bytes in.
                    for c in boxes(src, b.body + 24, b.end) {
                        if &c.kind == b"PRVW" {
                            if let Some(h) = src.read(c.body, 16) {
                                let size = u32_at(&h, 12, false).unwrap_or(0) as u64;
                                push_preview(src, &mut p.previews, c.body + 16, size);
                            }
                        }
                    }
                }
            }
            b"mdat" => break,
            _ => {}
        }
    }
    p.previews.sort_by_key(|j| (j.width as u64 * j.height as u64, j.len));
    p
}

fn parse_raf(src: &Src) -> Parsed<'_> {
    let mut p = Parsed::empty(Format::Raf);
    let Some(h) = src.read(84, 8) else { return p };
    let (off, len) = (u32_at(&h, 0, false).unwrap_or(0) as u64, u32_at(&h, 4, false).unwrap_or(0) as u64);
    push_preview(src, &mut p.previews, off, len);
    if let Some(info) = jpeg::scan(src, off) {
        if let Some((tiff, ifd0_off)) = info.exif_base.and_then(|b| Tiff::open(src, b)) {
            if let Some(ifd0) = tiff.ifd(ifd0_off) {
                // The preview carries the only EXIF block; its IFD1 thumbnail is a preview too.
                if let Some(ifd1) = tiff.ifd(ifd0.next) {
                    ifd_previews(src, &tiff, &ifd1, &mut p.previews);
                }
                attach_exif(&mut p, tiff, ifd0);
            }
        }
    }
    p.previews.sort_by_key(|j| (j.width as u64 * j.height as u64, j.len));
    p
}

/// Reads the bytes of a MakerNote-relative or TIFF-relative u16 array helper.
pub fn u16s(d: &[u8], le: bool) -> Vec<u16> {
    (0..d.len() / 2).filter_map(|i| u16_at(d, i * 2, le)).collect()
}
