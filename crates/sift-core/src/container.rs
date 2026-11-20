//! Container detection and structure walk. Produces the locations of the
//! EXIF IFDs, MakerNote, XMP packet and every embedded JPEG without touching
//! sensor data.

use crate::io::{u16_at, u32_at, Src};
use crate::jpeg;
use crate::tiff::{Ifd, Tiff, T_DNG_PRIVATE, T_EXIF_IFD, T_MAKERNOTE, T_SUBIFDS, T_XMP};

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Format {
    Jpeg,
    /// TIFF-based RAW (NEF, ARW, CR2, DNG, PEF, ORF, RW2, ...).
    Tiff,
    Cr3,
    Raf,
    /// HEVC-coded pictures in HEIF (`.heic`, `.hif`).
    Heif,
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
    /// HEIF files only: size and orientation of the picture.
    pub heif: Option<Heif>,
}

impl<'a> Parsed<'a> {
    fn empty(format: Format) -> Self {
        Parsed { format, ifd0: None, exif: None, maker: None, previews: Vec::new(), xmp: None, jpeg: None, heif: None }
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

    /// The part of the sensor a Panasonic RW2 records when it was shot in
    /// another aspect ratio than the sensor's (16:9, 3:2, 1:1 on a 4:3 sensor).
    pub fn recorded_crop(&self) -> Option<RecordedCrop> {
        if self.format != Format::Tiff {
            return None;
        }
        let (t, ifd0) = self.ifd0.as_ref()?;
        // Panasonic's TIFF variant; elsewhere these low tag numbers mean nothing.
        if t.src.read(0, 4)?.as_slice() != b"IIU\0" {
            return None;
        }
        let edges = |tags: [u16; 4]| -> Option<[u32; 4]> {
            let v = tags.map(|tag| ifd0.get(tag).and_then(|e| t.uint(e)));
            Some([v[0]?, v[1]?, v[2]?, v[3]?]).filter(|[l, t, r, b]| r > l && b > t)
        };
        // SensorLeft/Top/Right/BottomBorder and CropLeft/Top/Right/Bottom.
        let sensor = edges([0x0005, 0x0004, 0x0007, 0x0006])?;
        let crop = edges([0x0030, 0x002F, 0x0032, 0x0031])?;
        let inside = crop[0] >= sensor[0] && crop[1] >= sensor[1] && crop[2] <= sensor[2] && crop[3] <= sensor[3];
        (inside && crop != sensor).then_some(RecordedCrop { sensor, crop })
    }
}

/// The picture an RW2 records (`crop`) inside the sensor area (`sensor`),
/// both as left, top, right, bottom in sensor pixels. Its embedded JPEGs and
/// its decoded sensor data show the whole sensor area; the camera's own
/// JPEG, Apple's RAW engine and the AF area only the picture.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RecordedCrop {
    pub sensor: [u32; 4],
    pub crop: [u32; 4],
}

impl RecordedCrop {
    /// Whether an unrotated `w` x `h` image shows the sensor area (the same shape
    /// within 1%), rather than the picture already.
    pub fn fits(&self, w: u32, h: u32) -> bool {
        let [l, t, r, b] = self.sensor;
        let (sensor, image) = ((r - l) as f64 / (b - t) as f64, w as f64 / h.max(1) as f64);
        w > 0 && h > 0 && (sensor / image - 1.0).abs() < 0.01
    }

    /// The picture inside a `w` x `h` image of the sensor area: x, y, width, height.
    pub fn rect(&self, w: u32, h: u32) -> (u32, u32, u32, u32) {
        let [sl, st, sr, sb] = self.sensor;
        let [cl, ct, cr, cb] = self.crop;
        let x = |v: u32| ((v - sl) as f64 * w as f64 / (sr - sl) as f64).round() as u32;
        let y = |v: u32| ((v - st) as f64 * h as f64 / (sb - st) as f64).round() as u32;
        let (x0, y0, x1, y1) = (x(cl), y(ct), x(cr).min(w), y(cb).min(h));
        (x0, y0, x1.saturating_sub(x0).max(1), y1.saturating_sub(y0).max(1))
    }

    /// The picture's long edge as a share of the sensor area's.
    pub fn kept(&self) -> f64 {
        let [sl, st, sr, sb] = self.sensor;
        let [cl, ct, cr, cb] = self.crop;
        (cr - cl).max(cb - ct) as f64 / (sr - sl).max(sb - st) as f64
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
    if head.len() >= 12 && &head[4..8] == b"ftyp" && HEIF_BRANDS.iter().any(|brand| head[8..12] == brand[..]) {
        return parse_heif(src);
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
    // Pentax and Ricoh cameras keep the MakerNote of their own DNGs as DNGPrivateData,
    // whose offsets count from the note itself.
    if p.maker.is_none() {
        if let Some(e) = ifd0.get(T_DNG_PRIVATE) {
            let pos = tiff.data_pos(e);
            if tiff.src.read(pos, 8).is_some_and(|h| h.starts_with(b"PENTAX \0") || h.starts_with(b"RICOH\0")) {
                p.maker = Some(MakerNote { parent: tiff, pos, len: tiff.byte_len(e) });
            }
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
    attach_exif(&mut p, tiff, ifd0.clone());
    // Panasonic RW2 has its MakerNote only in the EXIF block of the embedded JpgFromRaw.
    if p.maker.is_none() {
        if let Some(e) = ifd0.get(0x002E).filter(|e| e.typ == 7 && e.count > 4) {
            p.maker = jpeg_maker(src, tiff.data_pos(e));
        }
    }

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

/// The MakerNote in the EXIF block of the JPEG stream at `offset`.
fn jpeg_maker(src: &Src, offset: u64) -> Option<MakerNote<'_>> {
    let (tiff, off) = Tiff::open(src, jpeg::scan(src, offset)?.exif_base?)?;
    let mut inner = Parsed::empty(Format::Jpeg);
    attach_exif(&mut inner, tiff, tiff.ifd(off)?);
    inner.maker
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

/// The Olympus MakerNote of cameras since about 2005: `OLYMPUS\0` and a byte
/// order mark, or `OM SYSTEM\0\0\0` and one, then an IFD whose offsets count
/// from the start of the note.
pub fn olympus_maker_ifd<'a>(mn: &MakerNote<'a>) -> Option<(Tiff<'a>, Ifd)> {
    let src = mn.parent.src;
    let h = src.read(mn.pos, 16)?;
    let (mark, skip) = if h.starts_with(b"OLYMPUS\0") {
        (8, 12)
    } else if h.starts_with(b"OM SYSTEM\0") {
        (12, 16)
    } else {
        return None;
    };
    let le = match &h[mark..mark + 2] {
        b"II" => true,
        b"MM" => false,
        _ => return None,
    };
    let t = Tiff { src, base: mn.pos, le };
    Some((t, t.ifd(skip)?))
}

/// The Apple MakerNote: `Apple iOS\0`, a version, a byte order mark, then an
/// IFD whose offsets count from the start of the note.
pub fn apple_maker_ifd<'a>(mn: &MakerNote<'a>) -> Option<(Tiff<'a>, Ifd)> {
    let src = mn.parent.src;
    let h = src.read(mn.pos, 14)?;
    if !h.starts_with(b"Apple iOS\0") {
        return None;
    }
    let le = match &h[12..14] {
        b"II" => true,
        b"MM" => false,
        _ => return None,
    };
    let t = Tiff { src, base: mn.pos, le };
    Some((t, t.ifd(14)?))
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

// --------------------------------------------------------------------- HEIF

/// What a HEIF file says about its picture, apart from the EXIF it carries.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Heif {
    /// Pixel size as stored, before the file's own rotation.
    pub width: u32,
    pub height: u32,
    /// The rotation and mirroring the file asks for, as an EXIF orientation
    /// (1-8). Readers of HEIF follow this and not the orientation in the
    /// EXIF block, which cameras write as well and editors forget to update.
    pub orientation: u8,
}

/// Brands of HEVC-coded pictures in HEIF: `.heic` from phones, `.hif` from cameras.
const HEIF_BRANDS: [&[u8; 4]; 10] = [b"heic", b"heix", b"heim", b"heis", b"hevc", b"hevx", b"hevm", b"hevs", b"mif1", b"msf1"];
/// The index of a HEIF file (`meta`) is a few kilobytes; a larger one is not read.
const MAX_HEIF_INDEX: u64 = 8 << 20;

/// A big-endian number of `n` bytes at `at` (zero when it has no bytes).
fn be(d: &[u8], at: usize, n: usize) -> Option<u64> {
    Some(d.get(at..at.checked_add(n)?)?.iter().fold(0, |v, b| v << 8 | *b as u64))
}

/// The boxes in `d`: their kind and where their payload lies in `d`.
fn boxes_in(d: &[u8]) -> Vec<([u8; 4], std::ops::Range<usize>)> {
    let mut out = Vec::new();
    let mut pos = 0;
    while let (Some(size), Some(kind)) = (be(d, pos, 4), d.get(pos + 4..pos + 8)) {
        let (size, header) = match size {
            0 => (d.len() - pos, 8),
            1 => match be(d, pos + 8, 8) {
                Some(size) => (size as usize, 16),
                None => break,
            },
            size => (size as usize, 8),
        };
        if size < header || size > d.len() - pos {
            break;
        }
        out.push((kind.try_into().unwrap(), pos + header..pos + size));
        pos += size;
    }
    out
}

/// The items of a HEIF file (`iinf`): number, type, and for `mime` items the content type.
fn heif_items(d: &[u8]) -> Vec<(u32, [u8; 4], Vec<u8>)> {
    let entries = d.get(if d.first() == Some(&0) { 6 } else { 8 }..).unwrap_or_default();
    boxes_in(entries)
        .into_iter()
        .filter(|(kind, _)| kind == b"infe")
        .filter_map(|(_, r)| {
            let e = &entries[r];
            let (id, at) = match e.first()? {
                2 => (be(e, 4, 2)?, 6),
                3 => (be(e, 4, 4)?, 8),
                _ => return None,
            };
            let kind: [u8; 4] = e.get(at + 2..at + 6)?.try_into().ok()?;
            // Then the item's name and the content type, each ended by a zero byte.
            let content = e.get(at + 6..)?.split(|b| *b == 0).nth(1).unwrap_or_default();
            Some((id as u32, kind, content.to_vec()))
        })
        .collect()
}

/// Where the items stored in one piece are (`iloc`): item, how the position
/// is counted (0 from the start of the file, 1 from the start of `idat`),
/// position, length.
fn heif_places(d: &[u8]) -> Option<Vec<(u32, u8, u64, u64)>> {
    let version = *d.first()?;
    let (sizes, more) = (*d.get(4)?, *d.get(5)?);
    let (offset_size, length_size, base_size) = ((sizes >> 4) as usize, (sizes & 15) as usize, (more >> 4) as usize);
    let index_size = if version > 0 { (more & 15) as usize } else { 0 };
    let wide = if version < 2 { 2 } else { 4 };
    let mut at = 6 + wide;
    let mut out = Vec::new();
    for _ in 0..be(d, 6, wide)? {
        let id = be(d, at, wide)? as u32;
        at += wide;
        let mut method = 0;
        if version > 0 {
            method = (be(d, at, 2)? & 15) as u8;
            at += 2;
        }
        // Skips the data reference: items in another file are not followed.
        let base = be(d, at + 2, base_size)?;
        let extents = be(d, at + 2 + base_size, 2)?;
        at += 4 + base_size;
        for _ in 0..extents {
            at += index_size;
            let (offset, length) = (be(d, at, offset_size)?, be(d, at + offset_size, length_size)?);
            at += offset_size + length_size;
            if extents == 1 {
                out.push((id, method, base.checked_add(offset)?, length));
            }
        }
    }
    Some(out)
}

/// The properties of `item` (`iprp`), in the order the file applies them.
fn heif_properties(iprp: &[u8], item: u32) -> Vec<([u8; 4], &[u8])> {
    let parts = boxes_in(iprp);
    let part = |kind: &[u8; 4]| parts.iter().find(|(k, _)| k == kind).map(|(_, r)| &iprp[r.clone()]);
    let (Some(all), Some(uses)) = (part(b"ipco"), part(b"ipma")) else { return Vec::new() };
    let properties = boxes_in(all);
    let mut out = Vec::new();
    (|| {
        let (id_size, index_size) = (if *uses.first()? == 0 { 2 } else { 4 }, if uses.get(3)? & 1 == 1 { 2 } else { 1 });
        let mut at = 8;
        for _ in 0..be(uses, 4, 4)? {
            let (id, count) = (be(uses, at, id_size)? as u32, be(uses, at + id_size, 1)?);
            at += id_size + 1;
            for _ in 0..count {
                // The top bit says whether a reader must understand the property.
                let index = (be(uses, at, index_size)? & if index_size == 2 { 0x7FFF } else { 0x7F }) as usize;
                at += index_size;
                if let Some((kind, r)) = index.checked_sub(1).and_then(|i| properties.get(i)).filter(|_| id == item) {
                    out.push((*kind, &all[r.clone()]));
                }
            }
        }
        Some(())
    })();
    out
}

/// The EXIF orientation that shows a picture as a HEIF's transforms do:
/// `irot` turns it anticlockwise in quarter turns, `imir` mirrors it, one
/// after the other in the order they are listed.
fn heif_orientation(properties: &[([u8; 4], &[u8])]) -> u8 {
    // Which corner of the stored picture ends up top left, top right, bottom left, bottom right.
    let mut corners = [0u8, 1, 2, 3];
    for (kind, body) in properties {
        match (kind, body.first()) {
            (b"irot", Some(turns)) => {
                for _ in 0..turns & 3 {
                    let [tl, tr, bl, br] = corners;
                    corners = [tr, br, tl, bl];
                }
            }
            (b"imir", Some(axis)) => {
                let [tl, tr, bl, br] = corners;
                // 0 swaps top and bottom, 1 left and right.
                corners = if axis & 1 == 0 { [bl, br, tl, tr] } else { [tr, tl, br, bl] };
            }
            _ => {}
        }
    }
    const EXIF: [[u8; 4]; 8] = [[0, 1, 2, 3], [1, 0, 3, 2], [3, 2, 1, 0], [2, 3, 0, 1], [0, 2, 1, 3], [2, 0, 3, 1], [3, 1, 2, 0], [1, 3, 0, 2]];
    EXIF.iter().position(|c| *c == corners).map_or(1, |i| i as u8 + 1)
}

fn parse_heif(src: &Src) -> Parsed<'_> {
    let mut p = Parsed::empty(Format::Heif);
    let top = boxes(src, 0, src.len());
    let Some(meta) = child(&top, b"meta").filter(|m| (4..=MAX_HEIF_INDEX).contains(&m.body_len)) else { return p };
    // The index starts with four bytes of version and flags.
    let Some(index) = src.read(meta.body + 4, meta.body_len as usize - 4) else { return p };
    let parts = boxes_in(&index);
    let part = |kind: &[u8; 4]| parts.iter().find(|(k, _)| k == kind).map(|(_, r)| &index[r.clone()]);
    let items = part(b"iinf").map(heif_items).unwrap_or_default();
    let places = part(b"iloc").and_then(heif_places).unwrap_or_default();
    let idat = parts.iter().find(|(k, _)| k == b"idat").map(|(_, r)| meta.body + 4 + r.start as u64);
    // Position and length in the file of an item stored in one piece.
    let place = |id: u32| {
        let (_, method, offset, len) = *places.iter().find(|(item, ..)| *item == id)?;
        let pos = match method {
            0 => offset,
            1 => idat?.checked_add(offset)?,
            _ => return None,
        };
        // No length means everything up to the end of the file.
        let len = if len == 0 && method == 0 { src.len().checked_sub(pos)? } else { len };
        (pos.checked_add(len)? <= src.len()).then_some((pos, len))
    };
    let primary = part(b"pitm").and_then(|d| be(d, 4, if *d.first()? == 0 { 2 } else { 4 })).map(|id| id as u32);
    if let Some((id, iprp)) = primary.zip(part(b"iprp")) {
        let properties = heif_properties(iprp, id);
        let orientation = heif_orientation(&properties);
        // The size on screen: what is coded (`ispe`), cropped (`clap`: width and height as fractions) and turned.
        let mut size = properties.iter().find(|(kind, _)| kind == b"ispe").and_then(|(_, d)| be(d, 4, 4).zip(be(d, 8, 4)));
        for (kind, d) in &properties {
            match kind {
                b"clap" => {
                    let side = |at| Some(be(d, at, 4)? / be(d, at + 4, 4).filter(|per| *per > 0)?);
                    size = side(0).zip(side(8)).or(size);
                }
                b"irot" if d.first().is_some_and(|turns| turns & 1 == 1) => size = size.map(|(w, h)| (h, w)),
                _ => {}
            }
        }
        if let Some((w, h)) = size.filter(|(w, h)| *w > 0 && *h > 0 && w.max(h) <= &(u32::MAX as u64)) {
            let (width, height) = if orientation >= 5 { (h as u32, w as u32) } else { (w as u32, h as u32) };
            p.heif = Some(Heif { width, height, orientation });
        }
    }
    for (id, kind, content) in &items {
        let Some((pos, len)) = place(*id) else { continue };
        match kind {
            // Four bytes say how far in the TIFF block starts (after `Exif\0\0` as a rule).
            b"Exif" if p.ifd0.is_none() => {
                let Some(skip) = src.read(pos, 4).and_then(|h| u32_at(&h, 0, false)) else { continue };
                if let Some((tiff, off)) = Tiff::open(src, pos + 4 + skip as u64) {
                    if let Some(ifd0) = tiff.ifd(off) {
                        attach_exif(&mut p, tiff, ifd0);
                    }
                }
            }
            b"mime" if content == b"application/rdf+xml" && p.xmp.is_none() => p.xmp = Some(XmpRegion { pos, len: len as usize }),
            // Cameras put a JPEG of screen size next to the picture.
            b"jpeg" if Some(*id) != primary => push_preview(src, &mut p.previews, pos, len),
            _ => {}
        }
    }
    p.previews.sort_by_key(|j| (j.width as u64 * j.height as u64, j.len));
    p
}

/// Reads the bytes of a MakerNote-relative or TIFF-relative u16 array helper.
pub fn u16s(d: &[u8], le: bool) -> Vec<u16> {
    (0..d.len() / 2).filter_map(|i| u16_at(d, i * 2, le)).collect()
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    fn boxed(kind: &[u8; 4], body: &[u8]) -> Vec<u8> {
        let mut out = (body.len() as u32 + 8).to_be_bytes().to_vec();
        out.extend(kind);
        out.extend(body);
        out
    }

    /// A HEIF file as a camera writes it, without the picture itself: a
    /// primary item with the given transforms, an EXIF block with `note` as
    /// its maker note, and XMP. The picture's place, ahead of both, takes `lead` bytes.
    pub(crate) fn heif(transforms: &[(&[u8; 4], &[u8])], note: &[u8], lead: usize) -> Vec<u8> {
        // Little-endian TIFF: the maker, orientation 6 (which a HEIF reader must not follow), the maker note.
        let mut tiff = b"II\x2A\0\x08\0\0\0\x03\0".to_vec();
        tiff.extend([0x0F, 0x01, 2, 0, 4, 0, 0, 0, b'A', b'c', b'e', 0]);
        tiff.extend([0x12, 0x01, 3, 0, 1, 0, 0, 0, 6, 0, 0, 0]);
        tiff.extend([0x69, 0x87, 4, 0, 1, 0, 0, 0, 50, 0, 0, 0]);
        tiff.extend([0, 0, 0, 0, 1, 0, 0x7C, 0x92, 7, 0]);
        tiff.extend((note.len() as u32).to_le_bytes());
        tiff.extend([68, 0, 0, 0, 0, 0, 0, 0]);
        tiff.extend(note);
        let mut exif = b"\0\0\0\x06Exif\0\0".to_vec();
        exif.extend(&tiff);
        let xmp = b"<x:xmpmeta/>".to_vec();

        let infe = |id: u16, kind: &[u8; 4], content: &[u8]| {
            let mut body = vec![2, 0, 0, 0];
            body.extend(id.to_be_bytes());
            body.extend([0, 0]);
            body.extend(kind);
            body.push(0);
            body.extend(content);
            boxed(b"infe", &body)
        };
        let mut iinf = vec![0, 0, 0, 0, 0, 3];
        iinf.extend(infe(1, b"hvc1", b""));
        iinf.extend(infe(2, b"Exif", b""));
        iinf.extend(infe(3, b"mime", b"application/rdf+xml\0"));

        let mut ipco = boxed(b"ispe", &[0, 0, 0, 0, 0, 0, 0x17, 0x70, 0, 0, 0x0F, 0xA0]);
        transforms.iter().for_each(|(kind, body)| ipco.extend(boxed(kind, body)));
        let mut ipma = vec![0, 0, 0, 0, 0, 0, 0, 1, 0, 1, transforms.len() as u8 + 1];
        ipma.extend((1..=transforms.len() as u8 + 1).map(|i| 0x80 | i));
        let mut iprp = boxed(b"ipco", &ipco);
        iprp.extend(boxed(b"ipma", &ipma));

        // Offsets are filled in once the size of everything before the data is known.
        let index = |exif_at: u32, xmp_at: u32| {
            let mut iloc = vec![1, 0, 0, 0, 0x44, 0x00, 0, 2];
            for (id, at, len) in [(2u16, exif_at, exif.len()), (3, xmp_at, xmp.len())] {
                iloc.extend(id.to_be_bytes());
                iloc.extend([0, 0, 0, 0, 0, 1]);
                iloc.extend(at.to_be_bytes());
                iloc.extend((len as u32).to_be_bytes());
            }
            let mut meta = vec![0, 0, 0, 0];
            meta.extend(boxed(b"pitm", &[0, 0, 0, 0, 0, 1]));
            meta.extend(boxed(b"iinf", &iinf));
            meta.extend(boxed(b"iloc", &iloc));
            meta.extend(boxed(b"iprp", &iprp));
            let mut file = boxed(b"ftyp", b"heic\0\0\0\0mif1heic");
            file.extend(boxed(b"meta", &meta));
            file
        };
        let data_at = (index(0, 0).len() + 8 + lead) as u32;
        let mut file = index(data_at, data_at + exif.len() as u32);
        let mut data = vec![7; lead];
        data.extend(&exif);
        data.extend(&xmp);
        file.extend(boxed(b"mdat", &data));
        file
    }

    #[test]
    fn a_heif_is_read_by_its_index() {
        let src = Src::mem(heif(&[], b"", 0));
        let p = parse(&src);
        assert_eq!(p.format, Format::Heif);
        assert_eq!(p.heif, Some(Heif { width: 6000, height: 4000, orientation: 1 }));
        // The orientation in the EXIF block is not the picture's.
        assert_eq!(crate::exif::orientation(&p), 1);
        assert_eq!(crate::exif::read(&p).make.as_deref(), Some("Ace"));
        let xmp = p.xmp.expect("XMP");
        assert_eq!(src.read(xmp.pos, xmp.len).as_deref(), Some(&b"<x:xmpmeta/>"[..]));
    }

    #[test]
    fn a_heif_says_how_it_is_turned_and_cropped() {
        let of = |transforms: &[(&[u8; 4], &[u8])]| parse(&Src::mem(heif(transforms, b"", 0))).heif.unwrap();
        // A quarter turn anticlockwise is what EXIF calls 8; three of them, 6.
        assert_eq!(of(&[(b"irot", &[1])]).orientation, 8);
        assert_eq!(of(&[(b"irot", &[2])]).orientation, 3);
        assert_eq!(of(&[(b"irot", &[3])]).orientation, 6);
        assert_eq!(of(&[(b"imir", &[1])]).orientation, 2);
        assert_eq!(of(&[(b"imir", &[0])]).orientation, 4);
        assert_eq!(of(&[(b"irot", &[1]), (b"imir", &[1])]).orientation, 7);
        assert_eq!(of(&[(b"imir", &[1]), (b"irot", &[1])]).orientation, 5);
        // The size stays the stored one, whichever way the picture is turned.
        assert_eq!(of(&[(b"irot", &[3])]), Heif { width: 6000, height: 4000, orientation: 6 });
        // A crop to 5996 x 3998 (fractions over 1), before and after the turn.
        let clap = |w: u32, h: u32| [w.to_be_bytes(), 1u32.to_be_bytes(), h.to_be_bytes(), 1u32.to_be_bytes(), [0; 4], [0, 0, 0, 1], [0; 4], [0, 0, 0, 1]].concat();
        assert_eq!(of(&[(b"clap", &clap(5996, 3998)), (b"irot", &[1])]), Heif { width: 5996, height: 3998, orientation: 8 });
        assert_eq!(of(&[(b"irot", &[1]), (b"clap", &clap(3998, 5996))]), Heif { width: 5996, height: 3998, orientation: 8 });
    }

    /// An RW2 header and an IFD0 of SHORT values only.
    fn rw2(entries: &[(u16, u16)]) -> Vec<u8> {
        let mut out = b"IIU\0\x08\0\0\0".to_vec();
        out.extend((entries.len() as u16).to_le_bytes());
        for (tag, v) in entries {
            out.extend(tag.to_le_bytes());
            out.extend([3, 0, 1, 0, 0, 0]);
            out.extend(v.to_le_bytes());
            out.extend([0, 0]);
        }
        out.extend([0; 4]);
        out
    }

    #[test]
    fn an_rw2_shot_in_another_aspect_ratio_says_which_part_it_records() {
        // A DC-GH6 file shot in 16:9: the sensor area, then the picture inside it.
        let gh6 = [(0x04, 8), (0x05, 8), (0x06, 4344), (0x07, 5784), (0x2F, 552), (0x30, 8), (0x31, 3800), (0x32, 5784)];
        let src = Src::mem(rw2(&gh6));
        let c = parse(&src).recorded_crop().expect("crop");
        assert_eq!(c, RecordedCrop { sensor: [8, 8, 5784, 4344], crop: [8, 552, 5784, 3800] });
        // The embedded JPEG (1920 x 1440) and the decoded sensor data show the whole sensor area.
        assert!(c.fits(1920, 1440) && c.fits(5776, 4336));
        assert!(!c.fits(1920, 1080) && !c.fits(1440, 1920));
        assert_eq!(c.rect(5776, 4336), (0, 544, 5776, 3248));
        assert_eq!(c.rect(1920, 1440), (0, 181, 1920, 1078));
        assert_eq!(c.kept(), 1.0);
        // 1:1 keeps the middle of the long edge.
        let square = [(0x04, 8), (0x05, 8), (0x06, 4344), (0x07, 5784), (0x2F, 8), (0x30, 728), (0x31, 4344), (0x32, 5064)];
        let c = parse(&Src::mem(rw2(&square))).recorded_crop().expect("crop");
        assert_eq!(c.rect(5776, 4336), (720, 0, 4336, 4336));
        assert!((c.kept() - 4336.0 / 5776.0).abs() < 1e-9);
        // Shot in the sensor's own shape, or not an RW2 at all.
        let whole = [(0x04, 8), (0x05, 8), (0x06, 4344), (0x07, 5784), (0x2F, 8), (0x30, 8), (0x31, 4344), (0x32, 5784)];
        assert_eq!(parse(&Src::mem(rw2(&whole))).recorded_crop(), None);
        let mut tiff = rw2(&gh6);
        tiff[2] = 0x2A;
        assert_eq!(parse(&Src::mem(tiff)).recorded_crop(), None);
    }
}
