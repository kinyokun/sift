//! The system's H.264 encoder on Windows: Media Foundation's sink writer,
//! which is given the frames one by one and writes them as an MP4. Only the
//! encoder that comes with Windows is used, not a graphics card's, so that
//! the movie is the same on every machine.

use crate::{Error, Result};
use std::ffi::c_void;
use std::mem::transmute;
use std::os::windows::ffi::OsStrExt;
use std::path::Path;
use std::sync::OnceLock;
use windows::core::{s, w, Error as Failure, Interface, GUID, HRESULT, PCWSTR};
use windows::Win32::Foundation::{E_POINTER, RPC_E_CHANGED_MODE};
use windows::Win32::Media::MediaFoundation::{
    eAVEncH264VProfile_High, IMFAttributes, IMFMediaBuffer, IMFMediaType, IMFSample, IMFSinkWriter, MFMediaType_Video, MFNominalRange_16_235, MFTranscodeContainerType_MPEG4, MFVideoFormat_H264,
    MFVideoFormat_NV12, MFVideoInterlace_Progressive, MFVideoPrimaries_BT709, MFVideoTransFunc_709, MFVideoTransferMatrix_BT709, CODECAPI_AVEncMPVDefaultBPictureCount, MFSTARTUP_FULL, MF_MT_AVG_BITRATE,
    MF_MT_DEFAULT_STRIDE, MF_MT_FRAME_RATE, MF_MT_FRAME_SIZE, MF_MT_INTERLACE_MODE, MF_MT_MAJOR_TYPE, MF_MT_MPEG2_PROFILE, MF_MT_PIXEL_ASPECT_RATIO, MF_MT_SUBTYPE, MF_MT_TRANSFER_FUNCTION,
    MF_MT_VIDEO_NOMINAL_RANGE, MF_MT_VIDEO_PRIMARIES, MF_MT_YUV_MATRIX, MF_TRANSCODE_CONTAINERTYPE, MF_VERSION,
};
use windows::Win32::System::Com::{CoInitializeEx, CoUninitialize, COINIT_MULTITHREADED};
use windows::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryExW, LOAD_LIBRARY_SEARCH_SYSTEM32};

/// Media Foundation counts time in units of 100 ns.
const TICKS: f64 = 1e7;
/// Bits a second for every pixel of a frame, as the movies written on Apple platforms have.
const BITS_PER_PIXEL: u32 = 9;

fn failed(what: &'static str) -> impl Fn(Failure) -> Error {
    move |e| Error::Decode(format!("Media Foundation could not {what} ({:#010X}): {}", e.code().0, e.message()))
}

/// Two numbers in one attribute, as sizes and ratios are stored.
fn pair(high: u32, low: u32) -> u64 {
    (high as u64) << 32 | low as u64
}

/// Where a function leaves the object it makes.
type Out = *mut *mut c_void;
type Entry = unsafe extern "system" fn() -> isize;
type Start = unsafe extern "system" fn(u32, u32) -> HRESULT;
type Stop = unsafe extern "system" fn() -> HRESULT;
type Make = unsafe extern "system" fn(Out) -> HRESULT;
type MakeAttributes = unsafe extern "system" fn(Out, u32) -> HRESULT;
type MakeBuffer = unsafe extern "system" fn(u32, Out) -> HRESULT;
type MakeWriter = unsafe extern "system" fn(PCWSTR, *mut c_void, *mut c_void, Out) -> HRESULT;

/// The functions of Media Foundation that are called here. They are looked up
/// when first needed instead of being linked: the editions of Windows sold
/// without media features ("N") have no Media Foundation, and a program
/// linked to it does not start there.
struct Api {
    startup: Start,
    shutdown: Stop,
    attributes: MakeAttributes,
    media_type: Make,
    memory_buffer: MakeBuffer,
    sample: Make,
    sink_writer: MakeWriter,
}

impl Api {
    fn get() -> Result<&'static Api> {
        static API: OnceLock<Option<Api>> = OnceLock::new();
        API.get_or_init(Api::load).as_ref().ok_or(Error::Unsupported("this Windows has no Media Foundation (the Media Feature Pack adds it)"))
    }

    fn load() -> Option<Api> {
        // SAFETY: the libraries are the system's own and stay loaded for good,
        // and every function is taken with the signature it is declared with.
        unsafe {
            let platform = LoadLibraryExW(w!("mfplat.dll"), None, LOAD_LIBRARY_SEARCH_SYSTEM32).ok()?;
            let readwrite = LoadLibraryExW(w!("mfreadwrite.dll"), None, LOAD_LIBRARY_SEARCH_SYSTEM32).ok()?;
            Some(Api {
                startup: transmute::<Entry, Start>(GetProcAddress(platform, s!("MFStartup"))?),
                shutdown: transmute::<Entry, Stop>(GetProcAddress(platform, s!("MFShutdown"))?),
                attributes: transmute::<Entry, MakeAttributes>(GetProcAddress(platform, s!("MFCreateAttributes"))?),
                media_type: transmute::<Entry, Make>(GetProcAddress(platform, s!("MFCreateMediaType"))?),
                memory_buffer: transmute::<Entry, MakeBuffer>(GetProcAddress(platform, s!("MFCreateMemoryBuffer"))?),
                sample: transmute::<Entry, Make>(GetProcAddress(platform, s!("MFCreateSample"))?),
                sink_writer: transmute::<Entry, MakeWriter>(GetProcAddress(readwrite, s!("MFCreateSinkWriterFromURL"))?),
            })
        }
    }

    /// The object `make` leaves behind, which is then owned here.
    unsafe fn made<T: Interface>(make: impl FnOnce(Out) -> HRESULT) -> windows::core::Result<T> {
        let mut object = std::ptr::null_mut();
        make(&mut object).ok()?;
        if object.is_null() {
            return Err(E_POINTER.into());
        }
        Ok(T::from_raw(object))
    }

    unsafe fn new_attributes(&self) -> windows::core::Result<IMFAttributes> {
        Self::made(|out| (self.attributes)(out, 1))
    }

    unsafe fn new_media_type(&self) -> windows::core::Result<IMFMediaType> {
        Self::made(|out| (self.media_type)(out))
    }
}

/// COM and Media Foundation, running for as long as this lives.
struct Session {
    api: &'static Api,
    /// Whether COM was started here, and so is stopped here.
    com: bool,
}

impl Session {
    fn start() -> Result<Session> {
        let api = Api::get()?;
        // SAFETY: plain calls; `drop` undoes exactly what succeeded here.
        unsafe {
            let started = CoInitializeEx(None, COINIT_MULTITHREADED);
            // A thread that already runs COM in its other mode keeps that; the encoder works in both.
            if started.is_err() && started != RPC_E_CHANGED_MODE {
                return Err(failed("start COM")(started.into()));
            }
            let com = started.is_ok();
            if let Err(e) = (api.startup)(MF_VERSION, MFSTARTUP_FULL).ok() {
                if com {
                    CoUninitialize();
                }
                return Err(failed("start")(e));
            }
            Ok(Session { api, com })
        }
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        // SAFETY: pairs with the calls in `start`, on the same thread; the
        // objects made in between are released by then.
        unsafe {
            let _ = (self.api.shutdown)();
            if self.com {
                CoUninitialize();
            }
        }
    }
}

/// What both ends of the encoder are told about the frames.
unsafe fn describe(kind: &IMFMediaType, format: &GUID, size: (u32, u32), rate: (u32, u32), colours: bool) -> windows::core::Result<()> {
    kind.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
    kind.SetGUID(&MF_MT_SUBTYPE, format)?;
    kind.SetUINT64(&MF_MT_FRAME_SIZE, pair(size.0, size.1))?;
    kind.SetUINT64(&MF_MT_FRAME_RATE, pair(rate.0, rate.1))?;
    kind.SetUINT64(&MF_MT_PIXEL_ASPECT_RATIO, pair(1, 1))?;
    kind.SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32)?;
    if colours {
        // As the frames are made (`nv12`).
        kind.SetUINT32(&MF_MT_VIDEO_PRIMARIES, MFVideoPrimaries_BT709.0 as u32)?;
        kind.SetUINT32(&MF_MT_TRANSFER_FUNCTION, MFVideoTransFunc_709.0 as u32)?;
        kind.SetUINT32(&MF_MT_YUV_MATRIX, MFVideoTransferMatrix_BT709.0 as u32)?;
        kind.SetUINT32(&MF_MT_VIDEO_NOMINAL_RANGE, MFNominalRange_16_235.0 as u32)?;
    }
    Ok(())
}

/// A sink writer on the file `name` and its one stream, ready for frames.
/// With `colours`, both ends are told how the frames code their colours.
unsafe fn open(api: &Api, name: &[u16], size: (u32, u32), rate: (u32, u32), bitrate: u32, colours: bool) -> Result<(IMFSinkWriter, u32)> {
    let setup = failed("be set up");
    let container = api.new_attributes().map_err(&setup)?;
    container.SetGUID(&MF_TRANSCODE_CONTAINERTYPE, &MFTranscodeContainerType_MPEG4).map_err(&setup)?;
    let writer: IMFSinkWriter = Api::made(|out| (api.sink_writer)(PCWSTR(name.as_ptr()), std::ptr::null_mut(), container.as_raw(), out)).map_err(failed("create the movie"))?;

    let coded = api.new_media_type().map_err(&setup)?;
    describe(&coded, &MFVideoFormat_H264, size, rate, colours).map_err(&setup)?;
    coded.SetUINT32(&MF_MT_AVG_BITRATE, bitrate).map_err(&setup)?;
    coded.SetUINT32(&MF_MT_MPEG2_PROFILE, eAVEncH264VProfile_High.0 as u32).map_err(&setup)?;
    let stream = writer.AddStream(&coded).map_err(failed("encode H.264 at this size"))?;

    let raw = api.new_media_type().map_err(&setup)?;
    describe(&raw, &MFVideoFormat_NV12, size, rate, colours).map_err(&setup)?;
    // Rows follow one another from the top, without padding.
    raw.SetUINT32(&MF_MT_DEFAULT_STRIDE, size.0).map_err(&setup)?;
    // Frames are coded in the order they are shown, as on Apple platforms. An
    // encoder that does not take the setting is used as it is: the order of
    // its frames is copied along with them.
    let ordered = api.new_attributes().and_then(|options| options.SetUINT32(&CODECAPI_AVEncMPVDefaultBPictureCount, 0).map(|()| options));
    let accepted = ordered.and_then(|options| writer.SetInputMediaType(stream, &raw, &options));
    if accepted.is_err() {
        writer.SetInputMediaType(stream, &raw, None::<&IMFAttributes>).map_err(failed("take frames of this size"))?;
    }
    writer.BeginWriting().map_err(failed("start the movie"))?;
    Ok((writer, stream))
}

/// An MP4 being written, one frame at a time.
pub struct Encoder {
    // Fields are dropped in this order: the writer has to go before the session ends.
    writer: IMFSinkWriter,
    stream: u32,
    /// Bytes in a frame: a plane of luma and half as much chroma.
    length: usize,
    /// How long a frame is shown.
    step: i64,
    session: Session,
}

impl Encoder {
    /// Starts an MP4 at `path` (a local file) of frames `size` large, both
    /// sides even, each shown for `step` seconds. Media Foundation is started
    /// on this thread and stopped when the encoder is dropped, so the encoder
    /// stays on the thread that made it.
    pub fn new(path: &Path, size: (u32, u32), step: f64) -> Result<Encoder> {
        let (width, height) = size;
        if width < 2 || height < 2 || width % 2 != 0 || height % 2 != 0 || step.is_nan() || step <= 0.0 {
            return Err(Error::Malformed);
        }
        let bitrate = width.checked_mul(height).and_then(|pixels| pixels.checked_mul(BITS_PER_PIXEL)).ok_or(Error::Malformed)?;
        let rate = (((1000.0 / step).round() as u32).max(1), 1000);
        let name: Vec<u16> = path.as_os_str().encode_wide().chain([0]).collect();
        let session = Session::start()?;
        // SAFETY: `name` is terminated and outlives the calls; what the first
        // attempt made is released before the second starts the file anew.
        let (writer, stream) = unsafe {
            // An encoder that will not be told about colours goes without: the
            // movie made of its frames names them itself.
            match open(session.api, &name, size, rate, bitrate, true) {
                Ok(ready) => ready,
                Err(_) => open(session.api, &name, size, rate, bitrate, false)?,
            }
        };
        Ok(Encoder { writer, stream, length: (width as usize * height as usize) * 3 / 2, step: ((step * TICKS).round() as i64).max(1), session })
    }

    /// Adds a frame, shown from `seconds` into the movie: NV12, the rows of
    /// luma from the top, then the rows of interleaved chroma.
    pub fn write(&mut self, nv12: &[u8], seconds: f64) -> Result<()> {
        if nv12.len() != self.length {
            return Err(Error::Malformed);
        }
        let api = self.session.api;
        let failed = failed("encode a frame");
        // SAFETY: the buffer is asked for `length` bytes, and is only written
        // to while locked and after it has said that it holds as many.
        unsafe {
            let buffer: IMFMediaBuffer = Api::made(|out| (api.memory_buffer)(self.length as u32, out)).map_err(&failed)?;
            let (mut data, mut room) = (std::ptr::null_mut(), 0u32);
            buffer.Lock(&mut data, Some(&mut room), None).map_err(&failed)?;
            let fits = !data.is_null() && room as usize >= self.length;
            if fits {
                std::ptr::copy_nonoverlapping(nv12.as_ptr(), data, self.length);
            }
            buffer.Unlock().map_err(&failed)?;
            if !fits {
                return Err(Error::Decode("Media Foundation gave no room for a frame".into()));
            }
            buffer.SetCurrentLength(self.length as u32).map_err(&failed)?;
            let sample: IMFSample = Api::made(|out| (api.sample)(out)).map_err(&failed)?;
            sample.AddBuffer(&buffer).map_err(&failed)?;
            sample.SetSampleTime((seconds.max(0.0) * TICKS).round() as i64).map_err(&failed)?;
            sample.SetSampleDuration(self.step).map_err(&failed)?;
            self.writer.WriteSample(self.stream, &sample).map_err(&failed)
        }
    }

    /// Ends the movie. Only then is the file complete; an encoder that is
    /// dropped instead leaves a file that is none.
    pub fn finish(self) -> Result<()> {
        // SAFETY: the writer is live, and released when `self` goes.
        unsafe { self.writer.Finalize() }.map_err(failed("finish the movie"))
    }
}
