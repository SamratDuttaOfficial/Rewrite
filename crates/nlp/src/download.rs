//! The optional models: where they live, whether they are present, and downloading them (was `rewrite/models.py`).
//!
//! All models are ONNX files fetched once from Hugging Face at pinned revisions and then used fully offline. A
//! download goes to `<name>.part` and resumes with an HTTP Range request; the finished file must have the expected
//! size and SHA-256 before it replaces `<name>`.

use sha2::{Digest, Sha256};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

/// One downloadable model.
#[derive(Debug)]
pub struct ModelSpec {
    pub key: &'static str,
    pub title: &'static str,
    pub detail: &'static str,
    /// Hugging Face repository and pinned revision.
    pub repo: &'static str,
    pub rev: &'static str,
    pub files: &'static [ModelFile],
}

/// One file of a model: its local name, its path in the repository, size in bytes and SHA-256 (hex).
#[derive(Debug)]
pub struct ModelFile {
    pub name: &'static str,
    pub remote: &'static str,
    pub size: u64,
    pub sha256: &'static str,
}

const fn file(name: &'static str, remote: &'static str, size: u64, sha256: &'static str) -> ModelFile {
    ModelFile { name, remote, size, sha256 }
}

pub const MODELS: [ModelSpec; 3] = [
    ModelSpec {
        key: "context",
        title: "Context model",
        detail: "RoBERTa-base · 125M params · ranks words and phrases by how well they fit the sentence",
        repo: "Xenova/roberta-base",
        rev: "bb432e3f5502192ed482fae6a2bb0124ef61c51d",
        files: &[
            file("tokenizer.json", "tokenizer.json", 2108619,
                "1649e42e2e4cc707159be564b62216cd3c21a0225fad65b04bed94477dd79083"),
            file("model.onnx", "onnx/model_quantized.onnx", 126111809,
                "7aea99df87679b6efdece225e4d59cc1d72155d81a7ee13cb713ef2d8e7cfa5c"),
        ],
    },
    ModelSpec {
        key: "similarity",
        title: "Meaning check",
        detail: "MiniLM-L6 · 22M params · verifies a rewrite keeps the original meaning",
        repo: "Xenova/all-MiniLM-L6-v2",
        rev: "751bff37182d3f1213fa05d7196b954e230abad9",
        files: &[
            file("tokenizer.json", "tokenizer.json", 711661,
                "da0e79933b9ed51798a3ae27893d3c5fa4a201126cef75586296df9b4d2c62a0"),
            file("model.onnx", "onnx/model_quantized.onnx", 22972370,
                "afdb6f1a0e45b715d0bb9b11772f032c399babd23bfc31fed1c170afc848bdb1"),
        ],
    },
    ModelSpec {
        key: "paraphrase",
        title: "Sentence paraphraser",
        detail: "T5-base · 220M params · rewrites whole sentences and phrases",
        repo: "Felladrin/onnx-chatgpt_paraphraser_on_T5_base",
        rev: "4ec698d13cdf0a9d8d1315209b200f62346e7ed1",
        files: &[
            file("tokenizer.json", "tokenizer.json", 2422444,
                "a0ecce9cf06797c397257b34a01319f60493dcdbc27fdb6823c04766f8757596"),
            file("config.json", "config.json", 1642,
                "2a596850bc9a89d8b1b93470db3af99d3cfc02c03b9f92303fb070de568e9096"),
            file("encoder.onnx", "onnx/encoder_model_quantized.onnx", 110388422,
                "bfc9eb15622dc951db6fde1816c58eab00c21f94dc1e117c0178a27db2b19bf7"),
            file("decoder.onnx", "onnx/decoder_model_merged_quantized.onnx", 139724514,
                "9a0ca40e2650380dd9fdeb2cde86623237d3c44b433779899855b7dcc83d520f"),
        ],
    },
];

/// A model by key; panics on an unknown key (a programming error, as the Python `KeyError` was).
pub fn spec(key: &str) -> &'static ModelSpec {
    MODELS.iter().find(|m| m.key == key).unwrap_or_else(|| panic!("unknown model {key:?}"))
}

/// The folder a model lives in.
pub fn folder(key: &str) -> PathBuf {
    rewrite_core::paths::models_dir().join(spec(key).key)
}

/// Total download size in megabytes (10^6 bytes), rounded.
pub fn size_mb(key: &str) -> u64 {
    let bytes: u64 = spec(key).files.iter().map(|f| f.size).sum();
    (bytes as f64 / 1e6).round() as u64
}

/// Whether every file of a model is there with the right size (sizes only: hashing at every start-up is slow).
pub fn present(key: &str) -> bool {
    has_files(&folder(key), spec(key))
}

/// [`present`] for a model under another models folder.
pub fn present_in(models_dir: &Path, key: &str) -> bool {
    has_files(&models_dir.join(key), spec(key))
}

fn has_files(dir: &Path, spec: &ModelSpec) -> bool {
    spec.files.iter().all(|f| fs::metadata(dir.join(f.name)).is_ok_and(|m| m.is_file() && m.len() == f.size))
}

/// Whether the models can run: the ONNX Runtime library loads (the tokenizers are built in).
pub fn runtime_available() -> bool {
    crate::onnx::available()
}

/// Why a download stopped.
#[derive(Debug)]
pub enum DownloadError {
    Cancelled,
    Failed(String),
}

impl std::fmt::Display for DownloadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DownloadError::Cancelled => f.write_str("Download cancelled"),
            DownloadError::Failed(why) => f.write_str(why),
        }
    }
}

impl std::error::Error for DownloadError {}

fn failed(e: impl std::fmt::Display) -> DownloadError {
    DownloadError::Failed(e.to_string())
}

/// Downloads one model into its folder. `progress(done, total)` is called in bytes; setting `cancel` stops the
/// download at the next chunk, keeping the `.part` file so the next attempt resumes.
pub fn download(key: &str, progress: &mut dyn FnMut(u64, u64), cancel: &AtomicBool) -> Result<(), DownloadError> {
    download_into(&folder(key), spec(key), progress, cancel)
}

fn download_into(
    dir: &Path,
    spec: &ModelSpec,
    progress: &mut dyn FnMut(u64, u64),
    cancel: &AtomicBool,
) -> Result<(), DownloadError> {
    fs::create_dir_all(dir).map_err(|e| failed(format!("{}: {e}", dir.display())))?;
    let total: u64 = spec.files.iter().map(|f| f.size).sum();
    let mut done = 0;
    let agent = agent();
    for f in spec.files {
        let dest = dir.join(f.name);
        if fs::metadata(&dest).is_ok_and(|m| m.is_file() && m.len() == f.size) {
            done += f.size;
            progress(done, total);
            continue;
        }
        let url = format!("https://huggingface.co/{}/resolve/{}/{}", spec.repo, spec.rev, f.remote);
        fetch(&agent, &url, &dest, f, &mut |got| progress(done + got, total), cancel)?;
        done += f.size;
    }
    Ok(())
}

fn agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .user_agent("rewrite-app/1.0")
        .timeout_connect(Some(Duration::from_secs(60)))
        .timeout_recv_response(Some(Duration::from_secs(60)))
        .timeout_per_read(Some(Duration::from_secs(60)))
        .build()
        .into()
}

const CHUNK: usize = 1 << 18;

/// Downloads `url` to `dest` via `dest.part`, resuming a partial file when the server answers 206, then checks
/// size and hash. `progress(bytes of this file so far)`.
fn fetch(
    agent: &ureq::Agent,
    url: &str,
    dest: &Path,
    file: &ModelFile,
    progress: &mut dyn FnMut(u64),
    cancel: &AtomicBool,
) -> Result<(), DownloadError> {
    let part = part_path(dest);
    let mut have = fs::metadata(&part).map_or(0, |m| m.len());
    if have < file.size {
        let mut req = agent.get(url);
        if have > 0 {
            req = req.header("Range", format!("bytes={have}-"));
        }
        let resp = req.call().map_err(|e| failed(format!("{}: {e}", file.name)))?;
        let append = have > 0 && resp.status().as_u16() == 206;
        if !append {
            have = 0;
        }
        let mut out = OpenOptions::new()
            .create(true)
            .write(true)
            .append(append)
            .truncate(!append)
            .open(&part)
            .map_err(|e| failed(format!("{}: {e}", part.display())))?;
        let mut body = resp.into_body();
        let mut reader = body.as_reader();
        let mut buf = vec![0; CHUNK];
        let mut got = have;
        loop {
            if cancel.load(Ordering::Relaxed) {
                return Err(DownloadError::Cancelled);
            }
            let n = read_chunk(&mut reader, &mut buf).map_err(|e| failed(format!("{}: {e}", file.name)))?;
            if n == 0 {
                break;
            }
            out.write_all(&buf[..n]).map_err(|e| failed(format!("{}: {e}", part.display())))?;
            got += n as u64;
            progress(got);
        }
        out.flush().map_err(failed)?;
    }
    let len = fs::metadata(&part).map_or(0, |m| m.len());
    if len != file.size {
        return Err(failed(format!("{}: incomplete download ({len} of {} bytes)", file.name, file.size)));
    }
    let hash = sha256(&part).map_err(|e| failed(format!("{}: {e}", part.display())))?;
    if hash != file.sha256 {
        let _ = fs::remove_file(&part);
        return Err(failed(format!("{}: the download is damaged (checksum mismatch); please try again", file.name)));
    }
    fs::rename(&part, dest).map_err(|e| failed(format!("{}: {e}", dest.display())))
}

/// Fills `buf` as far as the stream allows (so chunks are 256 KiB, as before), returning the bytes read.
fn read_chunk(r: &mut impl Read, buf: &mut [u8]) -> std::io::Result<usize> {
    let mut n = 0;
    while n < buf.len() {
        match r.read(&mut buf[n..]) {
            Ok(0) => break,
            Ok(k) => n += k,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
    Ok(n)
}

fn part_path(dest: &Path) -> PathBuf {
    let mut name = dest.as_os_str().to_owned();
    name.push(".part");
    PathBuf::from(name)
}

/// SHA-256 of a file, lower-case hex.
pub fn sha256(path: &Path) -> std::io::Result<String> {
    let mut f = File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0; 1 << 20];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hasher.finalize().iter().map(|b| format!("{b:02x}")).collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::BufRead;
    use std::net::TcpListener;

    fn temp_dir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("rewrite-nlp-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    fn hex_sha(data: &[u8]) -> String {
        Sha256::digest(data).iter().map(|b| format!("{b:02x}")).collect()
    }

    /// A one-request-per-connection HTTP server serving `data`, honouring `Range: bytes=N-` when `ranges` is
    /// set. Returns its URL and a log of the Range headers it saw.
    fn serve(data: Vec<u8>, ranges: bool, requests: usize) -> (String, std::thread::JoinHandle<Vec<Option<u64>>>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/file", listener.local_addr().unwrap());
        let handle = std::thread::spawn(move || {
            let mut seen = Vec::new();
            for stream in listener.incoming().take(requests) {
                let mut stream = stream.unwrap();
                let mut reader = std::io::BufReader::new(stream.try_clone().unwrap());
                let mut from = None;
                loop {
                    let mut line = String::new();
                    reader.read_line(&mut line).unwrap();
                    let line = line.trim_end();
                    if line.is_empty() {
                        break;
                    }
                    if let Some(v) = line.to_ascii_lowercase().strip_prefix("range: bytes=") {
                        from = v.trim_end_matches('-').parse::<u64>().ok();
                    }
                }
                seen.push(from);
                let (status, body) = match from.filter(|_| ranges) {
                    Some(n) => ("206 Partial Content", &data[n as usize..]),
                    None => ("200 OK", &data[..]),
                };
                let head = format!("HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len());
                stream.write_all(head.as_bytes()).unwrap();
                stream.write_all(body).unwrap();
            }
            seen
        });
        (url, handle)
    }

    fn leak(file: ModelFile) -> &'static ModelFile {
        Box::leak(Box::new(file))
    }

    fn test_file(data: &[u8]) -> &'static ModelFile {
        let sha: &'static str = Box::leak(hex_sha(data).into_boxed_str());
        leak(file("blob.bin", "blob.bin", data.len() as u64, sha))
    }

    #[test]
    fn table_matches_python() {
        assert_eq!(MODELS.map(|m| m.key), ["context", "similarity", "paraphrase"]);
        assert_eq!(size_mb("context"), 128);
        assert_eq!(size_mb("similarity"), 24);
        assert_eq!(size_mb("paraphrase"), 253);
        assert!(MODELS.iter().flat_map(|m| m.files).all(|f| f.sha256.len() == 64));
    }

    #[test]
    fn resumes_with_range_on_206() {
        let data: Vec<u8> = (0..700_000u32).map(|i| (i * 7 % 251) as u8).collect();
        let dir = temp_dir("resume");
        let dest = dir.join("blob.bin");
        fs::write(part_path(&dest), &data[..300_000]).unwrap();
        let (url, server) = serve(data.clone(), true, 1);
        let mut last = 0;
        fetch(&agent(), &url, &dest, test_file(&data), &mut |n| last = n, &AtomicBool::new(false)).unwrap();
        assert_eq!(server.join().unwrap(), [Some(300_000)]);
        assert_eq!(last, data.len() as u64);
        assert_eq!(fs::read(&dest).unwrap(), data);
        assert!(!part_path(&dest).exists());
    }

    #[test]
    fn restarts_when_the_server_ignores_range() {
        let data: Vec<u8> = (0..400_000u32).map(|i| (i % 253) as u8).collect();
        let dir = temp_dir("restart");
        let dest = dir.join("blob.bin");
        fs::write(part_path(&dest), b"stale bytes").unwrap();
        let (url, server) = serve(data.clone(), false, 1);
        fetch(&agent(), &url, &dest, test_file(&data), &mut |_| {}, &AtomicBool::new(false)).unwrap();
        assert_eq!(server.join().unwrap(), [Some(11)]);
        assert_eq!(fs::read(&dest).unwrap(), data);
    }

    #[test]
    fn rejects_a_bad_checksum_and_drops_the_part() {
        let data = vec![42u8; 1000];
        let dir = temp_dir("checksum");
        let dest = dir.join("blob.bin");
        let wrong = leak(file("blob.bin", "blob.bin", 1000, "00"));
        let (url, server) = serve(data, true, 1);
        let err = fetch(&agent(), &url, &dest, wrong, &mut |_| {}, &AtomicBool::new(false)).unwrap_err();
        server.join().unwrap();
        assert!(err.to_string().contains("checksum"), "{err}");
        assert!(!dest.exists() && !part_path(&dest).exists());
    }

    #[test]
    fn cancel_keeps_the_part() {
        let data = vec![1u8; 600_000];
        let dir = temp_dir("cancel");
        let dest = dir.join("blob.bin");
        let (url, server) = serve(data.clone(), true, 1);
        let cancel = AtomicBool::new(false);
        let err = fetch(&agent(), &url, &dest, test_file(&data), &mut |_| cancel.store(true, Ordering::Relaxed), &cancel)
            .unwrap_err();
        let _ = server.join();
        assert!(matches!(err, DownloadError::Cancelled));
        assert_eq!(fs::metadata(part_path(&dest)).unwrap().len(), CHUNK as u64);
        assert!(!dest.exists());
    }

    #[test]
    fn downloads_the_paraphraser_config() {
        let spec = spec("paraphrase");
        let config = spec.files.iter().find(|f| f.name == "config.json").unwrap();
        let only_config = ModelSpec { files: std::slice::from_ref(config), ..*spec };
        let dir = temp_dir("config");
        let mut calls = Vec::new();
        download_into(&dir, &only_config, &mut |d, t| calls.push((d, t)), &AtomicBool::new(false)).unwrap();
        assert_eq!(sha256(&dir.join("config.json")).unwrap(), config.sha256);
        assert_eq!(calls.last(), Some(&(1642, 1642)));
        assert!(has_files(&dir, &only_config));
        // Present files are not fetched again.
        calls.clear();
        download_into(&dir, &only_config, &mut |d, t| calls.push((d, t)), &AtomicBool::new(false)).unwrap();
        assert_eq!(calls, [(1642, 1642)]);
    }
}
