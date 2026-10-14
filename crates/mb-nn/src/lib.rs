//! rekordbox's neural-network runtime, used from the user's own install. Spec:
//! `docs/re/analysis-beats-hp.md` § 2 (TensorFlow, `TFWrapper`).
//!
//! rekordbox 7.2.19 ships TensorFlow 2.16.1 (`Frameworks/libtensorflow_cc.2.dylib`) and runs its
//! beat and vocal models through it. musicbox loads the same library and the same model files from
//! the installed app at run time with `dlopen`; nothing is copied or bundled. Before a model is
//! used, the library version and the model files' MD5 sums are checked against the values recorded
//! here, so a different rekordbox build is refused rather than silently giving other results.
//!
//! All `unsafe` FFI of the analysis lives in this crate.

use libloading::Library;
use md5::{Digest, Md5};
use std::ffi::{CStr, CString, c_char, c_int, c_void};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

/// Default location of the rekordbox app.
pub const DEFAULT_APP: &str = "/Applications/rekordbox 7/rekordbox.app";
/// Environment variable overriding [`DEFAULT_APP`].
pub const APP_ENV: &str = "MB_REKORDBOX_APP";
/// The TensorFlow version rekordbox 7.2.19 ships (`TF_Version`).
pub const TF_VERSION: &str = "2.16.1";
const TF_LIBRARY: &str = "Contents/Frameworks/libtensorflow_cc.2.dylib";

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("TensorFlow runtime unavailable: {0}")]
    Runtime(String),
    #[error("TensorFlow {found} found, {TF_VERSION} expected")]
    Version { found: String },
    #[error("model file {path} has MD5 {found}, {expected} expected")]
    Checksum {
        path: PathBuf,
        found: String,
        expected: &'static str,
    },
    #[error("{0}: {1}")]
    Io(PathBuf, std::io::Error),
    #[error("TensorFlow: {0}")]
    Tf(String),
}

pub type Result<T> = std::result::Result<T, Error>;

/// The models musicbox runs, with the files and MD5 sums of rekordbox 7.2.19.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Model {
    /// `models/detect_beat/model7`: the beat/downbeat network of the Dynamic high-precision
    /// analysis (`DatabaseMediator::GetBeatDetectModelNo7DirectoryPath`).
    BeatNo7,
    /// `models/detect_beat/model8`: the network of the other high-precision modes.
    BeatNo8,
    /// `vd_param.pb`: the vocal detector's frozen graph (`Discriminator::inference`).
    Vocal,
}

impl Model {
    /// Path relative to `rekordbox.app/Contents/Resources`.
    pub fn resource(self) -> &'static str {
        match self {
            Model::BeatNo7 => "models/detect_beat/model7",
            Model::BeatNo8 => "models/detect_beat/model8",
            Model::Vocal => "vd_param.pb",
        }
    }

    /// The files of the model (relative to [`Model::resource`]; empty for a single file) and
    /// their MD5 sums in rekordbox 7.2.19.
    fn files(self) -> &'static [(&'static str, &'static str)] {
        match self {
            Model::BeatNo7 => &[
                ("saved_model.pb", "2fcaa488335e8e9ee035997bc9ec19da"),
                ("variables/variables.data-00000-of-00001", "4f65199efecdd3d862ce9433bb4fca60"),
                ("variables/variables.index", "af86d1862acff1f79143b33d8a223799"),
            ],
            Model::BeatNo8 => &[
                ("saved_model.pb", "0ec3f9d3a0253050c4d535c9c93fe8a1"),
                ("variables/variables.data-00000-of-00001", "dfb57914726bf38e6dac829a2f82c1fb"),
                ("variables/variables.index", "f4835efea6bc04a64ca1103b9b95729a"),
            ],
            Model::Vocal => &[("", "13217b009062598a322f3427bb7cd21b")],
        }
    }
}

/// The installed rekordbox app.
pub fn app_path() -> PathBuf {
    std::env::var_os(APP_ENV)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(DEFAULT_APP))
}

/// Absolute path of a model's file or directory.
pub fn model_path(model: Model) -> PathBuf {
    app_path().join("Contents/Resources").join(model.resource())
}

fn md5_file(path: &Path) -> Result<String> {
    let data = std::fs::read(path).map_err(|e| Error::Io(path.to_path_buf(), e))?;
    Ok(Md5::digest(&data).iter().map(|b| format!("{b:02x}")).collect())
}

/// Checks a model's files against the recorded MD5 sums (once per process and model).
pub fn verify(model: Model) -> Result<()> {
    static DONE: [OnceLock<std::result::Result<(), String>>; 3] =
        [OnceLock::new(), OnceLock::new(), OnceLock::new()];
    let slot = &DONE[model as usize];
    let check = || -> Result<()> {
        let root = model_path(model);
        for (rel, expected) in model.files() {
            let path = if rel.is_empty() { root.clone() } else { root.join(rel) };
            let found = md5_file(&path)?;
            if found != *expected {
                return Err(Error::Checksum { path, found, expected });
            }
        }
        Ok(())
    };
    slot.get_or_init(|| check().map_err(|e| e.to_string()))
        .clone()
        .map_err(Error::Runtime)
}

// --- TensorFlow C API (tensorflow/c/c_api.h, 2.16) ---

#[repr(C)]
struct TfOutput {
    oper: *mut c_void,
    index: c_int,
}

type Ptr = *mut c_void;
const TF_FLOAT: c_int = 1;

struct Api {
    _lib: Library,
    version: unsafe extern "C" fn() -> *const c_char,
    new_status: unsafe extern "C" fn() -> Ptr,
    delete_status: unsafe extern "C" fn(Ptr),
    get_code: unsafe extern "C" fn(Ptr) -> c_int,
    message: unsafe extern "C" fn(Ptr) -> *const c_char,
    new_graph: unsafe extern "C" fn() -> Ptr,
    delete_graph: unsafe extern "C" fn(Ptr),
    new_session_options: unsafe extern "C" fn() -> Ptr,
    set_config: unsafe extern "C" fn(Ptr, *const c_void, usize, Ptr),
    delete_session_options: unsafe extern "C" fn(Ptr),
    load_saved_model: unsafe extern "C" fn(Ptr, Ptr, *const c_char, *const *const c_char, c_int, Ptr, Ptr, Ptr) -> Ptr,
    new_session: unsafe extern "C" fn(Ptr, Ptr, Ptr) -> Ptr,
    close_session: unsafe extern "C" fn(Ptr, Ptr),
    delete_session: unsafe extern "C" fn(Ptr, Ptr),
    operation_by_name: unsafe extern "C" fn(Ptr, *const c_char) -> Ptr,
    allocate_tensor: unsafe extern "C" fn(c_int, *const i64, c_int, usize) -> Ptr,
    tensor_data: unsafe extern "C" fn(Ptr) -> *mut c_void,
    tensor_type: unsafe extern "C" fn(Ptr) -> c_int,
    num_dims: unsafe extern "C" fn(Ptr) -> c_int,
    dim: unsafe extern "C" fn(Ptr, c_int) -> i64,
    delete_tensor: unsafe extern "C" fn(Ptr),
    session_run: unsafe extern "C" fn(
        Ptr,
        Ptr,
        *const TfOutput,
        *const Ptr,
        c_int,
        *const TfOutput,
        *mut Ptr,
        c_int,
        *const Ptr,
        c_int,
        Ptr,
        Ptr,
    ),
    new_buffer_from_string: unsafe extern "C" fn(*const c_void, usize) -> Ptr,
    delete_buffer: unsafe extern "C" fn(Ptr),
    new_import_options: unsafe extern "C" fn() -> Ptr,
    delete_import_options: unsafe extern "C" fn(Ptr),
    import_graph_def: unsafe extern "C" fn(Ptr, Ptr, Ptr, Ptr),
}

// The C API is thread-safe for distinct sessions; sessions themselves are `Sync` in TensorFlow.
unsafe impl Send for Api {}
unsafe impl Sync for Api {}

static API: OnceLock<std::result::Result<Api, String>> = OnceLock::new();

/// TensorFlow's log threshold variable. Each of its two libraries reads it once, at its first
/// log statement, and logs INFO lines to stderr by default.
const LOG_ENV: &str = "TF_CPP_MIN_LOG_LEVEL";
/// Set while musicbox holds `LOG_ENV` at 1 (warnings and errors only): from before the library
/// loads until the first session run has finished. A value the caller set is left alone.
static QUIET: Mutex<bool> = Mutex::new(false);

fn quiet_begin() {
    if std::env::var_os(LOG_ENV).is_none() {
        // SAFETY: done once, while the library loads and before TensorFlow starts threads; the
        // rest of musicbox reads the environment only through `std::env`, which locks.
        unsafe { std::env::set_var(LOG_ENV, "1") };
        *QUIET.lock().unwrap_or_else(|e| e.into_inner()) = true;
    }
}

fn quiet_end() {
    let mut held = QUIET.lock().unwrap_or_else(|e| e.into_inner());
    if *held {
        // SAFETY: as in `quiet_begin`; both libraries have read the value by now.
        unsafe { std::env::remove_var(LOG_ENV) };
        *held = false;
    }
}

fn api() -> Result<&'static Api> {
    API.get_or_init(|| load(&app_path().join(TF_LIBRARY)))
        .as_ref()
        .map_err(|e| Error::Runtime(e.clone()))
}

/// True when rekordbox's TensorFlow can be loaded and has the expected version.
pub fn available() -> bool {
    api().is_ok()
}

fn load(path: &Path) -> std::result::Result<Api, String> {
    quiet_begin();
    // SAFETY: loads rekordbox's TensorFlow; its initialisers register kernels and ops only.
    let lib = unsafe { Library::new(path) }.map_err(|e| format!("{}: {e}", path.display()))?;
    macro_rules! sym {
        ($name:literal) => {
            // SAFETY: the signatures follow tensorflow/c/c_api.h of TensorFlow 2.16.
            *unsafe { lib.get($name) }.map_err(|e| format!("{}: {e}", String::from_utf8_lossy($name)))?
        };
    }
    let api = Api {
        version: sym!(b"TF_Version"),
        new_status: sym!(b"TF_NewStatus"),
        delete_status: sym!(b"TF_DeleteStatus"),
        get_code: sym!(b"TF_GetCode"),
        message: sym!(b"TF_Message"),
        new_graph: sym!(b"TF_NewGraph"),
        delete_graph: sym!(b"TF_DeleteGraph"),
        new_session_options: sym!(b"TF_NewSessionOptions"),
        set_config: sym!(b"TF_SetConfig"),
        delete_session_options: sym!(b"TF_DeleteSessionOptions"),
        load_saved_model: sym!(b"TF_LoadSessionFromSavedModel"),
        new_session: sym!(b"TF_NewSession"),
        close_session: sym!(b"TF_CloseSession"),
        delete_session: sym!(b"TF_DeleteSession"),
        operation_by_name: sym!(b"TF_GraphOperationByName"),
        allocate_tensor: sym!(b"TF_AllocateTensor"),
        tensor_data: sym!(b"TF_TensorData"),
        tensor_type: sym!(b"TF_TensorType"),
        num_dims: sym!(b"TF_NumDims"),
        dim: sym!(b"TF_Dim"),
        delete_tensor: sym!(b"TF_DeleteTensor"),
        session_run: sym!(b"TF_SessionRun"),
        new_buffer_from_string: sym!(b"TF_NewBufferFromString"),
        delete_buffer: sym!(b"TF_DeleteBuffer"),
        new_import_options: sym!(b"TF_NewImportGraphDefOptions"),
        delete_import_options: sym!(b"TF_DeleteImportGraphDefOptions"),
        import_graph_def: sym!(b"TF_GraphImportGraphDef"),
        _lib: lib,
    };
    // SAFETY: TF_Version returns a static NUL-terminated string.
    let v = unsafe { CStr::from_ptr((api.version)()) }.to_string_lossy().into_owned();
    if v != TF_VERSION {
        return Err(Error::Version { found: v }.to_string());
    }
    Ok(api)
}

/// The TensorFlow version string of the loaded library.
pub fn version() -> Result<String> {
    let api = api()?;
    // SAFETY: static string owned by the library.
    Ok(unsafe { CStr::from_ptr((api.version)()) }.to_string_lossy().into_owned())
}

struct Status<'a>(&'a Api, Ptr);

impl<'a> Status<'a> {
    fn new(api: &'a Api) -> Self {
        // SAFETY: plain allocation.
        Status(api, unsafe { (api.new_status)() })
    }
    fn check(&self) -> Result<()> {
        // SAFETY: valid status handle.
        if unsafe { (self.0.get_code)(self.1) } == 0 {
            return Ok(());
        }
        // SAFETY: the message lives as long as the status.
        let m = unsafe { CStr::from_ptr((self.0.message)(self.1)) };
        Err(Error::Tf(m.to_string_lossy().into_owned()))
    }
}

impl Drop for Status<'_> {
    fn drop(&mut self) {
        // SAFETY: created by TF_NewStatus.
        unsafe { (self.0.delete_status)(self.1) }
    }
}

/// A dense `f32` tensor.
#[derive(Debug, Clone, PartialEq)]
pub struct Tensor {
    pub dims: Vec<i64>,
    pub data: Vec<f32>,
}

/// A loaded graph with its session. `TFWrapper::Model` keeps one per analysis.
pub struct Session {
    api: &'static Api,
    graph: Ptr,
    session: Ptr,
}

// TensorFlow sessions accept concurrent `Run` calls.
unsafe impl Send for Session {}
unsafe impl Sync for Session {}

/// `ConfigProto` as `TFWrapper::defaultSessionOptions` @0x103404d18 builds it on a machine without
/// a GPU: intra- and inter-op pools of `threads` (at least 1) and soft placement.
fn config_proto(threads: usize) -> Vec<u8> {
    fn varint(out: &mut Vec<u8>, mut v: u64) {
        while v >= 0x80 {
            out.push(v as u8 | 0x80);
            v >>= 7;
        }
        out.push(v as u8);
    }
    let t = threads.max(1) as u64;
    let mut p = Vec::new();
    p.push(2 << 3); // intra_op_parallelism_threads
    varint(&mut p, t);
    p.push(5 << 3); // inter_op_parallelism_threads
    varint(&mut p, t);
    p.extend_from_slice(&[7 << 3, 1]); // allow_soft_placement
    p
}

fn session_options(api: &Api, threads: usize) -> Result<Ptr> {
    let st = Status::new(api);
    // SAFETY: valid options handle; the proto bytes outlive the call.
    let opts = unsafe { (api.new_session_options)() };
    let cfg = config_proto(threads);
    unsafe { (api.set_config)(opts, cfg.as_ptr().cast(), cfg.len(), st.1) };
    if let Err(e) = st.check() {
        // SAFETY: created above.
        unsafe { (api.delete_session_options)(opts) };
        return Err(e);
    }
    Ok(opts)
}

impl Session {
    /// Loads one of rekordbox's models (`TFWrapper::Model::Pimpl` @0x10340520c: tag `serve`;
    /// the vocal model is a frozen graph). `threads` = `std::thread::hardware_concurrency()` in
    /// rekordbox.
    pub fn open(model: Model, threads: usize) -> Result<Session> {
        verify(model)?;
        match model {
            Model::Vocal => {
                let path = model_path(model);
                let pb = std::fs::read(&path).map_err(|e| Error::Io(path, e))?;
                Self::frozen_graph(&pb, threads)
            }
            _ => Self::saved_model(&model_path(model), threads),
        }
    }

    /// Loads a SavedModel directory with the `serve` tag.
    pub fn saved_model(dir: &Path, threads: usize) -> Result<Session> {
        let api = api()?;
        let dir_c = CString::new(dir.as_os_str().as_encoded_bytes()).map_err(|e| Error::Tf(e.to_string()))?;
        let tag = c"serve";
        let tags = [tag.as_ptr()];
        let opts = session_options(api, threads)?;
        let st = Status::new(api);
        // SAFETY: valid handles; strings live across the call.
        let graph = unsafe { (api.new_graph)() };
        let session = unsafe {
            (api.load_saved_model)(
                opts,
                std::ptr::null_mut(),
                dir_c.as_ptr(),
                tags.as_ptr(),
                1,
                graph,
                std::ptr::null_mut(),
                st.1,
            )
        };
        unsafe { (api.delete_session_options)(opts) };
        if let Err(e) = st.check() {
            unsafe { (api.delete_graph)(graph) };
            return Err(e);
        }
        Ok(Session { api, graph, session })
    }

    /// Imports a frozen `GraphDef` and opens a session on it.
    pub fn frozen_graph(pb: &[u8], threads: usize) -> Result<Session> {
        let api = api()?;
        let st = Status::new(api);
        // SAFETY: the buffer copies `pb`; all handles are freed below.
        let graph = unsafe { (api.new_graph)() };
        let buf = unsafe { (api.new_buffer_from_string)(pb.as_ptr().cast(), pb.len()) };
        let iopts = unsafe { (api.new_import_options)() };
        unsafe { (api.import_graph_def)(graph, buf, iopts, st.1) };
        unsafe {
            (api.delete_import_options)(iopts);
            (api.delete_buffer)(buf);
        }
        if let Err(e) = st.check() {
            unsafe { (api.delete_graph)(graph) };
            return Err(e);
        }
        let opts = session_options(api, threads)?;
        let session = unsafe { (api.new_session)(graph, opts, st.1) };
        unsafe { (api.delete_session_options)(opts) };
        if let Err(e) = st.check() {
            unsafe { (api.delete_graph)(graph) };
            return Err(e);
        }
        Ok(Session { api, graph, session })
    }

    fn output(&self, name: &str) -> Result<TfOutput> {
        let (op, index) = match name.rsplit_once(':') {
            Some((op, i)) => (op, i.parse().map_err(|_| Error::Tf(format!("bad tensor name {name}")))?),
            None => (name, 0),
        };
        let c = CString::new(op).map_err(|e| Error::Tf(e.to_string()))?;
        // SAFETY: valid graph; the name is NUL-terminated.
        let oper = unsafe { (self.api.operation_by_name)(self.graph, c.as_ptr()) };
        if oper.is_null() {
            return Err(Error::Tf(format!("no operation {op}")));
        }
        Ok(TfOutput { oper, index })
    }

    /// Runs the graph: feeds `inputs` (tensor name → f32 tensor) and fetches `outputs`.
    pub fn run(&self, inputs: &[(&str, &Tensor)], outputs: &[&str]) -> Result<Vec<Tensor>> {
        let api = self.api;
        let mut in_ops = Vec::with_capacity(inputs.len());
        let mut in_vals: Vec<Ptr> = Vec::with_capacity(inputs.len());
        let free = |vals: &[Ptr]| {
            for &t in vals {
                if !t.is_null() {
                    // SAFETY: tensors allocated by TF_AllocateTensor or returned by TF_SessionRun.
                    unsafe { (api.delete_tensor)(t) };
                }
            }
        };
        for (name, t) in inputs {
            let op = match self.output(name) {
                Ok(op) => op,
                Err(e) => {
                    free(&in_vals);
                    return Err(e);
                }
            };
            let bytes = t.data.len() * 4;
            // SAFETY: allocates `bytes` and copies the data into it.
            let tt = unsafe { (api.allocate_tensor)(TF_FLOAT, t.dims.as_ptr(), t.dims.len() as c_int, bytes) };
            unsafe { std::ptr::copy_nonoverlapping(t.data.as_ptr(), (api.tensor_data)(tt).cast::<f32>(), t.data.len()) };
            in_ops.push(op);
            in_vals.push(tt);
        }
        let out_ops = match outputs.iter().map(|n| self.output(n)).collect::<Result<Vec<_>>>() {
            Ok(o) => o,
            Err(e) => {
                free(&in_vals);
                return Err(e);
            }
        };
        let mut out_vals: Vec<Ptr> = vec![std::ptr::null_mut(); out_ops.len()];
        let st = Status::new(api);
        // SAFETY: arrays sized as passed; the session owns nothing of ours after the call.
        unsafe {
            (api.session_run)(
                self.session,
                std::ptr::null_mut(),
                in_ops.as_ptr(),
                in_vals.as_ptr(),
                in_ops.len() as c_int,
                out_ops.as_ptr(),
                out_vals.as_mut_ptr(),
                out_ops.len() as c_int,
                std::ptr::null(),
                0,
                std::ptr::null_mut(),
                st.1,
            )
        };
        free(&in_vals);
        quiet_end();
        if let Err(e) = st.check() {
            free(&out_vals);
            return Err(e);
        }
        let mut result = Vec::with_capacity(out_vals.len());
        for &t in &out_vals {
            // SAFETY: output tensors from a successful run.
            let (ty, nd) = unsafe { ((api.tensor_type)(t), (api.num_dims)(t)) };
            if ty != TF_FLOAT {
                free(&out_vals);
                return Err(Error::Tf(format!("output type {ty}, float expected")));
            }
            let dims: Vec<i64> = (0..nd).map(|i| unsafe { (api.dim)(t, i) }).collect();
            let n = dims.iter().product::<i64>().max(0) as usize;
            let mut data = vec![0.0f32; n];
            unsafe { std::ptr::copy_nonoverlapping((api.tensor_data)(t).cast::<f32>(), data.as_mut_ptr(), n) };
            result.push(Tensor { dims, data });
        }
        free(&out_vals);
        Ok(result)
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        let st = Status::new(self.api);
        // SAFETY: handles created by this session; closed once.
        unsafe {
            (self.api.close_session)(self.session, st.1);
            (self.api.delete_session)(self.session, st.1);
            (self.api.delete_graph)(self.graph);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_proto_bytes() {
        assert_eq!(config_proto(14), vec![0x10, 14, 0x28, 14, 0x38, 1]);
        assert_eq!(config_proto(0), vec![0x10, 1, 0x28, 1, 0x38, 1]);
        assert_eq!(config_proto(200)[1..3], [0xc8, 0x01]);
    }

    /// Runs only where rekordbox 7.2.19 is installed.
    #[test]
    fn beat_model_runs() {
        if !available() {
            eprintln!("rekordbox TensorFlow not installed; skipped");
            return;
        }
        let s = Session::open(Model::BeatNo8, 2).unwrap();
        let input = Tensor { dims: vec![1, 44100], data: (0..44100).map(|i| ((i as f32) * 0.05).sin() * 0.1).collect() };
        let out = s.run(&[("serving_default_fltp:0", &input)], &["StatefulPartitionedCall:1"]).unwrap();
        assert_eq!(out[0].dims.len(), 2);
        assert!(out[0].data.iter().all(|v| v.is_finite()));
    }
}
