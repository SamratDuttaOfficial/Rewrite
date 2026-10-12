"""Optional small models: where they live, whether they are present, and downloading them.

All models are ONNX files fetched once from Hugging Face (pinned revisions) and
then used fully offline.
"""
import json
import os
import sys
import ssl
import threading
import urllib.request

APP_DIR = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))


def _writable(path):
    try:
        os.makedirs(path, exist_ok=True)
        probe = os.path.join(path, ".write-test")
        with open(probe, "w") as f:
            f.write("ok")
        os.remove(probe)
        return True
    except OSError:
        return False


USER_DIR = os.path.join(os.path.expanduser("~"), ".rewrite")
# Installed (frozen) builds keep models and settings in the user's home, not the install folder.
DATA_DIR = USER_DIR if getattr(sys, "frozen", False) or not _writable(APP_DIR) else APP_DIR
MODELS_DIR = os.path.join(DATA_DIR, "models")
SETTINGS_FILE = os.path.join(DATA_DIR, "settings.json")

MODELS = {
    "context": {
        "title": "Context model",
        "detail": "RoBERTa-base · 125M params · ranks words and phrases by how well they fit the sentence",
        "repo": "Xenova/roberta-base",
        "rev": "bb432e3f5502192ed482fae6a2bb0124ef61c51d",
        "files": {"tokenizer.json": ("tokenizer.json", 2108619),
                  "model.onnx": ("onnx/model_quantized.onnx", 126111809)},
    },
    "similarity": {
        "title": "Meaning check",
        "detail": "MiniLM-L6 · 22M params · verifies a rewrite keeps the original meaning",
        "repo": "Xenova/all-MiniLM-L6-v2",
        "rev": "751bff37182d3f1213fa05d7196b954e230abad9",
        "files": {"tokenizer.json": ("tokenizer.json", 711661),
                  "model.onnx": ("onnx/model_quantized.onnx", 22972370)},
    },
    "paraphrase": {
        "title": "Sentence paraphraser",
        "detail": "T5-base · 220M params · rewrites whole sentences and phrases",
        "repo": "Felladrin/onnx-chatgpt_paraphraser_on_T5_base",
        "rev": "4ec698d13cdf0a9d8d1315209b200f62346e7ed1",
        "files": {"tokenizer.json": ("tokenizer.json", 2422444),
                  "config.json": ("config.json", 1642),
                  "encoder.onnx": ("onnx/encoder_model_quantized.onnx", 110388422),
                  "decoder.onnx": ("onnx/decoder_model_merged_quantized.onnx", 139724514)},
    },
}


def folder(key):
    return os.path.join(MODELS_DIR, key)


def size_mb(key):
    return round(sum(s for _, s in MODELS[key]["files"].values()) / 1e6)


def present(key):
    d = folder(key)
    return all(os.path.isfile(os.path.join(d, name)) and os.path.getsize(os.path.join(d, name)) == size
               for name, (_, size) in MODELS[key]["files"].items())


def runtime_available():
    try:
        import onnxruntime  # noqa: F401
        import tokenizers  # noqa: F401
        return True
    except Exception:
        return False


def _ssl_context():
    try:
        import certifi
        return ssl.create_default_context(cafile=certifi.where())
    except Exception:
        return ssl.create_default_context()


def download(key, progress=None, cancel=None):
    """Download one model. progress(done_bytes, total_bytes). Raises on failure."""
    spec = MODELS[key]
    d = folder(key)
    os.makedirs(d, exist_ok=True)
    total = sum(s for _, s in spec["files"].values())
    done = 0
    ctx = _ssl_context()
    for name, (remote, size) in spec["files"].items():
        dest = os.path.join(d, name)
        if os.path.isfile(dest) and os.path.getsize(dest) == size:
            done += size
            if progress:
                progress(done, total)
            continue
        url = f"https://huggingface.co/{spec['repo']}/resolve/{spec['rev']}/{remote}"
        part = dest + ".part"
        have = os.path.getsize(part) if os.path.isfile(part) else 0
        req = urllib.request.Request(url, headers={"User-Agent": "rewrite-app/1.0"})
        if have:
            req.add_header("Range", f"bytes={have}-")
        with urllib.request.urlopen(req, context=ctx, timeout=60) as r:
            mode = "ab" if have and r.status == 206 else "wb"
            if mode == "wb":
                have = 0
            with open(part, mode) as f:
                got = have
                while True:
                    if cancel and cancel.is_set():
                        raise InterruptedError("Download cancelled")
                    chunk = r.read(1 << 18)
                    if not chunk:
                        break
                    f.write(chunk)
                    got += len(chunk)
                    if progress:
                        progress(done + got, total)
        if os.path.getsize(part) != size:
            raise IOError(f"{name}: incomplete download ({os.path.getsize(part)} of {size} bytes)")
        os.replace(part, dest)
        done += size
    return True


# -- settings ------------------------------------------------------------------------

DEFAULTS = {
    "theme": "light",
    "font_size": 13,
    "font_family": "",
    "first_run_done": False,
    "phrase_hints": True,
    "show_changes": True,
    "recent": [],
    "geometry": "",
    "spell_check": True,
    "grammar_check": True,
    "user_words": [],
    "preview": False,
    "preview_split": 0.5,
}
_lock = threading.Lock()


def load_settings():
    s = dict(DEFAULTS)
    try:
        with open(SETTINGS_FILE, encoding="utf-8") as f:
            s.update(json.load(f))
    except Exception:
        pass
    return s


def save_settings(s):
    with _lock:
        try:
            os.makedirs(os.path.dirname(SETTINGS_FILE), exist_ok=True)
            with open(SETTINGS_FILE, "w", encoding="utf-8") as f:
                json.dump(s, f, indent=2)
        except OSError:
            pass
