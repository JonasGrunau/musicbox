//! rekordbox's ONNX Runtime 1.18 (`Contents/Frameworks/libonnxruntime.1.18.0.dylib`), loaded
//! with `dlopen` from the user's install like TensorFlow, for the embedding and auto-cue models.
//! The C API is the function table `OrtGetApiBase()->GetApi(18)` returns (`onnxruntime_c_api.h`,
//! append-only); rekordbox reaches it through the C++ wrapper `Ort::Global<void>::api_`.
//! Spec: `docs/re/analysis-embedding.md` § 2.

use super::{Error, Result, app_path, md5_file};
use libloading::Library;
use std::ffi::{CStr, CString, c_char, c_int, c_void};
use std::path::PathBuf;
use std::sync::OnceLock;

/// The ONNX Runtime version rekordbox 7.2.19 ships.
pub const ORT_VERSION: &str = "1.18.0";
const ORT_LIBRARY: &str = "Contents/Frameworks/libonnxruntime.1.18.0.dylib";
const ORT_API_VERSION: u32 = 18;

/// rekordbox's ONNX models, with their MD5 sums in 7.2.19.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum OnnxModel {
    /// The audio CNN: tags and 64-dimensional features (`FeatureExtractor`).
    CnnEmbedding,
    /// Scaler before the UMAP projection.
    NormBeforeUmap,
    /// The 64 → 2 UMAP projection.
    UmapEmbedder,
    /// Scaler after it.
    NormAfterUmap,
    /// Genre classifier on the 64 features and the 2-D position.
    Genre,
    /// Energy ("danceability") regressor.
    Energy,
    /// Auto cue: the waveform CNN.
    SampleCnn,
    /// Auto cue: the cue position regressor.
    CuePrediction,
}

impl OnnxModel {
    /// Path relative to `Contents/Resources/models` and MD5.
    fn file(self) -> (&'static str, &'static str) {
        match self {
            Self::CnnEmbedding => ("embed_model/cnn_embedding.onnx", "8024c9bafacb11806213cbc718f92a7c"),
            Self::NormBeforeUmap => ("embed_model/norm_before_umap.onnx", "87f3ac3c0e0f2ca65da5673050f54e63"),
            Self::UmapEmbedder => ("embed_model/umap_embedder.onnx", "e806996a612d36614dd694b978afe83f"),
            Self::NormAfterUmap => ("embed_model/norm_after_umap.onnx", "67af0820c30de3a1f22492328942ea28"),
            Self::Genre => ("embed_model/model_genre.onnx", "e5b6d44e7f3314958359c5649b66a69d"),
            Self::Energy => ("embed_model/model_energy.onnx", "97138d6f61b6b2de983634f48ddf05df"),
            Self::SampleCnn => ("detect_cue_model/samplecnn.onnx", "7f4e89752271601cd92cfbdb7818de65"),
            Self::CuePrediction => ("detect_cue_model/cue_prediction_model_no_abs_position.onnx", "ba0f8a6f348d0c3a36315ce4f26329c7"),
        }
    }

    pub fn path(self) -> PathBuf {
        app_path().join("Contents/Resources/models").join(self.file().0)
    }

    /// Checks the file against the recorded MD5.
    pub fn verify(self) -> Result<()> {
        let path = self.path();
        let found = md5_file(&path)?;
        let expected = self.file().1;
        if found != expected {
            return Err(Error::Checksum { path, found, expected });
        }
        Ok(())
    }
}

type Ptr = *mut c_void;
type Status = Ptr;

#[repr(C)]
struct ApiBase {
    get_api: unsafe extern "C" fn(u32) -> *const Ptr,
    get_version_string: unsafe extern "C" fn() -> *const c_char,
}

/// The OrtApi entries used here, by their index in the table.
struct Api {
    _lib: Library,
    table: *const Ptr,
    env: Ptr,
    cpu: Ptr,
}

unsafe impl Send for Api {}
unsafe impl Sync for Api {}

mod idx {
    pub const GET_ERROR_MESSAGE: usize = 2;
    pub const CREATE_ENV: usize = 3;
    pub const CREATE_SESSION: usize = 7;
    pub const RUN: usize = 9;
    pub const CREATE_SESSION_OPTIONS: usize = 10;
    pub const SET_SESSION_EXECUTION_MODE: usize = 13;
    pub const DISABLE_CPU_MEM_ARENA: usize = 19;
    pub const SET_GRAPH_OPTIMIZATION_LEVEL: usize = 23;
    pub const SET_INTRA_OP_THREADS: usize = 24;
    pub const SET_INTER_OP_THREADS: usize = 25;
    pub const CREATE_TENSOR_WITH_DATA: usize = 49;
    pub const GET_TENSOR_MUTABLE_DATA: usize = 51;
    pub const GET_TENSOR_ELEMENT_TYPE: usize = 60;
    pub const GET_DIMENSIONS_COUNT: usize = 61;
    pub const GET_DIMENSIONS: usize = 62;
    pub const GET_TENSOR_TYPE_AND_SHAPE: usize = 65;
    pub const CREATE_CPU_MEMORY_INFO: usize = 69;
    pub const RELEASE_STATUS: usize = 93;
    pub const RELEASE_SESSION: usize = 95;
    pub const RELEASE_VALUE: usize = 96;
    pub const RELEASE_TENSOR_TYPE_AND_SHAPE_INFO: usize = 99;
    pub const RELEASE_SESSION_OPTIONS: usize = 100;
}

const ELEM_FLOAT: c_int = 1;
const ELEM_INT64: c_int = 7;
const ELEM_DOUBLE: c_int = 11;

impl Api {
    /// The function at `i`, typed by the caller.
    unsafe fn f<T: Copy>(&self, i: usize) -> T {
        // SAFETY: `T` is the C signature of entry `i` (onnxruntime_c_api.h, version 18).
        unsafe { std::mem::transmute_copy(&*self.table.add(i)) }
    }

    fn check(&self, st: Status) -> Result<()> {
        if st.is_null() {
            return Ok(());
        }
        // SAFETY: a non-null status from the API; released after the message is copied.
        let msg = unsafe {
            let m: unsafe extern "C" fn(Status) -> *const c_char = self.f(idx::GET_ERROR_MESSAGE);
            let s = CStr::from_ptr(m(st)).to_string_lossy().into_owned();
            let r: unsafe extern "C" fn(Status) = self.f(idx::RELEASE_STATUS);
            r(st);
            s
        };
        Err(Error::Onnx(msg))
    }
}

static API: OnceLock<std::result::Result<Api, String>> = OnceLock::new();

fn api() -> Result<&'static Api> {
    API.get_or_init(load).as_ref().map_err(|e| Error::Runtime(e.clone()))
}

fn load() -> std::result::Result<Api, String> {
    let path = app_path().join(ORT_LIBRARY);
    // SAFETY: rekordbox's ONNX Runtime; its initialisers only register kernels.
    let lib = unsafe { Library::new(&path) }.map_err(|e| format!("{}: {e}", path.display()))?;
    // SAFETY: `OrtGetApiBase` returns a static table.
    let base = unsafe {
        let f: libloading::Symbol<unsafe extern "C" fn() -> *const ApiBase> = lib.get(b"OrtGetApiBase").map_err(|e| e.to_string())?;
        &*f()
    };
    // SAFETY: static NUL-terminated string.
    let v = unsafe { CStr::from_ptr((base.get_version_string)()) }.to_string_lossy().into_owned();
    if v != ORT_VERSION {
        return Err(format!("ONNX Runtime {v} found, {ORT_VERSION} expected"));
    }
    // SAFETY: the version is supported, so the table is non-null.
    let table = unsafe { (base.get_api)(ORT_API_VERSION) };
    if table.is_null() {
        return Err("ONNX Runtime API 18 unavailable".into());
    }
    let mut api = Api { _lib: lib, table, env: std::ptr::null_mut(), cpu: std::ptr::null_mut() };
    // SAFETY: entries per the header; out-pointers are valid.
    unsafe {
        // ORT_LOGGING_LEVEL_ERROR, as `utils::ONNXRunner` creates its environment
        let create_env: unsafe extern "C" fn(c_int, *const c_char, *mut Ptr) -> Status = api.f(idx::CREATE_ENV);
        let mut env: Ptr = std::ptr::null_mut();
        api.check(create_env(3, c"".as_ptr(), &mut env)).map_err(|e| e.to_string())?;
        // OrtDeviceAllocator, OrtMemTypeCPU
        let create_cpu: unsafe extern "C" fn(c_int, c_int, *mut Ptr) -> Status = api.f(idx::CREATE_CPU_MEMORY_INFO);
        let mut cpu: Ptr = std::ptr::null_mut();
        api.check(create_cpu(0, -1, &mut cpu)).map_err(|e| e.to_string())?;
        (api.env, api.cpu) = (env, cpu);
    }
    Ok(api)
}

/// True when rekordbox's ONNX Runtime loads and has the expected version.
pub fn available() -> bool {
    api().is_ok()
}

/// A tensor passed to or returned from [`OrtSession::run`].
#[derive(Debug, Clone, PartialEq)]
pub enum OrtTensor {
    F32 { dims: Vec<i64>, data: Vec<f32> },
    F64 { dims: Vec<i64>, data: Vec<f64> },
    I64 { dims: Vec<i64>, data: Vec<i64> },
}

/// One model session, set up as `utils::ONNXRunner::ONNXRunner` @0x1005a5ea0 does on the CPU:
/// memory arena off, parallel execution with `threads` intra- and inter-op threads (sequential
/// with one), all graph optimisations.
pub struct OrtSession {
    api: &'static Api,
    session: Ptr,
}

unsafe impl Send for OrtSession {}
unsafe impl Sync for OrtSession {}

impl OrtSession {
    pub fn open(model: OnnxModel, threads: usize) -> Result<Self> {
        let threads = threads.max(1) as c_int;
        Self::open_with(model, |api, set| {
            // ORT_SEQUENTIAL for one thread, else ORT_PARALLEL
            set(api, idx::SET_SESSION_EXECUTION_MODE, c_int::from(threads > 1))?;
            set(api, idx::SET_INTRA_OP_THREADS, threads)?;
            set(api, idx::SET_INTER_OP_THREADS, threads)?;
            // ORT_ENABLE_ALL
            set(api, idx::SET_GRAPH_OPTIMIZATION_LEVEL, 99)
        })
    }

    /// A session as `CuePointPredictor::initilaize_session` @0x101b49614 opens the auto-cue
    /// models: memory arena off, extended graph optimisations (level 2), the default threads and
    /// execution mode.
    pub fn open_cue(model: OnnxModel) -> Result<Self> {
        Self::open_with(model, |api, set| set(api, idx::SET_GRAPH_OPTIMIZATION_LEVEL, 2))
    }

    /// Opens `model` with the CPU memory arena off and the options `configure` sets through its
    /// setter (an `int` option by table index).
    fn open_with(model: OnnxModel, configure: impl FnOnce(&Api, &dyn Fn(&Api, usize, c_int) -> Result<()>) -> Result<()>) -> Result<Self> {
        model.verify()?;
        let api = api()?;
        let path = model.path();
        let path_c = CString::new(path.as_os_str().as_encoded_bytes()).map_err(|e| Error::Onnx(e.to_string()))?;
        // SAFETY: entries per the header; handles are released on every path.
        unsafe {
            let mut opts: Ptr = std::ptr::null_mut();
            let create: unsafe extern "C" fn(*mut Ptr) -> Status = api.f(idx::CREATE_SESSION_OPTIONS);
            api.check(create(&mut opts))?;
            let release_opts: unsafe extern "C" fn(Ptr) = api.f(idx::RELEASE_SESSION_OPTIONS);
            let set = |api: &Api, i: usize, v: c_int| -> Result<()> {
                let f: unsafe extern "C" fn(Ptr, c_int) -> Status = api.f(i);
                api.check(f(opts, v))
            };
            let disable_arena: unsafe extern "C" fn(Ptr) -> Status = api.f(idx::DISABLE_CPU_MEM_ARENA);
            let setup = api.check(disable_arena(opts)).and_then(|()| configure(api, &set));
            if let Err(e) = setup {
                release_opts(opts);
                return Err(e);
            }
            let mut session: Ptr = std::ptr::null_mut();
            let open: unsafe extern "C" fn(Ptr, *const c_char, Ptr, *mut Ptr) -> Status = api.f(idx::CREATE_SESSION);
            let r = api.check(open(api.env, path_c.as_ptr(), opts, &mut session));
            release_opts(opts);
            r?;
            Ok(Self { api, session })
        }
    }

    /// Runs the model: `inputs` by name, `outputs` fetched by name (f32, f64 or i64 tensors).
    pub fn run(&self, inputs: &[(&str, &OrtTensor)], outputs: &[&str]) -> Result<Vec<OrtTensor>> {
        let api = self.api;
        let in_names: Vec<CString> = inputs.iter().map(|(n, _)| CString::new(*n).unwrap()).collect();
        let out_names: Vec<CString> = outputs.iter().map(|n| CString::new(*n).unwrap()).collect();
        let in_ptrs: Vec<*const c_char> = in_names.iter().map(|c| c.as_ptr()).collect();
        let out_ptrs: Vec<*const c_char> = out_names.iter().map(|c| c.as_ptr()).collect();
        // SAFETY: entries per the header; the input data outlives the run, every value is
        // released.
        unsafe {
            let release: unsafe extern "C" fn(Ptr) = api.f(idx::RELEASE_VALUE);
            let free = |vals: &[Ptr]| {
                for &v in vals {
                    if !v.is_null() {
                        release(v);
                    }
                }
            };
            let mut values: Vec<Ptr> = Vec::with_capacity(inputs.len());
            let create: unsafe extern "C" fn(Ptr, *mut c_void, usize, *const i64, usize, c_int, *mut Ptr) -> Status =
                api.f(idx::CREATE_TENSOR_WITH_DATA);
            for (_, t) in inputs {
                let (data, bytes, dims, ty): (*mut c_void, usize, &[i64], c_int) = match t {
                    OrtTensor::F32 { dims, data } => (data.as_ptr() as *mut c_void, data.len() * 4, dims, ELEM_FLOAT),
                    OrtTensor::F64 { dims, data } => (data.as_ptr() as *mut c_void, data.len() * 8, dims, ELEM_DOUBLE),
                    OrtTensor::I64 { dims, data } => (data.as_ptr() as *mut c_void, data.len() * 8, dims, ELEM_INT64),
                };
                let mut v: Ptr = std::ptr::null_mut();
                if let Err(e) = api.check(create(api.cpu, data, bytes, dims.as_ptr(), dims.len(), ty, &mut v)) {
                    free(&values);
                    return Err(e);
                }
                values.push(v);
            }
            let mut outs: Vec<Ptr> = vec![std::ptr::null_mut(); outputs.len()];
            let run: unsafe extern "C" fn(Ptr, Ptr, *const *const c_char, *const Ptr, usize, *const *const c_char, usize, *mut Ptr) -> Status =
                api.f(idx::RUN);
            let r = api.check(run(
                self.session,
                std::ptr::null_mut(),
                in_ptrs.as_ptr(),
                values.as_ptr(),
                values.len(),
                out_ptrs.as_ptr(),
                outs.len(),
                outs.as_mut_ptr(),
            ));
            free(&values);
            if let Err(e) = r {
                free(&outs);
                return Err(e);
            }
            let mut result = Vec::with_capacity(outs.len());
            for &v in &outs {
                match self.tensor(v) {
                    Ok(t) => result.push(t),
                    Err(e) => {
                        free(&outs);
                        return Err(e);
                    }
                }
            }
            free(&outs);
            Ok(result)
        }
    }

    /// Copies an output value (f32, f64 or i64 tensor).
    unsafe fn tensor(&self, v: Ptr) -> Result<OrtTensor> {
        let api = self.api;
        // SAFETY: `v` is a tensor value from a successful run.
        unsafe {
            let mut info: Ptr = std::ptr::null_mut();
            let tas: unsafe extern "C" fn(Ptr, *mut Ptr) -> Status = api.f(idx::GET_TENSOR_TYPE_AND_SHAPE);
            api.check(tas(v, &mut info))?;
            let release_info: unsafe extern "C" fn(Ptr) = api.f(idx::RELEASE_TENSOR_TYPE_AND_SHAPE_INFO);
            let shape = (|| -> Result<(c_int, Vec<i64>)> {
                let mut ty: c_int = 0;
                let et: unsafe extern "C" fn(Ptr, *mut c_int) -> Status = api.f(idx::GET_TENSOR_ELEMENT_TYPE);
                api.check(et(info, &mut ty))?;
                let mut n: usize = 0;
                let dc: unsafe extern "C" fn(Ptr, *mut usize) -> Status = api.f(idx::GET_DIMENSIONS_COUNT);
                api.check(dc(info, &mut n))?;
                let mut dims = vec![0i64; n];
                let gd: unsafe extern "C" fn(Ptr, *mut i64, usize) -> Status = api.f(idx::GET_DIMENSIONS);
                api.check(gd(info, dims.as_mut_ptr(), n))?;
                Ok((ty, dims))
            })();
            release_info(info);
            let (ty, dims) = shape?;
            let count = dims.iter().product::<i64>().max(0) as usize;
            let mut p: Ptr = std::ptr::null_mut();
            let md: unsafe extern "C" fn(Ptr, *mut Ptr) -> Status = api.f(idx::GET_TENSOR_MUTABLE_DATA);
            api.check(md(v, &mut p))?;
            match ty {
                ELEM_FLOAT => Ok(OrtTensor::F32 { dims, data: std::slice::from_raw_parts(p.cast::<f32>(), count).to_vec() }),
                ELEM_DOUBLE => Ok(OrtTensor::F64 { dims, data: std::slice::from_raw_parts(p.cast::<f64>(), count).to_vec() }),
                ELEM_INT64 => Ok(OrtTensor::I64 { dims, data: std::slice::from_raw_parts(p.cast::<i64>(), count).to_vec() }),
                other => Err(Error::Onnx(format!("output element type {other}"))),
            }
        }
    }
}

impl Drop for OrtSession {
    fn drop(&mut self) {
        // SAFETY: created by CreateSession.
        unsafe {
            let r: unsafe extern "C" fn(Ptr) = self.api.f(idx::RELEASE_SESSION);
            r(self.session);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Needs the installed rekordbox; skipped without it.
    #[test]
    fn runs_the_embedding_models() {
        if !available() {
            eprintln!("skipped: ONNX Runtime unavailable");
            return;
        }
        let cnn = OrtSession::open(OnnxModel::CnnEmbedding, 2).unwrap();
        let n = 463_872;
        let signal = OrtTensor::F32 { dims: vec![1, n], data: (0..n).map(|i| ((i as f32) * 0.01).sin() * 0.1).collect() };
        let i64s = |v: Vec<i64>| OrtTensor::I64 { dims: vec![v.len() as i64], data: v };
        let out = cnn
            .run(
                &[("signal", &signal), ("frame_length", &i64s(vec![512])), ("frame_step", &i64s(vec![256])), ("pads", &i64s(vec![0, 256, 0, 256]))],
                &["output", "features"],
            )
            .unwrap();
        let OrtTensor::F32 { dims, .. } = &out[0] else { panic!("tags") };
        assert_eq!(dims, &[1, 50]);
        let OrtTensor::F32 { dims, data } = &out[1] else { panic!("features") };
        assert_eq!(dims, &[1, 64]);
        let norm = OrtSession::open(OnnxModel::NormBeforeUmap, 1).unwrap();
        let x = OrtTensor::F32 { dims: vec![1, 64], data: data.clone() };
        let OrtTensor::F32 { dims, .. } = &norm.run(&[("input_features", &x)], &["normalized_features"]).unwrap()[0] else { panic!() };
        assert_eq!(dims, &[1, 64]);
    }

    /// The auto-cue models as `CuePointPredictor` runs them; needs the installed rekordbox.
    #[test]
    fn runs_the_cue_models() {
        if !available() {
            eprintln!("skipped: ONNX Runtime unavailable");
            return;
        }
        let cnn = OrtSession::open_cue(OnnxModel::SampleCnn).unwrap();
        let n = 59_049;
        let wave = OrtTensor::F32 { dims: vec![2, n], data: (0..2 * n).map(|i| ((i as f32) * 0.01).sin() * 0.1).collect() };
        let OrtTensor::F32 { dims, data } = &cnn.run(&[("waveform", &wave)], &["features"]).unwrap()[0] else { panic!("features") };
        assert_eq!(dims, &[2, 512]);
        let reg = OrtSession::open_cue(OnnxModel::CuePrediction).unwrap();
        let mut row = vec![0.0f64; 13];
        row[0] = 1.0;
        row[10] = 1.0;
        row.extend(data[..512].iter().cycle().take(3072).map(|&v| f64::from(v)));
        let x = OrtTensor::F64 { dims: vec![1, row.len() as i64], data: row };
        // a tree ensemble: it reads feature 3084, the last of the 13 one-hot values and 6 × 512 CNN features
        let OrtTensor::F32 { dims, .. } = &reg.run(&[("input", &x)], &["variable"]).unwrap()[0] else { panic!("score") };
        assert_eq!(dims, &[1, 1]);
    }
}
