//! Converts `assets/thesaurus.json.gz` into a compact binary that the library embeds and reads without parsing
//! JSON (see `src/thesaurus.rs` for the layout).

use flate2::read::GzDecoder;
use serde_json::Value;
use std::collections::HashMap;
use std::io::Read;
use std::path::PathBuf;

/// Unique strings, numbered in first-use order.
#[derive(Default)]
struct Strings {
    ids: HashMap<String, u32>,
    blob: Vec<u8>,
    ends: Vec<u32>,
}

impl Strings {
    fn id(&mut self, s: &str) -> u32 {
        if let Some(&id) = self.ids.get(s) {
            return id;
        }
        let id = self.ends.len() as u32;
        self.blob.extend_from_slice(s.as_bytes());
        self.ends.push(self.blob.len() as u32);
        self.ids.insert(s.to_string(), id);
        id
    }
}

fn put(out: &mut Vec<u8>, n: usize) {
    out.extend_from_slice(&u32::try_from(n).expect("fits in u32").to_le_bytes());
}

fn strs(v: &Value) -> Vec<&str> {
    v.as_array().expect("array").iter().map(|x| x.as_str().expect("string")).collect()
}

fn main() {
    let src = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap()).join("../../assets/thesaurus.json.gz");
    println!("cargo:rerun-if-changed={}", src.display());
    let mut text = String::new();
    GzDecoder::new(std::fs::File::open(&src).expect("assets/thesaurus.json.gz")).read_to_string(&mut text).unwrap();
    let d: Value = serde_json::from_str(&text).expect("thesaurus JSON");

    let mut st = Strings::default();
    let mut body = Vec::new();
    let synsets = d["S"].as_array().expect("S");
    put(&mut body, synsets.len());
    for s in synsets {
        let pos = s[0].as_str().expect("pos");
        assert!(matches!(pos, "n" | "v" | "a" | "r"), "unknown part of speech {pos}");
        body.push(pos.as_bytes()[0]);
        let lemmas = strs(&s[1]);
        put(&mut body, lemmas.len());
        for l in lemmas {
            let id = st.id(l);
            put(&mut body, id as usize);
        }
        let gloss = st.id(s[2].as_str().expect("gloss"));
        put(&mut body, gloss as usize);
        let rel = s[3].as_array().expect("rel");
        put(&mut body, rel.len());
        for r in rel {
            put(&mut body, r.as_u64().expect("synset id") as usize);
        }
    }
    let index = d["I"].as_object().expect("I");
    put(&mut body, index.len());
    for (w, sids) in index {
        let id = st.id(w);
        put(&mut body, id as usize);
        let sids = sids.as_array().expect("synset ids");
        put(&mut body, sids.len());
        for s in sids {
            put(&mut body, s.as_u64().expect("synset id") as usize);
        }
    }
    let ants = d["A"].as_object().expect("A");
    put(&mut body, ants.len());
    for (w, list) in ants {
        let id = st.id(w);
        put(&mut body, id as usize);
        let list = strs(list);
        put(&mut body, list.len());
        for a in list {
            let id = st.id(a);
            put(&mut body, id as usize);
        }
    }
    let freq = d["F"].as_object().expect("F");
    put(&mut body, freq.len());
    for (w, f) in freq {
        let id = st.id(w);
        put(&mut body, id as usize);
        put(&mut body, f.as_u64().expect("integer frequency") as usize);
    }

    let mut out = b"THS1".to_vec();
    put(&mut out, st.ends.len());
    for e in &st.ends {
        put(&mut out, *e as usize);
    }
    put(&mut out, st.blob.len());
    out.extend_from_slice(&st.blob);
    out.extend_from_slice(&body);
    let dest = PathBuf::from(std::env::var("OUT_DIR").unwrap()).join("thesaurus.bin");
    std::fs::write(dest, out).unwrap();
}
