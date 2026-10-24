//! settings.json: the same file and keys the app has always used, so settings carry over.

use rewrite_core::paths;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::sync::Mutex;

pub const DEFAULT_FONT_SIZE: f64 = 11.0;

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(default)]
pub struct Settings {
    pub version: u32,
    pub theme: String,
    pub font_size: f64,
    pub font_family: String,
    pub first_run_done: bool,
    pub phrase_hints: bool,
    pub show_changes: bool,
    pub recent: Vec<String>,
    pub geometry: String,
    pub spell_check: bool,
    pub grammar_check: bool,
    pub user_words: Vec<String>,
    pub preview: bool,
    pub preview_split: f64,
    pub find_case: bool,
    pub find_word: bool,
    pub find_regex: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_dir: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub export_target: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub export_paper: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub export_font: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub export_sentences: Option<bool>,
    /// Keys this version does not know, kept as they were.
    #[serde(flatten)]
    pub other: Map<String, Value>,
}

impl Default for Settings {
    fn default() -> Settings {
        Settings {
            version: 2,
            theme: "light".into(),
            font_size: DEFAULT_FONT_SIZE,
            font_family: String::new(),
            first_run_done: false,
            phrase_hints: true,
            show_changes: true,
            recent: vec![],
            geometry: String::new(),
            spell_check: true,
            grammar_check: true,
            user_words: vec![],
            preview: false,
            preview_split: 0.5,
            find_case: false,
            find_word: false,
            find_regex: false,
            last_dir: None,
            export_target: None,
            export_paper: None,
            export_font: None,
            export_sentences: None,
            other: Map::new(),
        }
    }
}

static WRITE: Mutex<()> = Mutex::new(());

pub fn load() -> Settings {
    let path = paths::settings_file();
    let Ok(text) = std::fs::read_to_string(&path) else { return Settings::default() };
    let Ok(mut raw) = serde_json::from_str::<Map<String, Value>>(&text) else { return Settings::default() };
    let version = raw.get("version").and_then(Value::as_u64).unwrap_or(1);
    if version < 2 && raw.get("font_size").and_then(Value::as_f64) == Some(13.0) {
        raw.remove("font_size"); // the old default size: take the new, smaller one
    }
    raw.insert("version".into(), Value::from(2));
    // tolerate keys of the wrong type (an old or hand-edited file): drop them and use the default
    let defaults = serde_json::to_value(Settings::default()).unwrap_or_default();
    let mut clean = Map::new();
    for (k, v) in raw {
        let ok = match defaults.get(&k) {
            Some(d) => same_kind(d, &v),
            None => true,
        };
        if ok {
            clean.insert(k, v);
        }
    }
    // a list with a wrong item keeps its good items
    for key in ["recent", "user_words"] {
        if let Some(Value::Array(items)) = clean.get_mut(key) {
            items.retain(Value::is_string);
        }
    }
    serde_json::from_value(Value::Object(clean)).unwrap_or_default()
}

fn same_kind(a: &Value, b: &Value) -> bool {
    matches!(
        (a, b),
        (Value::Bool(_), Value::Bool(_))
            | (Value::Number(_), Value::Number(_))
            | (Value::String(_), Value::String(_))
            | (Value::Array(_), Value::Array(_))
            | (Value::Object(_), Value::Object(_))
            | (Value::Null, _)
    )
}

/// Write the settings; errors are ignored (a read-only folder must not stop the app).
pub fn save(s: &Settings) {
    let _g = WRITE.lock();
    let path = paths::settings_file();
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Ok(text) = serde_json::to_string_pretty(s) {
        let tmp = path.with_extension("json.rewrite-tmp");
        if std::fs::write(&tmp, text).is_ok() {
            let _ = std::fs::rename(&tmp, &path);
        }
    }
}
