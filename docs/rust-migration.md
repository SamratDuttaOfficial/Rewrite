# Moving Rewrite to Rust

Status: phase 0 done (workspace, parity fixtures and tests). Written 2026-10-24 against commit `0da855b`.

## Where things stand

| Part | Lines | Language |
|---|---|---|
| Document engine: formats, PDF layout/editing/extraction, charts, maths, images, fonts (`native/`) | ~14 900 | Rust (PyO3 module) |
| UI: main window, rich editor, panels, dialogs, PDF view, preview (`app.py`, `rich.py`, `ui/`) | ~7 800 | Python / Tk |
| Language engine: suggestions, models, thesaurus, spelling, grammar (`engine.py`, `nlp/`, `proof.py`, …) | ~2 600 | Python + numpy, onnxruntime, tokenizers, lemminflect |
| Glue: formats, export routing, LaTeX build, PDFium reading, settings, downloads | ~1 900 | Python + pypdfium2 |
| Start-up and packaging (`run.py`, `packaging/`, CI) | ~430 | Python, PyInstaller, Inno Setup |

The goal is one Rust program: no Python, no venv, no PyInstaller, no maturin. Settings, downloaded models and the
Windows install (same Inno Setup AppId) carry over, so existing users upgrade in place.

## How the move is done

1. **The app works after every step.** Logic moves first, behind the existing PyO3 bridge: each Python module is
   replaced by a Rust one and the Python file shrinks to a call, then disappears. The Tk UI keeps running on top
   until the Rust UI reaches parity. Only then is Python deleted.
2. **Parity is tested, not assumed.** Before a module is ported, a throw-away script dumps the Python module's
   outputs for a fixed corpus (sentences, documents, LaTeX/Markdown files, misspellings, PDFs) into JSON fixtures
   under `tests/fixtures/`. The Rust tests must reproduce them. The fixtures stay as regression tests after Python
   is gone.
3. **Behaviour is copied first, improved later.** Thresholds, word lists, regexes, help text and messages are
   carried over verbatim. Known quirks are kept for parity and listed under "Decisions" below.

## Target layout

```
Cargo.toml                 workspace
crates/
  core/      formats, layout, pdfedit, pdfextract, charts, maths, images, fonts, office (was native/)
             + exporting, LaTeX build, file loading/saving, protected spans
  lang/      text utilities, diff, phrase book, thesaurus, lemminflect port, speller, grammar rules
  nlp/       model download, ONNX Runtime sessions, masked LM, embedder, T5, Engine
  pdfium/    PDFium binding behind one global lock: rendering, characters, page objects, bitmaps
  py/        PyO3 bridge (module rewrite_native), temporary: deleted with the Tk UI
  testkit/   loads the parity fixtures and compares results with them
  app/       the desktop program (binary `Rewrite`, GTK 4)
xtask/       cargo xtask run | package | selftest | thesaurus | fixtures
assets/      icons, thesaurus, lemminflect data, phrase book, word lists, help text
```

## Phases

### Phase 0: workspace and test harness (done)
- Cargo workspace: `native/src/*` is now `crates/core` (`rewrite-core`, no Python), the PyO3 module is
  `crates/py` (still `rewrite_native`, so the Python side is unchanged). `run.py` and `packaging/build.py` build
  `crates/py/Cargo.toml`.
- `tools/parity_dump.py` (temporary) records the Python results for `tests/corpus` into `tests/fixtures/*.json`,
  13 groups: textutil, diff, phrases, formats, docmodel, mdrender, latexbuild, lemminflect, thesaurus, speller,
  grammar, engine_lite, engine_smart (the last needs the models). `crates/testkit` loads them;
  `crates/core/tests/parity_docmodel.rs` is the first parity test (model.rs against docmodel.py).
- Writing a parity test for a phase: `Fixture::load("<group>").check("<function>", |case| json(port(case.arg(0))))`.
  Parts not ported yet are `#[ignore = "phase N: …"]`, so `cargo test -- --ignored` lists what is left.
- `.github/workflows/tests.yml` runs `cargo test` on Windows, macOS and Linux (Smart-mode tests skip there, as
  the models are not downloaded).

### Phase 1: pure text logic → `crates/lang`, `crates/core`
| Python | Goes to | Notes |
|---|---|---|
| `textutil.py` | `lang::text` | `WORD_RE`, `words`, `sentence_bounds` (block splitting, abbreviations), `sentence_at`, `match_case`, `STOPWORDS` |
| difflib use (`app.compute_changes`, `diff_chunks`, `textutil.word_diff_ratio`) | `lang::diff` | Port Ratcliff-Obershelp `SequenceMatcher` (`autojunk=False`) so "% reworded" and diff chunks match exactly |
| `phrases.py` | `lang::phrases` | `_RAW` copied to `assets/phrases.txt`; Aho-Corasick with the `[A-Za-z'-]` boundary checks, leftmost-longest |
| `formats.py` | `core::files` | Kind tables, `sniff_kind`, decoding that reports the encoding and rejects binary (utf-8-sig, utf-16, utf-8, cp1252, latin-1), newline style, atomic save, `.doc` via LibreOffice, `.pdf` sniffing |
| `formats.py` protected spans | `core::protect` | LaTeX, Markdown, HTML, code-comment, URL spans, `merge`/`overlaps`/`clip`; patterns use lookbehind, so `fancy-regex` or hand-written scanners |
| `exporting.py` | `core::export` | Targets, `engines()`, `model_of`, routing, `default_page`, LaTeX failure text |
| `latexbuild.py` | `core::latex` | Engine discovery (MiKTeX/TeX Live/MacTeX paths), `% !TEX program`, xelatex detection, Tectonic, rerun loop, BibTeX/Biber with the stamp file and shipped `.bbl`, log parsing, cancel and kill (reuse `office.rs` process code) |
| `mdrender.py` | `core::md` | Preview renders from `md.rs`; extend it with task checkboxes, quote depth, table alignment, footnote marks, `<br>` |
| `docmodel.py` leftovers | `core::model` | `STYLES`, `STYLE_LABEL`, `Image::fit`, landscape/inch fallback in `PageSetup::name`; the rest already exists or is dead |
| `__init__.py` | `app` constants | Version from `Cargo.toml`; author and GitHub URL (no e-mail shown) |

Each lands with PyO3 functions so `app.py` calls Rust, and the Python file is deleted.

### Phase 2: language data → `crates/lang`
- **lemminflect port.** Bundle `lemma_lu.csv.gz`, `infl_lu.csv.gz` and both override files (MIT, keep the licence);
  export the two pickled networks (252→64→34 and 252→64→3) once to a small binary file. Reproduce
  `getAllLemmas`, `getAllLemmasOOV`, `getAllInflections`, `getAllInflectionsOOV`, `getInflection` (alternate tags,
  hard-coded auxiliaries, OOV builders `buildRegVerb` … `buildGrecNoun`) and the lemma-dictionary key list.
  Fixture-test every word in both lookup tables plus an OOV list.
- **Thesaurus** (`thesaurus.py`): load `thesaurus.json.gz` (convert to a compact binary at build time for start-up
  speed), `lemmas`, `tags`, `inflect`, `same_lemma`, `antonyms`, `relation`, `senses`, `candidates`, `lesk_rank`.
- **Speller** (`proof.Speller`): word lists copied verbatim, `known` (all eight rules), `check`, `suggest`
  (edits 1/2, splits, MISSPELLINGS first), `zipf`, user and session words, caches.
- **Grammar rules** (`proof.grammar_rules`, `dedupe`): the 15 `_RULES`, doubled words, a/an, pronoun agreement,
  `_NEG_VERB`, `_HAVE_VERB`, sentence-start capitals. Uses backreferences and inline flags: `fancy-regex`.
- `tools/build_thesaurus.py` → `cargo xtask thesaurus`, reading the WordNet 3.0 database files and wordfreq's
  English list directly (no nltk). Output must be byte-for-byte the same JSON before switching to binary.

### Phase 3: models and engine → `crates/nlp`
- **Downloads** (`models.py`): same repos, revisions, file names and sizes; `.part` files with HTTP Range resume;
  progress and cancel; `ureq` + rustls (replaces certifi). Add SHA-256 checks (values in the inventory below).
- **Data folder**: installed builds keep `~/.rewrite` (settings and models carry over); development runs keep the
  repo root. Same `settings.json` keys and the version-2 migration.
- **ONNX Runtime** via `ort` with `load-dynamic`, loading the `onnxruntime` library shipped next to the program by
  absolute path (avoids the old copy in `System32`, and works with the GNU toolchain used here). Same session
  options: threads `clamp(cpus/2, 1, 8)`, full graph optimisation, CPU provider.
- **Tokenizers**: the `tokenizers` crate with `default-features = false` + `fancy-regex` (no C build).
- **Masked LM** (`nlp/mlm.py`): vocabulary tables, `_parts`, the paired-sentence input, `substitutes` (two-stage
  scoring), `score_many`, `fill`, `known_words`, batching 12/16.
- **Embedder** (`nlp/embed.py`): `encode`, `similarity`, `span_similarity`, `multi_span_similarity`.
- **T5** (`nlp/t5.py`): encoder once, merged decoder with `use_cache_branch` and zero-length past tensors, repetition
  penalty, no-repeat 3-grams, diverse groups (including the double penalty at step 0), greedy or top-40 sampling.
- **Engine** (`engine.py`): every method — `suggest`, `suggest_word`, `_lite_word`, `suggest_phrase`,
  `_word_swaps`, masking/unmasking protected spans, `paraphrase`, `_lite_paraphrase`, `auto_rewrite` with
  strengths, `spell_suggest`, `_alternatives`, `_prose`, `_slip_scores`, `grammar`, calibration and all constants.
- **`cleanup.Helper`** becomes a Rust `textclean::Helper` implementation on the speller and masked LM, so PDF
  conversion and sentence splitting no longer call back into Python.
- Port the selftest's engine checks as Rust tests (Lite mode in CI; Smart mode locally with models present).

### Phase 4: PDFium → `crates/pdfium`
- `pdfium-render` (or a thin FFI over the same `FPDF*` calls), PDFium library from pdfium-binaries shipped next to
  the program, one global mutex replacing `pdfcore.LOCK`.
- Port `pdfdoc.py`: per-character data with the font-size and box corrections, bold/italic/mono detection, page
  objects with form recursion (depth 6, 40 000 objects), picture bitmaps composited onto white, `_renderer` crops,
  `first_page_png`, `to_document`.
- Add what the PDF editor reads today on the Tk thread: character font names, `get_text_bounded` (`_still_there`),
  page rendering for the views.
- After this phase Python uses no pypdfium2, numpy, onnxruntime, tokenizers or lemminflect: only Tk remains.

### Phase 5: the desktop program → `crates/app`
Built alongside the Tk app; nothing is removed until the parity checklist passes. Every item in the UI inventory
must exist, including:
- **Main window**: toolbar, status bar (file kind picker, word count and % reworded, model state, progress with
  cancel, flash messages), movable panes with saved split, 84-column reading width, geometry restore, DPI.
- **Menus and shortcuts**: every menu entry and binding, with the macOS variants (Cmd, Cmd+G, Option, no Cmd+H) and
  F1/F3/F5/F8, Alt+Y/N, Alt+1…9, Ctrl+Space.
- **Plain editor**: decorations (protected, hints, changes, proposals, target, flash, found, spelling and grammar
  underlines with their own colours) in the same stacking order, undo with grouped steps, proposal anchors that
  move with edits, paste detection of the kind, context menu.
- **Formatted editor** (`rich.py`, `ui/objects.py`): the editor buffer is the flat form already in `flat.rs`; paragraph
  styles, character formats, soft breaks, list markers and renumbering, embedded pictures, charts, formulas,
  notes, rules, breaks, opaque objects and in-place tables (with row/column menu), the Put/Cut/Tag/Change undo
  with 2-second typing groups, paragraph normalisation, rich clipboard round trip, key behaviours (Return, Shift+
  Return, Tab, Backspace on lists), Ctrl+click links, zoom, page set-up.
- **Suggestion panel**: all sections and states (review, grammar, get started, Lite mode, alternatives with fit bar
  and badges, original text, whole sentence), one/two-column layout, animated "Thinking".
- **Find bar**: case/word/regex, `\1` replacements, counts to 100 000, highlight near the view, replace all as one
  undo step.
- **Format bar**, **PDF bar and editor** (11 tools, page operations, inline text entry and in-place line editing with
  the erase-then-check fallback, undo of whole versions up to 400 MB, atomic save), **preview** (Markdown, LaTeX
  with clickable errors, formatted document as it prints, lazy page rendering, Ctrl+wheel zoom).
- **Dialogs**: models (first-run welcome, download, retry), auto-rewrite, help (text verbatim), export, picture,
  link, formula, note, table, chart, page set-up, about, unsaved changes.
- **Icons**: the 60 stroke icons from `icons.py` drawn as vector paths, tinted per theme and state.
- **Themes**: the LIGHT and DARK palettes verbatim, per-platform font choices, compact sizes.
- **Workers**: the latest-job-wins lanes (words, sentences, slow, diff, proof with yielding, export, pdf,
  pdf-change, latex) as threads with channels; generation counters and cancel flags as today.
- **Drag and drop** of files, native file dialogs, opening links in the browser, command-line file argument.

### Phase 6: packaging and removal of Python
- `cargo xtask package` replaces `packaging/build.py`: builds the release binary, runs `Rewrite --selftest`, then
  makes the Inno Setup installer (same AppId, per-user install, "Open with" entries, shortcuts), the macOS `.app`
  and `.dmg` (bundle id `com.samratdutta.rewrite`), the `.deb` and the AppImage. The Debian Maintainer field uses
  the GitHub address instead of an e-mail.
- Ship next to the binary: ONNX Runtime and PDFium libraries for each system, the UI toolkit's runtime if it needs
  one, and `assets/`.
- CI: drop Python setup; build with Rust only.
- Delete `rewrite/`, `run.py`, `requirements.txt`, `packaging/build.py`, `launcher.py`, `rewrite.spec`,
  `crates/py`, the parity dump script; `.venv` leaves `.gitignore`. README: "Run from source"
  becomes `cargo run --release`.

## Decisions

| # | Question | Proposal |
|---|---|---|
| 1 | UI toolkit for `crates/app` | GTK 4 (decided 2026-10-24) |
| 2 | MiniLM input is padded to 128 tokens and the padding is averaged in; thresholds were tuned that way | Keep for parity; fix and re-tune later as its own change |
| 3 | Two sentence splitters disagree (`textutil` and `textclean`) | Port both as they are; merge later |
| 4 | T5 sampling uses numpy's PCG64; Rust's generator gives different samples for a seed | Accept: variations stay varied, just not identical |
| 5 | Object and table menus are unreachable with a Mac right-click (Button-3 only) | Fix in the port |
| 6 | Models have no checksums | Add SHA-256 |
| 7 | `mdrender.py` duplicates `md.rs` | Render the preview from `md.rs` |
| 8 | `native.read` claims HTML input but has none | Correct the claim |
| 9 | `latexbuild.parse_log` turns MiKTeX's "Fatal error" line into a junk error (seen in the `missing.log` fixture) | Fix in the port; update the fixture then |
| 10 | Speller suggestions, thesaurus candidates and Lite suggestions break ties in set order, which changes with Python's hash seed (fixtures are recorded with `PYTHONHASHSEED=0`) | Rust breaks ties in a fixed order; the parity tests for these compare tied items as a group |

### The UI toolkit

The formatted editor decides this: it needs styled paragraphs, character tags with their own underline colours,
anchors that move with edits, embedded pictures and live widgets (charts, tables), and custom undo.

- **GTK 4 (gtk4-rs)**: `TextView` has tags, marks with gravity, embedded widgets and pictures, real justify, file
  drops, native dialogs; the Tk code maps almost one to one, so parity is fastest and least risky. Cost: the GTK
  runtime (about 40–70 MB) must be bundled on Windows and macOS, and macOS bundling takes extra work.
- **egui (eframe)**: light, single binary, easy panels and PDF canvas, compact look is simple. Cost: no rich text
  editor, so the formatted and plain editors (layout, selection, IME, embedded objects, undo) must be written from
  scratch, the largest single piece of the whole move.

Chosen: GTK 4.

## Inventory notes kept for the port

- Model files (repo @ revision, size, SHA-256 of the current local copies):
  - `Xenova/roberta-base` @ `bb432e3f…`: `tokenizer.json` 2 108 619 `1649e42e…`, `onnx/model_quantized.onnx`
    126 111 809 `7aea99df…`
  - `Xenova/all-MiniLM-L6-v2` @ `751bff37…`: `tokenizer.json` 711 661 `da0e7993…`,
    `onnx/model_quantized.onnx` 22 972 370 `afdb6f1a…`
  - `Felladrin/onnx-chatgpt_paraphraser_on_T5_base` @ `4ec698d1…`: `tokenizer.json` 2 422 444 `a0ecce9c…`,
    `config.json` 1 642 `2a596850…`, `encoder_model_quantized.onnx` 110 388 422 `bfc9eb15…`,
    `decoder_model_merged_quantized.onnx` 139 724 514 `9a0ca40e…`
  - Full revisions and hashes are recorded when phase 3 starts, taken from `models.py` and the local files.
- Settings keys: version, theme, font_size, font_family, first_run_done, phrase_hints, show_changes, recent,
  geometry, spell_check, grammar_check, user_words, preview, preview_split, find_case, find_word, find_regex,
  last_dir, export_target, export_paper, export_font, export_sentences.
- Toolchain here is `x86_64-pc-windows-gnu` with MinGW; ONNX Runtime and PDFium are both loaded at run time, so no
  MSVC is needed.

## Every current file and where it goes

| File | Phase | Destination |
|---|---|---|
| `rewrite/__init__.py`, `__main__.py` | 1, 5 | `app` constants, `main.rs` |
| `rewrite/app.py` | 5 | `app` (window, menus, keys, analysis pipeline, review mode); `compute_changes`, `diff_chunks` → `lang::diff` in 1 |
| `rewrite/rich.py` | 5 | `app` formatted editor on `core::flat` |
| `rewrite/ui/*.py` (11 files) | 5 | `app` |
| `rewrite/lane.py` | 5 | `app::workers` |
| `rewrite/textutil.py` | 1 | `lang::text` |
| `rewrite/phrases.py` | 1 | `lang::phrases` |
| `rewrite/formats.py` | 1 | `core::files`, `core::protect` |
| `rewrite/exporting.py` | 1 | `core::export` |
| `rewrite/latexbuild.py` | 1 | `core::latex` |
| `rewrite/mdrender.py` | 1 | `core::md` |
| `rewrite/docmodel.py` | 1 | `core::model` (the bridge half goes with `crates/py`) |
| `rewrite/native.py` | 6 | removed with the bridge |
| `rewrite/thesaurus.py` | 2 | `lang::thesaurus` |
| `rewrite/proof.py` | 2 | `lang::speller`, `lang::grammar` |
| lemminflect (dependency) | 2 | `lang::inflect` |
| `rewrite/models.py` | 3 | `nlp::download`, `app::settings` |
| `rewrite/nlp/*.py` | 3 | `nlp::{onnx, mlm, embed, t5}` |
| `rewrite/engine.py` | 3 | `nlp::engine` |
| `rewrite/cleanup.py` | 3 | `nlp::helper` |
| `rewrite/pdfcore.py`, `pdfdoc.py` | 4 | `pdfium` |
| `rewrite/data/thesaurus.json.gz`, `rewrite/assets/*` | 1–2 | `assets/` |
| `tools/build_thesaurus.py` | 2 | `xtask thesaurus` |
| `run.py` | 6 | `cargo xtask run` / `cargo run` |
| `packaging/build.py`, `launcher.py`, `rewrite.spec` | 6 | `xtask package`, `--selftest` flag |
| `packaging/windows/rewrite.iss` | 6 | kept, pointed at the Rust build |
| `.github/workflows/installers.yml` | 0, 6 | Rust-only CI |
| `requirements.txt`, `crates/py/pyproject.toml` | 6 | removed |
| `native/src/*.rs` | 0 (done) | `crates/core` (`py.rs` → `crates/py`) |
