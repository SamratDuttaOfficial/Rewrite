//! Parity fixtures: what the earlier Python version returned for the corpus in `tests/corpus`, recorded in
//! `tests/fixtures/<group>.json` before it was removed. The Rust code is tested by running it on each
//! recorded call and comparing the result with the recorded one.
//!
//! Values are JSON as Python produced them: tuples are arrays, bytes are `{"b64": ...}` (see [`normalize`]),
//! offsets count characters (see [`char_to_byte`]), and a call that raised is `{"error": "Type: message"}`.

use base64::Engine as _;
use serde::de::DeserializeOwned;
use serde::Serialize;
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// One recorded call.
#[derive(Clone, Debug, serde::Deserialize)]
pub struct Case {
    pub args: Vec<Value>,
    pub out: Value,
}

impl Case {
    /// Argument `i` converted to a Rust type (bytes are normalised first).
    pub fn arg<T: DeserializeOwned>(&self, i: usize) -> T {
        let v = normalize(self.args.get(i).cloned().unwrap_or(Value::Null));
        serde_json::from_value(v.clone()).unwrap_or_else(|e| panic!("argument {i} ({v}) does not convert: {e}"))
    }

    pub fn str(&self, i: usize) -> &str {
        self.args[i].as_str().unwrap_or_else(|| panic!("argument {i} is not a string"))
    }

    /// The recorded error message, when the Python call raised.
    pub fn error(&self) -> Option<&str> {
        self.out.as_object().filter(|o| o.len() == 1)?.get("error")?.as_str()
    }
}

/// All recorded calls of one group.
#[derive(Debug, serde::Deserialize)]
pub struct Fixture {
    pub group: String,
    /// The commit the fixtures were recorded from.
    pub source: String,
    cases: BTreeMap<String, Vec<Case>>,
}

impl Fixture {
    /// Loads `tests/fixtures/<group>.json`, or panics with how to make it.
    pub fn load(group: &str) -> Fixture {
        Self::try_load(group).unwrap_or_else(|| {
            panic!("tests/fixtures/{group}.json is missing")
        })
    }

    pub fn try_load(group: &str) -> Option<Fixture> {
        let path = root().join("tests").join("fixtures").join(format!("{group}.json"));
        let text = std::fs::read_to_string(path).ok()?;
        Some(serde_json::from_str(&text).unwrap_or_else(|e| panic!("tests/fixtures/{group}.json: {e}")))
    }

    /// The recorded calls of one function; panics when there are none, so a renamed function is noticed.
    pub fn cases(&self, function: &str) -> &[Case] {
        match self.cases.get(function) {
            Some(c) if !c.is_empty() => c,
            _ => panic!("{}: no recorded calls of {function}", self.group),
        }
    }

    pub fn functions(&self) -> impl Iterator<Item = &str> {
        self.cases.keys().map(String::as_str)
    }

    /// Runs `f` on every recorded call of `function` and compares with the recorded result (numbers to
    /// within 1e-6). Panics listing the first mismatches.
    pub fn check<F: FnMut(&Case) -> Value>(&self, function: &str, f: F) {
        self.check_within(function, 1e-6, f)
    }

    pub fn check_within<F: FnMut(&Case) -> Value>(&self, function: &str, tolerance: f64, mut f: F) {
        let cases = self.cases(function);
        let mut failures = Vec::new();
        for (n, case) in cases.iter().enumerate() {
            let got = normalize(f(case));
            let want = normalize(case.out.clone());
            if let Some(path) = first_difference(&want, &got, tolerance, String::new()) {
                failures.push(format!(
                    "#{n} args {}\n    at {path}\n    want {}\n    got  {}",
                    short(&Value::Array(case.args.clone()), 300),
                    short(&want, 600),
                    short(&got, 600)
                ));
            }
        }
        if !failures.is_empty() {
            let shown: Vec<_> = failures.iter().take(8).cloned().collect();
            panic!(
                "{}::{function}: {} of {} calls differ from Python (recorded at {})\n{}",
                self.group,
                failures.len(),
                cases.len(),
                self.source,
                shown.join("\n")
            );
        }
    }
}

/// A Rust value as JSON, for returning from a check closure.
pub fn json<T: Serialize>(value: T) -> Value {
    serde_json::to_value(value).expect("value converts to JSON")
}

/// The repository root.
pub fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..").join("..")
}

/// The folder holding the downloaded models (`<repo>/models` when running from source, else `~/.rewrite/models`),
/// if all three are there. Tests that need them are skipped without.
pub fn models_dir() -> Option<PathBuf> {
    let home = std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME")).map(PathBuf::from);
    let candidates = [Some(root().join("models")), home.map(|h| h.join(".rewrite").join("models"))];
    candidates.into_iter().flatten().find(|d| {
        ["context/model.onnx", "similarity/model.onnx", "paraphrase/encoder.onnx", "paraphrase/decoder.onnx"]
            .iter()
            .all(|f| d.join(f).is_file())
    })
}

/// A folder in the system temp folder that is deleted when the value is dropped, so a test leaves nothing behind
/// whether it passes or panics.
pub struct TempDir(PathBuf);

impl TempDir {
    /// A fresh, empty folder for one test. The name holds the process ID, so parallel test runs don't share it.
    pub fn new(name: &str) -> TempDir {
        let dir = std::env::temp_dir().join(format!("rewrite-test-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap_or_else(|e| panic!("cannot create {}: {e}", dir.display()));
        TempDir(dir)
    }

    /// Takes charge of a folder that the code under test creates, such as a LaTeX build folder.
    pub fn adopt(dir: PathBuf) -> TempDir {
        TempDir(dir)
    }
}

impl std::ops::Deref for TempDir {
    type Target = Path;

    fn deref(&self) -> &Path {
        &self.0
    }
}

impl AsRef<Path> for TempDir {
    fn as_ref(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Bytes recorded as `{"b64": ...}` become arrays of numbers, the form serde uses for byte buffers.
pub fn normalize(v: Value) -> Value {
    match v {
        Value::Object(o) => {
            if o.len() == 1 {
                if let Some(Value::String(s)) = o.get("b64") {
                    let bytes = base64::engine::general_purpose::STANDARD.decode(s).expect("valid base64");
                    return Value::Array(bytes.into_iter().map(Value::from).collect());
                }
            }
            Value::Object(o.into_iter().map(|(k, v)| (k, normalize(v))).collect())
        }
        Value::Array(a) => Value::Array(a.into_iter().map(normalize).collect()),
        v => v,
    }
}

/// Where two values first differ (a JSON-path-like string), or None when they match. Integers and floats
/// compare by value.
pub fn first_difference(want: &Value, got: &Value, tolerance: f64, path: String) -> Option<String> {
    let here = || if path.is_empty() { "$".to_string() } else { path.clone() };
    match (want, got) {
        (Value::Number(a), Value::Number(b)) => {
            let (a, b) = (a.as_f64()?, b.as_f64()?);
            ((a - b).abs() > tolerance * a.abs().max(1.0)).then(here)
        }
        (Value::Array(a), Value::Array(b)) => {
            if a.len() != b.len() {
                return Some(format!("{} (length {} vs {})", here(), a.len(), b.len()));
            }
            a.iter().zip(b).enumerate().find_map(|(i, (x, y))| first_difference(x, y, tolerance, format!("{path}[{i}]")))
        }
        (Value::Object(a), Value::Object(b)) => {
            if let Some(k) = a.keys().find(|k| !b.contains_key(*k)).or_else(|| b.keys().find(|k| !a.contains_key(*k))) {
                return Some(format!("{}.{k} (key only on one side)", here()));
            }
            a.iter().find_map(|(k, x)| first_difference(x, &b[k], tolerance, format!("{path}.{k}")))
        }
        (a, b) => (a != b).then(here),
    }
}

/// Lines `got` up with `want` where only the order of tied items may differ (Python broke some ties in set
/// order, Rust in a fixed one). Both lists are split into runs of consecutive items with the same `key` (a
/// score, a sense); where a run of `got` holds the same items as the matching run of `want`, in any order, it
/// takes `want`'s order. With `truncated` (the list was cut at a limit, so the last run may hold different
/// tied items), a last run of the same key and length is taken as is. Everything else is left alone, so a
/// real difference still shows when the result is compared.
pub fn align_ties(want: &Value, got: Value, key: impl Fn(&Value) -> Value, truncated: bool) -> Value {
    let (Some(w), Value::Array(mut g)) = (want.as_array(), got.clone()) else { return got };
    let runs = |xs: &[Value]| {
        let mut out: Vec<(usize, usize)> = Vec::new();
        for i in 0..xs.len() {
            match out.last_mut() {
                Some(r) if key(&xs[r.0]) == key(&xs[i]) => r.1 = i + 1,
                _ => out.push((i, i + 1)),
            }
        }
        out
    };
    let (rw, rg) = (runs(w), runs(&g));
    for (n, (&(ws, we), &(gs, ge))) in rw.iter().zip(&rg).enumerate() {
        if ws != gs || we != ge || key(&w[ws]) != key(&g[gs]) {
            break;
        }
        let last = n + 1 == rw.len() && n + 1 == rg.len();
        // the same items in any order (numbers to within 1e-6, as `check` compares)
        let mut unused: Vec<&Value> = g[gs..ge].iter().collect();
        let same = w[ws..we].iter().all(|x| {
            let hit = unused.iter().position(|y| first_difference(x, y, 1e-6, String::new()).is_none());
            hit.map(|i| unused.swap_remove(i)).is_some()
        });
        if same || (last && truncated) {
            g[gs..ge].clone_from_slice(&w[ws..we]);
        }
    }
    Value::Array(g)
}

fn short(v: &Value, max: usize) -> String {
    let s = v.to_string();
    if s.chars().count() <= max {
        return s;
    }
    let cut: String = s.chars().take(max).collect();
    format!("{cut}… ({} chars)", s.chars().count())
}

/// Byte offset of character offset `i` in `s` (Python offsets count characters).
pub fn char_to_byte(s: &str, i: usize) -> usize {
    s.char_indices().nth(i).map_or(s.len(), |(b, _)| b)
}

/// Character offset of byte offset `b` in `s`.
pub fn byte_to_char(s: &str, b: usize) -> usize {
    s[..b].chars().count()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn numbers_compare_by_value() {
        assert!(first_difference(&json!([1, 2.0]), &json!([1.0, 2]), 1e-6, String::new()).is_none());
        assert!(first_difference(&json!({"a": 0.5}), &json!({"a": 0.6}), 1e-6, String::new()).is_some());
        assert_eq!(first_difference(&json!({"a": [1, 2]}), &json!({"a": [1, 3]}), 0.0, String::new()).unwrap(), ".a[1]");
    }

    #[test]
    fn ties_line_up() {
        let score = |v: &Value| v[1].clone();
        let want = json!([["a", 3], ["b", 2], ["c", 2], ["d", 1], ["e", 1]]);
        let got = json!([["a", 3], ["c", 2], ["b", 2], ["e", 1], ["f", 1]]);
        assert_eq!(align_ties(&want, got.clone(), score, true), want);
        let fixed = align_ties(&want, got, score, false);
        assert_eq!(fixed[1], json!(["b", 2]));
        assert_eq!(fixed[4], json!(["f", 1]), "an untruncated last run must hold the same items");
        let other = json!([["a", 3], ["x", 2], ["b", 2]]);
        assert_eq!(align_ties(&want, other.clone(), score, true), other);
    }

    #[test]
    fn bytes_become_arrays() {
        assert_eq!(normalize(json!({"b64": "AQI="})), json!([1, 2]));
    }

    #[test]
    fn offsets() {
        let s = "aé€b";
        assert_eq!(char_to_byte(s, 3), 6);
        assert_eq!(byte_to_char(s, 6), 3);
        assert_eq!(char_to_byte(s, 9), s.len());
    }

    #[test]
    fn every_fixture_parses() {
        let dir = root().join("tests").join("fixtures");
        let mut n = 0;
        for entry in std::fs::read_dir(dir).expect("tests/fixtures exists") {
            let path = entry.unwrap().path();
            if path.extension().is_some_and(|e| e == "json") {
                let group = path.file_stem().unwrap().to_string_lossy().into_owned();
                let f = Fixture::load(&group);
                assert_eq!(f.group, group);
                assert!(f.functions().count() > 0, "{group} has no functions");
                n += 1;
            }
        }
        assert!(n > 0, "no fixtures recorded");
    }
}
