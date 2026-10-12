//! Runtime binding to the libmpg123 that ships inside the user's rekordbox install
//! (`docs/re/decode.md` § MP3). The library is loaded with `dlopen` from the installed app and is
//! never copied or bundled. rekordbox loads it the same way (`Mpg123DynamicLibraryHandle::
//! loadMpg123DynamicLibrary` @0x102818d90: the sibling of the executable, then `mpg123_init`).

use crate::{Error, Result};
use libloading::Library;
use std::ffi::{c_char, c_double, c_int, c_long, c_uchar, c_void};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// Where rekordbox 7 keeps its libmpg123.
pub const DEFAULT_LIBRARY: &str = "/Applications/rekordbox 7/rekordbox.app/Contents/MacOS/libmpg123.0.dylib";

/// Environment variable that overrides [`DEFAULT_LIBRARY`].
pub const LIBRARY_ENV: &str = "MB_MPG123_LIB";

pub const MPG123_OK: c_int = 0;
pub const MPG123_NEED_MORE: c_int = -10;
pub const MPG123_NEW_FORMAT: c_int = -11;
pub const MPG123_BAD_OUTFORMAT: c_int = 1;

// mpg123_parms
const MPG123_VERBOSE: c_int = 0;
const MPG123_FLAGS: c_int = 1;
const MPG123_INDEX_SIZE: c_int = 15;
// mpg123_param_flags
const MPG123_SEEKBUFFER: c_long = 0x100;
const MPG123_FUZZY: c_long = 0x200;
/// MPG123_ENC_SIGNED_16
const MPG123_ENC_SIGNED_16: c_int = 0xd0;

type Handle = *mut c_void;

struct Api {
    _lib: Library,
    new: unsafe extern "C" fn(*const c_char, *mut c_int) -> Handle,
    delete: unsafe extern "C" fn(Handle),
    param: unsafe extern "C" fn(Handle, c_int, c_long, c_double) -> c_int,
    format_none: unsafe extern "C" fn(Handle) -> c_int,
    format: unsafe extern "C" fn(Handle, c_long, c_int, c_int) -> c_int,
    open_feed: unsafe extern "C" fn(Handle) -> c_int,
    feed: unsafe extern "C" fn(Handle, *const c_uchar, usize) -> c_int,
    decode_frame: unsafe extern "C" fn(Handle, *mut i64, *mut *mut c_uchar, *mut usize) -> c_int,
    errcode: unsafe extern "C" fn(Handle) -> c_int,
}

// The function pointers are plain C entry points; mpg123 handles are used from one thread at a time.
unsafe impl Send for Api {}
unsafe impl Sync for Api {}

static API: OnceLock<std::result::Result<Api, String>> = OnceLock::new();

/// Path of the library that will be loaded.
pub fn library_path() -> PathBuf {
    std::env::var_os(LIBRARY_ENV).map(PathBuf::from).unwrap_or_else(|| PathBuf::from(DEFAULT_LIBRARY))
}

/// True when rekordbox's libmpg123 can be loaded on this machine.
pub fn available() -> bool {
    api().is_ok()
}

fn api() -> Result<&'static Api> {
    API.get_or_init(|| load(&library_path()))
        .as_ref()
        .map_err(|e| Error::Mpg123(e.clone()))
}

fn load(path: &Path) -> std::result::Result<Api, String> {
    // SAFETY: loading a C library; its initialisers only set up decoder tables.
    let lib = unsafe { Library::new(path) }.map_err(|e| format!("{}: {e}", path.display()))?;
    macro_rules! sym {
        ($name:literal) => {
            // SAFETY: the symbol types match mpg123.h for the 64-bit off_t build (API version 38+).
            *unsafe { lib.get($name) }.map_err(|e| format!("{}: {e}", String::from_utf8_lossy($name)))?
        };
    }
    let init: unsafe extern "C" fn() -> c_int = sym!(b"mpg123_init");
    // SAFETY: global one-time init, as rekordbox does right after loading.
    if unsafe { init() } != MPG123_OK {
        return Err("mpg123_init failed".into());
    }
    let api = Api {
        new: sym!(b"mpg123_new"),
        delete: sym!(b"mpg123_delete"),
        param: sym!(b"mpg123_param"),
        format_none: sym!(b"mpg123_format_none"),
        format: sym!(b"mpg123_format"),
        open_feed: sym!(b"mpg123_open_feed"),
        feed: sym!(b"mpg123_feed"),
        decode_frame: sym!(b"mpg123_decode_frame"),
        errcode: sym!(b"mpg123_errcode"),
        _lib: lib,
    };
    Ok(api)
}

/// One mpg123 decoder in feed mode, configured like `FileReadMp3::decoderOpen` @0x102819b14:
/// verbose 0, flags exactly `SEEKBUFFER | FUZZY` (this clears mpg123's default GAPLESS flag, so no
/// encoder delay or padding is removed), index size −1, output fixed to signed 16-bit at the
/// stream's rate and channel count.
pub struct Decoder {
    api: &'static Api,
    h: Handle,
}

// The handle is owned exclusively by this value.
unsafe impl Send for Decoder {}

/// Result of one `mpg123_decode_frame` call.
pub struct Frame<'a> {
    pub ret: c_int,
    pub pcm: &'a [i16],
    pub bytes: usize,
}

impl Decoder {
    pub fn new(rate: i32, channels: u16) -> Result<Self> {
        let api = api()?;
        let mut err = 0;
        // SAFETY: plain C calls on a handle we own; arguments follow mpg123.h.
        unsafe {
            let h = (api.new)(std::ptr::null(), &mut err);
            if h.is_null() {
                return Err(Error::Mpg123(format!("mpg123_new failed ({err})")));
            }
            let d = Self { api, h };
            let steps = [
                (api.param)(h, MPG123_VERBOSE, 0, 0.0),
                (api.param)(h, MPG123_FLAGS, MPG123_SEEKBUFFER | MPG123_FUZZY, 0.0),
                (api.param)(h, MPG123_INDEX_SIZE, -1, 0.0),
                (api.format_none)(h),
                (api.format)(h, c_long::from(rate), c_int::from(channels), MPG123_ENC_SIGNED_16),
                (api.open_feed)(h),
            ];
            if let Some(i) = steps.iter().position(|&r| r != MPG123_OK) {
                return Err(Error::Mpg123(format!("failed to create decoder ({})", i + 2)));
            }
            Ok(d)
        }
    }

    /// `mpg123_open_feed`: closes the current stream (resetting the frame state) and starts a new feed.
    pub fn open_feed(&mut self) -> c_int {
        // SAFETY: valid handle.
        unsafe { (self.api.open_feed)(self.h) }
    }

    pub fn feed(&mut self, data: &[u8]) -> c_int {
        // SAFETY: valid handle; mpg123 copies the input.
        unsafe { (self.api.feed)(self.h, data.as_ptr(), data.len()) }
    }

    /// `mpg123_decode_frame`. The returned PCM borrows mpg123's internal buffer.
    pub fn decode_frame(&mut self) -> Frame<'_> {
        let mut num = 0i64;
        let mut audio: *mut c_uchar = std::ptr::null_mut();
        let mut bytes = 0usize;
        // SAFETY: valid handle and out pointers.
        let ret = unsafe { (self.api.decode_frame)(self.h, &mut num, &mut audio, &mut bytes) };
        let pcm = if audio.is_null() || bytes < 2 {
            &[][..]
        } else {
            // SAFETY: mpg123 guarantees `bytes` valid bytes at `audio` until the next decode call,
            // and its output buffer is aligned for the 16-bit encoding we requested.
            unsafe { std::slice::from_raw_parts(audio.cast::<i16>(), bytes / 2) }
        };
        Frame { ret, pcm, bytes }
    }

    pub fn errcode(&self) -> c_int {
        // SAFETY: valid handle.
        unsafe { (self.api.errcode)(self.h) }
    }
}

impl Drop for Decoder {
    fn drop(&mut self) {
        // SAFETY: handle created by mpg123_new and not used afterwards.
        unsafe { (self.api.delete)(self.h) }
    }
}
