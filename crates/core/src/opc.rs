//! Zip packages (Office Open XML and OpenDocument): reading parts and relationships.

use std::collections::{BTreeMap, HashMap};
use std::io::{Cursor, Read, Write};

pub const STRICT: [(&str, &str); 7] = [
    ("http://purl.oclc.org/ooxml/wordprocessingml/main", "http://schemas.openxmlformats.org/wordprocessingml/2006/main"),
    ("http://purl.oclc.org/ooxml/officeDocument/relationships", "http://schemas.openxmlformats.org/officeDocument/2006/relationships"),
    ("http://purl.oclc.org/ooxml/drawingml/main", "http://schemas.openxmlformats.org/drawingml/2006/main"),
    ("http://purl.oclc.org/ooxml/drawingml/wordprocessingDrawing", "http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing"),
    ("http://purl.oclc.org/ooxml/drawingml/picture", "http://schemas.openxmlformats.org/drawingml/2006/picture"),
    ("http://purl.oclc.org/ooxml/drawingml/chart", "http://schemas.openxmlformats.org/drawingml/2006/chart"),
    ("http://purl.oclc.org/ooxml/officeDocument/math", "http://schemas.openxmlformats.org/officeDocument/2006/math"),
];

#[derive(Clone, Debug)]
pub struct Rel {
    pub typ: String,
    pub target: String, // a part name, or the URL of an external target
    pub external: bool,
}

pub struct Package {
    pub data: Vec<u8>,
    pub parts: BTreeMap<String, Vec<u8>>,
}

impl Package {
    pub fn open(data: &[u8]) -> Result<Package, String> {
        let mut z = zip::ZipArchive::new(Cursor::new(data)).map_err(|e| format!("not a valid package: {e}"))?;
        let mut parts = BTreeMap::new();
        for k in 0..z.len() {
            let mut f = z.by_index(k).map_err(|e| e.to_string())?;
            if f.is_dir() {
                continue;
            }
            let mut buf = Vec::with_capacity(f.size() as usize);
            f.read_to_end(&mut buf).map_err(|e| e.to_string())?;
            let name = f.name().map(|n| n.trim_start_matches('/').to_string()).unwrap_or_default();
            parts.insert(name, buf);
        }
        Ok(Package { data: data.to_vec(), parts })
    }

    pub fn get(&self, name: &str) -> Option<&Vec<u8>> {
        self.parts.get(name.trim_start_matches('/'))
    }

    /// A part as text, with Strict OOXML namespaces mapped to the usual ones.
    pub fn text(&self, name: &str) -> Option<String> {
        let mut s = crate::xml::decode(self.get(name)?);
        for (strict, normal) in STRICT {
            if s.contains(strict) {
                s = s.replace(strict, normal);
            }
        }
        Some(s)
    }

    pub fn rels(&self, part: &str) -> HashMap<String, Rel> {
        let (dir, base) = match part.rfind('/') {
            Some(i) => (&part[..i], &part[i + 1..]),
            None => ("", part),
        };
        let name = if dir.is_empty() { format!("_rels/{base}.rels") } else { format!("{dir}/_rels/{base}.rels") };
        let mut out = HashMap::new();
        let Some(text) = self.text(&name) else { return out };
        let Ok(doc) = roxmltree::Document::parse(&text) else { return out };
        for r in doc.root_element().children().filter(|n| n.is_element()) {
            let target = r.attribute("Target").unwrap_or("").to_string();
            let external = r.attribute("TargetMode") == Some("External");
            let target = if external { target } else if let Some(t) = target.strip_prefix('/') { t.to_string() } else { join(dir, &target) };
            out.insert(r.attribute("Id").unwrap_or("").to_string(), Rel { typ: r.attribute("Type").unwrap_or("").to_string(), target, external });
        }
        out
    }
}

/// A path relative to a folder, with "." and ".." resolved.
pub fn join(dir: &str, rel: &str) -> String {
    let mut parts: Vec<&str> = if dir.is_empty() { vec![] } else { dir.split('/').collect() };
    for p in rel.split('/') {
        match p {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            x => parts.push(x),
        }
    }
    parts.join("/")
}

/// `target` relative to the folder `dir` (both part names).
pub fn relative(target: &str, dir: &str) -> String {
    let t: Vec<&str> = target.split('/').collect();
    let d: Vec<&str> = if dir.is_empty() { vec![] } else { dir.split('/').collect() };
    let common = t.iter().zip(&d).take_while(|(a, b)| a == b).count();
    let mut out: Vec<&str> = vec![".."; d.len() - common];
    out.extend(&t[common..]);
    out.join("/")
}

/// A zip file from (name, data) pairs; `stored` names are written without compression (ODF's mimetype).
pub fn zip(files: &[(String, Vec<u8>)], stored: &[&str]) -> Vec<u8> {
    let mut buf = Cursor::new(Vec::new());
    {
        let mut z = zip::ZipWriter::new(&mut buf);
        for (name, data) in files {
            let method = if stored.contains(&name.as_str()) { zip::CompressionMethod::Stored } else { zip::CompressionMethod::Deflated };
            let opts = zip::write::SimpleFileOptions::default().compression_method(method);
            let _ = z.start_file(name.as_str(), opts);
            let _ = z.write_all(data);
        }
        let _ = z.finish();
    }
    buf.into_inner()
}
