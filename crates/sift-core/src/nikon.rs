//! Nikon's own NEF renderer (the NEF/NRW Image SDK, the engine behind
//! NX Studio) as an optional rendering backend. It is the only way to get
//! exactly the colours the camera would have produced for a Picture Control.
//!
//! Nothing of the SDK is in this repository. Official builds ship Nikon's
//! libraries together with `sift-nikon`, a small separate program that drives
//! them; its source is not public, because the SDK's interface is
//! confidential under Nikon's licence (see NOTICE). Builds from this
//! repository alone simply have no such engine and fall back to
//! approximating NP3 looks.
//!
//! [`Engine`] starts that program and talks to it over a loopback socket.
//! Everything said on that socket is defined here: one tab-separated line
//! per request,
//!
//! ```text
//! source\t<key>\t<length>\n<length bytes>      a NEF to develop from memory
//! render\t<max edge>\t<base>\t<47 values>\t<key or ->\t<path>\n
//! quit\n
//! ```
//!
//! and for each `render` either `ok\t<width>\t<height>\t<0|1>\n` followed by
//! width x height RGB bytes (the flag says the picture was developed from
//! the bytes sent as `<key>`), or `err\t<why>\n`.
//!
//! macOS and 64-bit Windows are supported.

use crate::{Error, Result};
use image::RgbImage;
use std::path::{Path, PathBuf};

/// The Picture Control a look starts from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Base {
    /// Whatever the camera recorded in the file.
    AsShot,
    Standard,
    Neutral,
    Vivid,
    Monochrome,
    Portrait,
    Landscape,
    FlexibleColor,
}

impl Base {
    /// The name the helper knows it by.
    fn name(self) -> &'static str {
        match self {
            Base::AsShot => "as-shot",
            Base::Standard => "standard",
            Base::Neutral => "neutral",
            Base::Vivid => "vivid",
            Base::Monochrome => "monochrome",
            Base::Portrait => "portrait",
            Base::Landscape => "landscape",
            Base::FlexibleColor => "flexible-color",
        }
    }
}

const FLEXIBLE_VALUES: usize = 41;

/// How a NEF should be developed. `NaN` in a slider keeps the value recorded
/// by the camera.
#[derive(Clone, Debug)]
pub struct Recipe {
    /// Picture Control to develop with; [`Base::AsShot`] keeps the camera's.
    pub base: Base,
    /// Contrast, brightness, saturation, hue of classic Picture Controls.
    pub classic: [f64; 4],
    /// Sharpening, mid-range sharpening, clarity.
    pub detail: [f64; 3],
    /// Flexible Color: contrast, highlights, shadows, white level, black
    /// level, saturation, colour blender (8 bands of hue/chroma/brightness),
    /// colour grading (highlights, mid-tones, shadows of hue/chroma/
    /// brightness), blending, balance.
    pub flexible: [f64; FLEXIBLE_VALUES],
    /// An NP3 file to develop "as shot": its settings, custom tone curve
    /// included, are written over the ones the camera recorded in an
    /// in-memory copy of the NEF (see [`record_look`]), and the engine is
    /// given that copy. The fields above are the fallback for files that
    /// cannot take it.
    pub recorded: Option<Vec<u8>>,
}

/// What [`Engine::render`] produced.
pub struct Rendered {
    pub image: RgbImage,
    /// The look was developed from [`Recipe::recorded`], so its tone curve
    /// is already in the picture.
    pub recorded: bool,
}

impl Recipe {
    /// A Picture Control with every slider as the camera recorded it.
    pub fn base(base: Base) -> Recipe {
        Recipe { base, classic: [f64::NAN; 4], detail: [f64::NAN; 3], flexible: [0.0; FLEXIBLE_VALUES], recorded: None }
    }

    /// `<base>\t<values>` of a render request.
    #[cfg_attr(not(any(target_os = "macos", windows)), allow(dead_code))]
    fn encode(&self) -> String {
        let values: Vec<String> = self.classic.iter().chain(&self.detail).chain(&self.flexible).map(|v| v.to_string()).collect();
        format!("{}\t{}", self.base.name(), values.join(","))
    }
}

/// True for the files the SDK can develop.
pub fn handles(path: &Path) -> bool {
    matches!(crate::formats::ext_of(path).as_str(), "nef" | "nrw")
}

/// Whether Nikon's engine exists for this platform at all.
pub const SUPPORTED: bool = cfg!(any(target_os = "macos", all(windows, target_arch = "x86_64")));

/// Client for the helper process. One request runs at a time.
pub struct Engine {
    root: PathBuf,
    #[cfg(any(target_os = "macos", windows))]
    helper: std::sync::Mutex<Option<client::Helper>>,
}

impl Engine {
    /// Whether the engine is in place under `root`.
    pub fn installed(root: &Path) -> bool {
        SUPPORTED && root.join(HELPER).is_file()
    }

    /// Where this build keeps the engine, if it ships one: on macOS the
    /// application bundle itself, on Windows the `nikon` folder next to the
    /// executable. `SIFT_NIKON_ROOT` points somewhere else during development.
    pub fn bundled() -> Option<PathBuf> {
        let root = match std::env::var_os("SIFT_NIKON_ROOT") {
            Some(root) => PathBuf::from(root),
            None => {
                let exe = std::env::current_exe().ok()?;
                // <bundle>/Contents/MacOS/<exe>, or <folder>\<exe>.
                if cfg!(target_os = "macos") { exe.ancestors().nth(3)?.to_path_buf() } else { exe.parent()?.join("nikon") }
            }
        };
        Engine::installed(&root).then_some(root)
    }

    pub fn new(root: PathBuf) -> Engine {
        Engine {
            root,
            #[cfg(any(target_os = "macos", windows))]
            helper: std::sync::Mutex::new(None),
        }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Develops `nef` upright, scaled so its long edge is at most `max_edge`
    /// (0: full size).
    pub fn render(&self, nef: &Path, recipe: &Recipe, max_edge: u32) -> Result<Rendered> {
        #[cfg(any(target_os = "macos", windows))]
        {
            let mut slot = self.helper.lock().unwrap();
            // A helper that died (or a first failure) gets one fresh start.
            for attempt in 0..2 {
                if slot.is_none() {
                    *slot = Some(client::Helper::start(&self.root)?);
                }
                match slot.as_mut().unwrap().render(nef, recipe, max_edge) {
                    Err(client::Failure::Broken(e)) if attempt == 0 => {
                        let _ = e;
                        *slot = None;
                    }
                    Err(client::Failure::Broken(e)) => {
                        *slot = None;
                        return Err(Error::Decode(format!("Nikon SDK helper stopped: {e}")));
                    }
                    Err(client::Failure::Refused(e)) => return Err(Error::Decode(format!("Nikon SDK: {e}"))),
                    Ok(img) => return Ok(img),
                }
            }
            unreachable!()
        }
        #[cfg(not(any(target_os = "macos", windows)))]
        {
            let _ = (nef, recipe, max_edge);
            Err(Error::Unsupported("the Nikon SDK is only available on macOS and Windows"))
        }
    }

    /// Starts the helper and lets the SDK initialise; fails if it cannot.
    pub fn check(&self) -> Result<()> {
        #[cfg(any(target_os = "macos", windows))]
        {
            let mut slot = self.helper.lock().unwrap();
            if slot.is_none() {
                *slot = Some(client::Helper::start(&self.root)?);
            }
            Ok(())
        }
        #[cfg(not(any(target_os = "macos", windows)))]
        Err(Error::Unsupported("the Nikon SDK is only available on macOS and Windows"))
    }
}

/// Where a file keeps its Picture Control: positions of the three MakerNote
/// values (see [`record_look`]) and the byte order they are in.
struct Recorded {
    /// PictureControlData (0x0023), at least 100 bytes of version "03".
    control: usize,
    /// The Flexible Color settings (0x005C), on cameras that have them.
    flexible: Option<usize>,
    /// The tone curve (0x008C): position and length.
    curve: (usize, usize),
    le: bool,
}

/// Finds the Picture Control in the bytes of a NEF, or of a JPEG from the
/// camera (whose Exif block is the same TIFF structure).
fn locate(file: &[u8]) -> std::result::Result<Recorded, &'static str> {
    let u16_at = |o: usize, le: bool| file.get(o..o + 2).map(|v| if le { u16::from_le_bytes([v[0], v[1]]) } else { u16::from_be_bytes([v[0], v[1]]) });
    let u32_at = |o: usize, le: bool| file.get(o..o + 4).map(|v| if le { u32::from_le_bytes(v.try_into().unwrap()) } else { u32::from_be_bytes(v.try_into().unwrap()) });
    // (position of the value, its length in bytes) of `tag` in the IFD at `ifd`.
    let find = |tiff: usize, ifd: usize, le: bool, tag: u16| -> Option<(usize, usize)> {
        let n = u16_at(ifd, le)? as usize;
        (0..n).map(|i| ifd + 2 + i * 12).find(|&e| u16_at(e, le) == Some(tag)).and_then(|e| {
            let len = crate::tiff::type_size(u16_at(e + 2, le)?).checked_mul(u32_at(e + 4, le)? as usize)?;
            let at = if len <= 4 { e + 8 } else { tiff.checked_add(u32_at(e + 8, le)? as usize)? };
            (at.checked_add(len)? <= file.len()).then_some((at, len))
        })
    };
    let order = |at: usize| match file.get(at..at + 2) {
        Some(b"II") => Some(true),
        Some(b"MM") => Some(false),
        _ => None,
    };
    // A JPEG keeps the TIFF in its "Exif" segment.
    let tiff = if file.starts_with(&[0xff, 0xd8]) {
        let mut pos = 2;
        loop {
            let (Some(&[0xff, marker]), Some(len)) = (file.get(pos..pos + 2).map(|m| <&[u8; 2]>::try_from(m).unwrap()), u16_at(pos + 2, false)) else { return Err("no Nikon MakerNote") };
            if marker == 0xe1 && file.get(pos + 4..pos + 10) == Some(b"Exif\0\0") {
                break pos + 10;
            }
            if marker == 0xda || len < 2 {
                return Err("no Nikon MakerNote");
            }
            pos += 2 + len as usize;
        }
    } else {
        0
    };
    // IFD0 -> Exif IFD -> MakerNote ("Nikon\0", version, then a TIFF of its own).
    let (note, le) = (|| {
        let le = order(tiff)?;
        let (exif, _) = find(tiff, tiff + u32_at(tiff + 4, le)? as usize, le, crate::tiff::T_EXIF_IFD)?;
        let (note, len) = find(tiff, tiff + u32_at(exif, le)? as usize, le, crate::tiff::T_MAKERNOTE)?;
        (len > 18 && file[note..].starts_with(b"Nikon\0")).then_some(())?;
        Some((note + 10, order(note + 10)?))
    })()
    .ok_or("no Nikon MakerNote")?;
    let ifd = note + u32_at(note + 4, le).ok_or("no Nikon MakerNote")? as usize;
    let value = |tag: u16, min: usize| find(note, ifd, le, tag).filter(|v| v.1 >= min);
    let (control, _) = value(0x0023, 100).filter(|v| file[v.0..].starts_with(b"03")).ok_or("the camera's Picture Control data is of another kind")?;
    let curve = value(0x008c, 2).ok_or("no tone curve in the file")?;
    Ok(Recorded { control, flexible: value(0x005c, 72).map(|v| v.0), curve, le })
}

/// The Picture Control recorded in `file` (the bytes of a NEF or of a JPEG
/// from the camera) as an NP3 file: the inverse of [`record_look`], so a
/// recipe that only exists in a photo can be used on other photos.
pub fn recorded_look(file: &[u8]) -> Result<Vec<u8>> {
    let at = locate(file).map_err(|what| Error::Decode(format!("cannot read the Picture Control: {what}")))?;
    let c = &file[at.control..at.control + 100];
    let mut out = b"NCP\0".to_vec();
    let mut rec = |tag: u32, value: &[u8]| {
        out.extend_from_slice(&tag.to_be_bytes());
        out.extend_from_slice(&(value.len() as u32).to_be_bytes());
        out.extend_from_slice(value);
    };
    rec(0x100, &c[4..8]);
    rec(0x200, &c[8..28]);
    rec(0x300, &if at.le { [c[49], c[48]] } else { [c[48], c[49]] });
    rec(0x400, &[0, 0]);
    rec(0x500, &c[53..55]);
    rec(0x600, &c[57..59]);
    rec(0x700, &c[61..63]);
    for (i, tag) in [0x800, 0x900, 0xa00, 0xb00].into_iter().enumerate() {
        rec(tag, &c[63 + i * 2..65 + i * 2]);
    }
    rec(0xc00, &[c[71], 0]);
    rec(0xd00, &[c[72], 0]);
    rec(0xe00, &c[73..75]);
    for (i, tag) in [0xf00, 0x1000, 0x1100, 0x1200, 0x1300].into_iter().enumerate() {
        rec(tag, &c[90 + i * 2..92 + i * 2]);
    }
    rec(0x1400, &[0xff, 1]);
    rec(0x1500, &c[55..57]);
    rec(0x1600, &c[59..61]);
    rec(0x1700, &c[75..77]);
    rec(0x1800, &c[77..79]);
    // Cameras without Flexible Color leave these out; an NP3 has them "not set".
    let mut unset = [0xffu8; 72];
    for pair in unset[12..24].chunks_mut(2) {
        pair[1] = 1;
    }
    unset[48..52].copy_from_slice(&[1, 1, 1, 0]);
    unset[64..72].copy_from_slice(&[1, 1, 1, 0, 0xff, 1, 0xff, 1]);
    let f = at.flexible.map_or(&unset[..], |f| &file[f..f + 72]);
    for (i, tag) in [0x1900, 0x1a00, 0x1b00, 0x1c00, 0x1d00, 0x1e00].into_iter().enumerate() {
        rec(tag, &f[12 + i * 2..14 + i * 2]);
    }
    rec(0x1f00, &f[24..52]);
    let mut grading: [u8; 20] = f[52..72].try_into().unwrap();
    if at.le && at.flexible.is_some() {
        for zone in 0..3 {
            grading.swap(zone * 4, zone * 4 + 1);
        }
    }
    rec(0x2000, &grading);
    let curve = &file[at.curve.0..at.curve.0 + at.curve.1];
    if curve.len() > 2 && curve[2..].iter().any(|&b| b != 0) {
        rec(2, curve);
    }
    out.extend_from_slice(&[0; 4]);
    Ok(out)
}

/// Rewrites the Picture Control recorded in `nef` (the bytes of a NEF file,
/// never the file itself) with the one in the NP3 file `np3`, so that
/// developing it "as shot" gives that look, custom tone curve included.
///
/// The camera keeps a Picture Control in three MakerNote values whose bytes
/// are the NP3 records one after another: PictureControlData (0x0023),
/// the Flexible Color settings (0x005C) and the tone curve (0x008C). They
/// have fixed sizes, so nothing in the file moves. Layout checked against
/// Z f files shot with custom Flexible Color recipes: recording a recipe in
/// a file shot with another gives the same picture as handing the SDK its
/// settings, and recording a file's own recipe changes nothing.
pub fn record_look(nef: &mut [u8], np3: &[u8]) -> Result<()> {
    let bad = |what: &str| Error::Decode(format!("cannot record a Picture Control: {what}"));
    if np3.len() < 12 || !np3.starts_with(b"NCP") {
        return Err(bad("not an NP3 file"));
    }
    // NP3: (tag u32, length u32, value) records, big-endian, after "NCP\0".
    let mut records = std::collections::HashMap::new();
    let mut pos = 4;
    while pos + 8 <= np3.len() {
        let word = |o: usize| u32::from_be_bytes(np3[o..o + 4].try_into().unwrap());
        let (tag, len) = (word(pos), word(pos + 4) as usize);
        if tag == 0 || pos + 8 + len > np3.len() {
            break;
        }
        records.insert(tag, &np3[pos + 8..pos + 8 + len]);
        pos += 8 + len;
    }
    let base = records.get(&0x300).filter(|v| v.len() >= 2).map(|v| u16::from_be_bytes([v[0], v[1]])).ok_or_else(|| bad("no base Picture Control"))?;
    let base_name: &[u8] = match base {
        0x0020 => b"FLEXIBLE COLOR",
        0x0001 => b"STANDARD",
        0x03c2 => b"NEUTRAL",
        0x00c3 => b"VIVID",
        0x064d => b"MONOCHROME",
        0x0486 => b"PORTRAIT",
        0x04c7 => b"LANDSCAPE",
        _ => return Err(bad("unknown base Picture Control")),
    };

    let at = locate(nef).map_err(bad)?;
    let (control, flexible, (curve, curve_len), le) = (at.control, at.flexible, at.curve, at.le);
    let new_curve = records.get(&2).copied();
    if flexible.is_none() && base == 0x0020 {
        return Err(bad("the camera has no Flexible Color"));
    }
    if new_curve.is_some_and(|c| c.len() != curve_len) {
        return Err(bad("the tone curve has another size"));
    }

    // A record's bytes, or the camera's "not set" when the NP3 leaves it out.
    let mut put = |at: usize, tag: u32, len: usize, unset: &[u8]| {
        let v = records.get(&tag).filter(|v| v.len() >= len).map_or(unset, |v| &v[..len]);
        nef[at..at + len].copy_from_slice(&v[..len]);
    };
    let c = control;
    put(c + 8, 0x200, 20, &[0; 20]);
    put(c + 53, 0x500, 2, &[0xff, 1]);
    put(c + 55, 0x1500, 2, &[0xff, 10]);
    for (i, tag) in [0x600, 0x1600, 0x700, 0x800, 0x900, 0xa00, 0xb00].into_iter().enumerate() {
        put(c + 57 + i * 2, tag, 2, &[0xff, 4]);
    }
    put(c + 71, 0xc00, 1, &[0xff]);
    put(c + 72, 0xd00, 1, &[0xff]);
    put(c + 73, 0xe00, 2, &[0xff, 4]);
    put(c + 75, 0x1700, 2, &[0xff, 4]);
    put(c + 77, 0x1800, 2, &[0xff, 4]);
    for (i, tag) in [0xf00, 0x1000, 0x1100, 0x1200, 0x1300].into_iter().enumerate() {
        put(c + 90 + i * 2, tag, 2, &[0xff, 1]);
    }
    if let Some(f) = flexible {
        for (i, tag) in [0x1900, 0x1a00, 0x1b00, 0x1c00, 0x1d00, 0x1e00].into_iter().enumerate() {
            put(f + 12 + i * 2, tag, 2, &[0xff, 1]);
        }
        let mut blender = [0xff; 28];
        blender[24..].copy_from_slice(&[1, 1, 1, 0]);
        put(f + 24, 0x1f00, 28, &blender);
        let mut grading = [0xff; 20];
        grading[12..].copy_from_slice(&[1, 1, 1, 0, 0xff, 1, 0xff, 1]);
        put(f + 52, 0x2000, 20, &grading);
        if le {
            // The three grading hues are 16-bit numbers in the file's byte order.
            for zone in 0..3 {
                nef.swap(f + 52 + zone * 4, f + 53 + zone * 4);
            }
        }
    }
    nef[c + 28..c + 48].fill(0);
    nef[c + 28..c + 28 + base_name.len()].copy_from_slice(base_name);
    let id = if le { base.to_le_bytes() } else { base.to_be_bytes() };
    nef[c + 48..c + 50].copy_from_slice(&id);
    // Both bytes are 1 in files shot with a Flexible Color recipe, 0 with a built-in control.
    nef[c + 50] = 1;
    nef[c + 89] = (base == 0x0020) as u8;
    match new_curve {
        // The SDK develops from the control points at the start of the record
        // and ignores the 257 samples after them.
        Some(v) => nef[curve..curve + curve_len].copy_from_slice(v),
        None => nef[curve + 2..curve + curve_len].fill(0),
    }
    Ok(())
}

/// The helper program, relative to the root (see [`Engine::bundled`]). The
/// SDK looks for its own files around the program that loads it, so on macOS
/// it sits where a bundle's executables do.
#[cfg(target_os = "macos")]
const HELPER: &str = "Contents/MacOS/sift-nikon";
#[cfg(windows)]
const HELPER: &str = "sift-nikon.exe";
#[cfg(not(any(target_os = "macos", windows)))]
const HELPER: &str = "sift-nikon";

#[cfg(any(target_os = "macos", windows))]
mod client {
    use super::*;
    use std::io::{BufRead, BufReader, Read, Write};
    use std::net::{TcpListener, TcpStream};
    use std::process::{Child, Command, Stdio};
    use std::time::{Duration, Instant};

    pub enum Failure {
        /// The helper is gone or out of step; restart it.
        Broken(String),
        /// The SDK answered with an error for this request.
        Refused(String),
    }

    pub struct Helper {
        child: Child,
        out: TcpStream,
        answers: BufReader<TcpStream>,
        /// The file last read, so that trying one look after another on a
        /// photo reads it (often from a network share) only once.
        file: Option<Source>,
        /// Key of the bytes the helper holds.
        sent: Option<u64>,
    }

    struct Source {
        path: PathBuf,
        stamp: Option<(u64, std::time::SystemTime)>,
        bytes: Vec<u8>,
    }

    /// How long the helper may take to connect, and then to load the SDK.
    const CONNECT: Duration = Duration::from_secs(20);
    const START: Duration = Duration::from_secs(120);

    impl Helper {
        pub fn start(root: &Path) -> Result<Helper> {
            let exe = root.join(HELPER);
            // The helper connects back to this socket and proves it is ours
            // with a token only it was told.
            let listener = TcpListener::bind(("127.0.0.1", 0))?;
            let token = {
                use std::hash::{BuildHasher, Hasher};
                let word = || std::collections::hash_map::RandomState::new().build_hasher().finish();
                format!("{:016x}{:016x}", word(), word())
            };
            let mut command = Command::new(&exe);
            command.arg(root).arg(listener.local_addr()?.port().to_string()).arg(&token);
            // The SDK prints to stdout; nobody reads it.
            command.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
            #[cfg(windows)]
            {
                use std::os::windows::process::CommandExt;
                const CREATE_NO_WINDOW: u32 = 0x0800_0000;
                command.creation_flags(CREATE_NO_WINDOW);
            }
            let mut child = command.spawn()?;
            let started = (|| -> std::result::Result<(TcpStream, BufReader<TcpStream>), String> {
                listener.set_nonblocking(true).map_err(|e| e.to_string())?;
                let begun = Instant::now();
                let (out, mut answers) = loop {
                    match listener.accept() {
                        Ok((stream, _)) => {
                            stream.set_nonblocking(false).map_err(|e| e.to_string())?;
                            stream.set_read_timeout(Some(START)).map_err(|e| e.to_string())?;
                            let mut answers = BufReader::new(stream.try_clone().map_err(|e| e.to_string())?);
                            let mut hello = String::new();
                            // Anything else on this machine that found the port is turned away.
                            if answers.read_line(&mut hello).is_ok() && hello.trim_end() == token {
                                break (stream, answers);
                            }
                        }
                        Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                            if child.try_wait().ok().flatten().is_some() {
                                return Err("the helper exited at once".into());
                            }
                            if begun.elapsed() > CONNECT {
                                return Err("the helper did not answer".into());
                            }
                            std::thread::sleep(Duration::from_millis(15));
                        }
                        Err(e) => return Err(e.to_string()),
                    }
                };
                let mut line = String::new();
                answers.read_line(&mut line).map_err(|e| e.to_string())?;
                if line.trim() != "ready" {
                    return Err(line.trim().strip_prefix("err\t").unwrap_or("no answer").to_string());
                }
                // A full-size render takes as long as it takes.
                out.set_read_timeout(None).map_err(|e| e.to_string())?;
                let _ = out.set_nodelay(true);
                Ok((out, answers))
            })();
            match started {
                Ok((out, answers)) => Ok(Helper { child, out, answers, file: None, sent: None }),
                Err(why) => {
                    let _ = child.kill();
                    let _ = child.wait();
                    Err(Error::Decode(format!("Nikon SDK could not start: {why}")))
                }
            }
        }

        pub fn render(&mut self, nef: &Path, recipe: &Recipe, max_edge: u32) -> std::result::Result<Rendered, Failure> {
            let broken = |e: std::io::Error| Failure::Broken(e.to_string());
            let path = nef.to_str().ok_or_else(|| Failure::Refused("file name is not valid UTF-8".into()))?;
            let key = match &recipe.recorded {
                Some(np3) => self.send_recorded(nef, np3).map_err(broken)?,
                None => None,
            };
            let key = key.map_or("-".to_string(), |k| format!("{k:016x}"));
            self.out.write_all(format!("render\t{max_edge}\t{}\t{key}\t{path}\n", recipe.encode()).as_bytes()).map_err(broken)?;
            let mut line = String::new();
            if self.answers.read_line(&mut line).map_err(broken)? == 0 {
                return Err(Failure::Broken("no answer".into()));
            }
            let mut parts = line.trim_end().split('\t');
            match parts.next() {
                Some("ok") => {
                    let size = (parts.next().and_then(|s| s.parse::<u32>().ok()), parts.next().and_then(|s| s.parse::<u32>().ok()));
                    let (Some(w), Some(h)) = size else { return Err(Failure::Broken("bad answer".into())) };
                    let recorded = parts.next() == Some("1");
                    let mut data = vec![0u8; w as usize * h as usize * 3];
                    self.answers.read_exact(&mut data).map_err(broken)?;
                    let image = RgbImage::from_raw(w, h, data).ok_or_else(|| Failure::Broken("bad image size".into()))?;
                    Ok(Rendered { image, recorded })
                }
                Some("err") => Err(Failure::Refused(parts.next().unwrap_or("unknown error").to_string())),
                _ => Err(Failure::Broken("bad answer".into())),
            }
        }
    }

    impl Helper {
        /// Hands the helper a copy of `nef` with the look `np3` recorded in
        /// it, unless it already has that copy. `None` when the file cannot
        /// take the look (or be read): it is then developed from the
        /// recipe's settings.
        fn send_recorded(&mut self, nef: &Path, np3: &[u8]) -> std::io::Result<Option<u64>> {
            use std::hash::{Hash, Hasher};
            let stamp = std::fs::metadata(nef).ok().map(|m| (m.len(), m.modified().unwrap_or(std::time::UNIX_EPOCH)));
            let mut hasher = std::collections::hash_map::DefaultHasher::new();
            (nef, stamp, np3).hash(&mut hasher);
            let key = hasher.finish();
            if self.sent == Some(key) {
                return Ok(Some(key));
            }
            if !self.file.as_ref().is_some_and(|f| f.path == nef && f.stamp == stamp) {
                self.file = None;
                let Ok(bytes) = std::fs::read(nef) else { return Ok(None) };
                self.file = Some(Source { path: nef.to_path_buf(), stamp, bytes });
            }
            let mut copy = self.file.as_ref().unwrap().bytes.clone();
            if record_look(&mut copy, np3).is_err() {
                return Ok(None);
            }
            self.out.write_all(format!("source\t{key:016x}\t{}\n", copy.len()).as_bytes())?;
            self.out.write_all(&copy)?;
            self.sent = Some(key);
            Ok(Some(key))
        }
    }

    impl Drop for Helper {
        fn drop(&mut self) {
            let _ = self.out.write_all(b"quit\n");
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A little-endian NEF skeleton: IFD0 -> Exif IFD -> Nikon MakerNote with
    /// the three Picture Control values, as an Auto shot records them.
    fn nef(flexible: bool) -> Vec<u8> {
        fn entry(tag: u16, typ: u16, count: u32, value: u32) -> Vec<u8> {
            [&tag.to_le_bytes()[..], &typ.to_le_bytes(), &count.to_le_bytes(), &value.to_le_bytes()].concat()
        }
        let mut control = b"03100310AUTO".to_vec();
        control.resize(108, 0);
        control[48..50].copy_from_slice(&[0xcf, 0x02]);
        let mut settings = b"010003100310".to_vec();
        settings.resize(88, 0xff);
        let mut curve = b"I0".to_vec();
        curve.resize(578, 0);
        // The MakerNote's own TIFF: header (8), IFD (2 + 3 * 12 + 4), then the values.
        let values = 8 + 2 + 36 + 4;
        let mut note = b"II*\0\x08\0\0\0\x03\0".to_vec();
        note.extend(entry(0x0023, 7, 108, values));
        note.extend(entry(if flexible { 0x005c } else { 0x005d }, 7, 88, values + 108));
        note.extend(entry(0x008c, 7, 578, values + 108 + 88));
        note.extend([0; 4]);
        note.extend([control, settings, curve].concat());
        let note = [&b"Nikon\0\x02\x11\0\0"[..], &note].concat();
        let mut file = b"II*\0\x08\0\0\0\x01\0".to_vec();
        file.extend(entry(0x8769, 4, 1, 26));
        file.extend([0; 4]);
        file.extend(1u16.to_le_bytes());
        file.extend(entry(0x927c, 7, note.len() as u32, 44));
        file.extend([0; 4]);
        assert_eq!(file.len(), 44);
        file.extend(note);
        file
    }

    fn rec(tag: u32, value: &[u8]) -> Vec<u8> {
        [&tag.to_be_bytes()[..], &(value.len() as u32).to_be_bytes(), value].concat()
    }

    #[test]
    fn a_look_is_recorded_where_the_camera_keeps_it() {
        let mut name = b"Film".to_vec();
        name.resize(20, 0);
        let mut curve = b"I0\0\xfe\x01\xfc\0\x50\x02\0\x10\xff\xf0".to_vec();
        curve.resize(578, 7);
        let grading = [0x80, 0xf9, 0x86, 0x64, 0x81, 0x5c, 0x85, 0x98, 0x80, 0xf3, 0x88, 0x7a, 1, 1, 1, 0, 0x86, 1, 0x8b, 1];
        let np3 = [b"NCP\0".to_vec(), rec(0x100, b"0310"), rec(0x200, &name), rec(0x300, &[0, 0x20]), rec(0x600, &[0x90, 4]), rec(0x1900, &[1, 1]), rec(0x1e00, &[0xb8, 1]), rec(0x2000, &grading), rec(2, &curve), vec![0; 4]].concat();
        let (mut file, before) = (nef(true), nef(true));
        record_look(&mut file, &np3).unwrap();
        assert_eq!(file.len(), before.len());
        // Nothing outside the three values changes.
        let at = 44 + 10 + 50;
        assert_eq!(file[..at], before[..at]);
        let (control, settings, tone) = (&file[at..at + 108], &file[at + 108..at + 196], &file[at + 196..]);
        assert_eq!(&control[..8], b"03100310");
        assert_eq!(&control[8..28], &name[..]);
        assert!(control[28..].starts_with(b"FLEXIBLE COLOR\0"));
        assert_eq!(control[48..51], [0x20, 0, 1], "base id in the file's byte order");
        assert_eq!(control[57..61], [0x90, 4, 0xff, 4], "sharpening set, mid-range sharpening not");
        assert_eq!(&settings[..12], b"010003100310");
        assert_eq!(settings[12..24], [1, 1, 0xff, 1, 0xff, 1, 0xff, 1, 0xff, 1, 0xb8, 1]);
        assert_eq!(settings[24..52], [[0xff; 24].as_slice(), &[1, 1, 1, 0]].concat()[..], "no colour blender in the NP3");
        assert_eq!(settings[52..60], [0xf9, 0x80, 0x86, 0x64, 0x5c, 0x81, 0x85, 0x98], "grading hues are swapped to little-endian");
        assert_eq!(settings[68..72], [0x86, 1, 0x8b, 1]);
        assert_eq!(tone, &curve[..]);

        // Without a curve in the NP3, the one in the file is cleared.
        let plain = [b"NCP\0".to_vec(), rec(0x300, &[0, 0xc3]), vec![0; 4]].concat();
        record_look(&mut file, &plain).unwrap();
        assert!(file[at + 28..].starts_with(b"VIVID\0") && file[at + 89] == 0);
        assert!(file[at + 196..].starts_with(b"I0\0") && file[at + 198..].iter().all(|b| *b == 0));

        // Files that cannot take the look are left alone.
        let mut old = nef(false);
        assert!(record_look(&mut old, &np3).is_err());
        assert_eq!(old, nef(false));
        assert!(record_look(&mut file, b"not a look").is_err());
        assert!(record_look(&mut b"II*\0 not a NEF".to_vec(), &np3).is_err());
    }

    #[test]
    fn the_recorded_look_comes_back_as_an_np3() {
        let mut name = b"Film".to_vec();
        name.resize(20, 0);
        let mut curve = b"I0\0\xfe\x01\xfc\0\x50\x02\0\x10\xff\xf0".to_vec();
        curve.resize(578, 7);
        let grading = [0x80, 0xf9, 0x86, 0x64, 0x81, 0x5c, 0x85, 0x98, 0x80, 0xf3, 0x88, 0x7a, 1, 1, 1, 0, 0x86, 1, 0x8b, 1];
        let np3 = [b"NCP\0".to_vec(), rec(0x100, b"0310"), rec(0x200, &name), rec(0x300, &[0, 0x20]), rec(0x600, &[0x90, 4]), rec(0x1e00, &[0xb8, 1]), rec(0x2000, &grading), rec(2, &curve), vec![0; 4]].concat();
        let mut shot = nef(true);
        record_look(&mut shot, &np3).unwrap();
        let back = recorded_look(&shot).unwrap();
        let look = crate::look::parse_np3(&back).unwrap();
        assert_eq!((look.name.as_str(), look.base, look.saturation), ("Film", 0x20, 56.0));
        assert_eq!(look.grading[0].hue, 249.0);
        assert!(look.curve.is_some() && look.detail[0] == Some(4.0));
        // Recording it in another file gives the bytes it was read from.
        let mut other = nef(true);
        record_look(&mut other, &back).unwrap();
        assert_eq!(other, shot);

        // The same values inside a JPEG's Exif segment.
        let jpeg = [&[0xff, 0xd8, 0xff, 0xe0, 0, 4, 0, 0, 0xff, 0xe1][..], &(shot.len() as u16 + 8).to_be_bytes(), b"Exif\0\0", &shot, &[0xff, 0xd9]].concat();
        assert_eq!(recorded_look(&jpeg).unwrap(), back);

        // A look without a curve has no curve record; other files are refused.
        let mut plain = nef(true);
        record_look(&mut plain, &[b"NCP\0".to_vec(), rec(0x300, &[0, 0xc3]), vec![0; 4]].concat()).unwrap();
        assert!(crate::look::parse_np3(&recorded_look(&plain).unwrap()).unwrap().curve.is_none());
        assert!(recorded_look(b"II*\0 not a NEF").is_err() && recorded_look(&[0xff, 0xd8, 0xff, 0xd9]).is_err());
    }

    #[test]
    fn a_recipe_is_one_field_of_names_and_one_of_numbers() {
        let mut recipe = Recipe::base(Base::FlexibleColor);
        recipe.classic[2] = -0.75;
        recipe.flexible[40] = 17.0;
        let line = recipe.encode();
        let (base, values) = line.split_once('\t').unwrap();
        assert_eq!(base, "flexible-color");
        let values: Vec<&str> = values.split(',').collect();
        assert_eq!(values.len(), 7 + FLEXIBLE_VALUES);
        assert_eq!((values[0], values[2], values[47]), ("NaN", "-0.75", "17"));
        assert!(!line.contains('\n') && line.matches('\t').count() == 1);
    }
}
