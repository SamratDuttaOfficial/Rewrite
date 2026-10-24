//! ONNX Runtime sessions, set up as `rewrite/nlp/onnx_util.py` did, plus the small array helpers the models share.
//!
//! The runtime is loaded at run time from the library shipped with Rewrite (see
//! [`rewrite_core::paths::runtime_library`]), by absolute path, so an older `onnxruntime.dll` that Windows keeps in
//! System32 is never picked up. `ort` refuses a library older than ONNX Runtime 1.30.

use ort::environment::Environment;
use ort::logging::LogLevel;
use ort::session::builder::GraphOptimizationLevel;
use ort::session::SessionInputValue;
use ort::value::{DynValue, Tensor, ValueType};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

/// File name of the ONNX Runtime library on this system.
pub const LIBRARY: &str = if cfg!(windows) {
    "onnxruntime.dll"
} else if cfg!(target_os = "macos") {
    "libonnxruntime.dylib"
} else {
    "libonnxruntime.so"
};

/// The shipped ONNX Runtime library, if it is there.
pub fn library() -> Option<PathBuf> {
    rewrite_core::paths::runtime_library(LIBRARY)
}

/// Loads ONNX Runtime (once per process) and returns its environment.
pub fn environment() -> Result<&'static Environment, String> {
    static ENV: OnceLock<Result<Environment, String>> = OnceLock::new();
    ENV.get_or_init(|| {
        let lib = library().ok_or_else(|| format!("{LIBRARY} was not found next to the program"))?;
        ort::init_from(&lib)
            .map_err(|e| format!("{}: {e}", lib.display()))?
            .with_name("rewrite")
            .with_telemetry(false)
            .build()
            .map_err(|e| e.to_string())
    })
    .as_ref()
    .map_err(Clone::clone)
}

/// Whether ONNX Runtime can be loaded.
pub fn available() -> bool {
    environment().is_ok()
}

/// Intra-op threads: half the logical CPUs, 1 to 8.
pub fn threads() -> usize {
    (std::thread::available_parallelism().map_or(2, |n| n.get()) / 2).clamp(1, 8)
}

/// One loaded model. `ort` needs exclusive access to run a session, so calls are serialised by a lock.
pub struct Session {
    inner: Mutex<ort::session::Session>,
    inputs: Vec<String>,
    outputs: Vec<String>,
    /// Shapes of the inputs as the model declares them (-1 for a dynamic dimension).
    shapes: Vec<Vec<i64>>,
}

impl Session {
    /// Loads a model with full graph optimisation on the CPU, logging errors only.
    pub fn load(path: &Path) -> Result<Session, String> {
        let env = environment()?;
        let fail = |e: &dyn std::fmt::Display| format!("{}: {e}", path.display());
        let session = ort::session::Session::builder(env)
            .map_err(|e| fail(&e))?
            .with_intra_threads(threads())
            .and_then(|b| b.with_optimization_level(GraphOptimizationLevel::All))
            .and_then(|b| b.with_log_level(LogLevel::Error))
            .map_err(|e| fail(&e))?
            .commit_from_file(path)
            .map_err(|e| fail(&e))?;
        let inputs = session.inputs().iter().map(|i| i.name().to_string()).collect();
        let outputs = session.outputs().iter().map(|o| o.name().to_string()).collect();
        let shapes = session
            .inputs()
            .iter()
            .map(|i| match i.dtype() {
                ValueType::Tensor { shape, .. } => shape.to_vec(),
                _ => Vec::new(),
            })
            .collect();
        Ok(Session { inner: Mutex::new(session), inputs, outputs, shapes })
    }

    pub fn inputs(&self) -> &[String] {
        &self.inputs
    }

    pub fn outputs(&self) -> &[String] {
        &self.outputs
    }

    pub fn has_input(&self, name: &str) -> bool {
        self.inputs.iter().any(|n| n == name)
    }

    /// The declared shape of an input (-1 for dynamic dimensions).
    pub fn input_shape(&self, name: &str) -> Option<&[i64]> {
        self.inputs.iter().position(|n| n == name).map(|i| self.shapes[i].as_slice())
    }

    /// Runs the model on named inputs and returns all outputs, in the model's output order.
    pub fn run(&self, inputs: Vec<(&str, SessionInputValue<'_>)>) -> Result<Vec<DynValue>, String> {
        let mut session = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let inputs: Vec<_> = inputs.into_iter().map(|(n, v)| (std::borrow::Cow::Borrowed(n), v)).collect();
        let mut out = session.run(inputs).map_err(|e| e.to_string())?;
        Ok(self.outputs.iter().map(|n| out.remove(n.as_str()).expect("every output is returned")).collect())
    }
}

/// An int64 tensor.
pub fn tensor_i64(shape: &[usize], data: Vec<i64>) -> Result<DynValue, String> {
    Ok(Tensor::from_array((shape.to_vec(), data)).map_err(|e| e.to_string())?.into_dyn())
}

/// A float32 tensor.
pub fn tensor_f32(shape: &[usize], data: Vec<f32>) -> Result<DynValue, String> {
    Ok(Tensor::from_array((shape.to_vec(), data)).map_err(|e| e.to_string())?.into_dyn())
}

/// A bool tensor.
pub fn tensor_bool(shape: &[usize], data: Vec<bool>) -> Result<DynValue, String> {
    Ok(Tensor::from_array((shape.to_vec(), data)).map_err(|e| e.to_string())?.into_dyn())
}

/// A float32 output as its shape and flat data.
pub fn floats(value: &DynValue) -> Result<(Vec<usize>, &[f32]), String> {
    let (shape, data) = value.try_extract_tensor::<f32>().map_err(|e| e.to_string())?;
    Ok((shape.iter().map(|&d| d as usize).collect(), data))
}

/// Log-softmax over one row of logits, in f32 as numpy did. The sum is taken in f64: a plain f32 sum over 50 000
/// values drifts by 1e-4, while numpy's pairwise sum does not.
pub fn log_softmax(x: &[f32]) -> Vec<f32> {
    let max = x.iter().copied().fold(f32::NEG_INFINITY, f32::max);
    let sum: f64 = x.iter().map(|&v| (v - max).exp() as f64).sum();
    let log_sum = sum.ln() as f32;
    x.iter().map(|&v| v - max - log_sum).collect()
}

/// Right-pads token sequences to the longest one: (ids, attention mask, width), both row-major.
pub fn pad(seqs: &[Vec<i64>], pad_id: i64) -> (Vec<i64>, Vec<i64>, usize) {
    let width = seqs.iter().map(Vec::len).max().unwrap_or(0);
    let mut ids = vec![pad_id; seqs.len() * width];
    let mut mask = vec![0; seqs.len() * width];
    for (i, s) in seqs.iter().enumerate() {
        ids[i * width..i * width + s.len()].copy_from_slice(s);
        mask[i * width..i * width + s.len()].fill(1);
    }
    (ids, mask, width)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn log_softmax_normalises() {
        let lp = log_softmax(&[1.0, 2.0, 3.0]);
        let total: f32 = lp.iter().map(|v| v.exp()).sum();
        assert!((total - 1.0).abs() < 1e-6);
        assert!((lp[2] - -0.407_606).abs() < 1e-5);
    }

    #[test]
    fn pads_right() {
        let (ids, mask, w) = pad(&[vec![5, 6, 7], vec![8]], 1);
        assert_eq!(w, 3);
        assert_eq!(ids, [5, 6, 7, 8, 1, 1]);
        assert_eq!(mask, [1, 1, 1, 1, 0, 0]);
    }

    #[test]
    fn threads_in_range() {
        assert!((1..=8).contains(&threads()));
    }
}
