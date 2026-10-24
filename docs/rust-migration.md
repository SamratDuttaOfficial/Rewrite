# Moving Rewrite to Rust

Status: done (2026-10-25). Rewrite is one Rust program on GTK 4; the Python/Tk version is gone. Started from
commit `0da855b` (version 1.2.1).

## What changed for users

Nothing to do: settings (`settings.json`, same keys) and downloaded models (`~/.rewrite/models`) carry over, and
the Windows installer keeps its AppId, so it upgrades the old install in place (and removes the old Python files).
The models are now checked against SHA-256 hashes after a download. On Windows the program sits in `bin\` next to
the GTK libraries.

## Layout

```
Cargo.toml        workspace
crates/
  core/           document formats, PDF layout and editing, PDF-to-document analysis, charts, maths, pictures,
                  fonts, LibreOffice, file loading and saving, protected spans, LaTeX builds, Markdown preview
                  parser, export routing, where data lives (paths.rs)
  lang/           text utilities, Python's difflib, phrase book, lemminflect, thesaurus, speller, grammar rules
  nlp/            model downloads, ONNX Runtime, masked language model, embedder, T5 paraphraser, the engine,
                  the helper used by PDF conversion and exports
  pdfium/         PDFium through its C API (loaded at run time), one global lock
  app/            the desktop program (GTK 4)
  testkit/        loads the parity fixtures and compares results with them
xtask/            cargo xtask run | runtime | package | selftest | thesaurus
assets/           thesaurus, lemminflect data, icons
tests/            corpus (inputs) and fixtures (what the Python version returned for them)
runtime/          ONNX Runtime and PDFium for running from source (fetched by cargo xtask runtime; not committed)
```

## How it was done

1. **Parity was tested, not assumed.** Before porting, a script recorded what each Python module returned for a
   fixed corpus (`tests/corpus`) into `tests/fixtures/*.json` (about 45,000 calls, recorded with
   `PYTHONHASHSEED=0` so they repeat). Every Rust port is tested against them with `rewrite-testkit`; the
   fixtures stay as regression tests. The recording script went with the Python code.
2. **Behaviour was copied first.** Thresholds, word lists, regexes, help text, messages and quirks were carried
   over; the decisions below say where Rust differs and why.
3. **The libraries came first, the program last.** Each crate was ported and tested on its own, then the GTK
   program was built on them and compared feature by feature with the Tk program before Python was removed.

## Decisions

| # | Question | Decision |
|---|---|---|
| 1 | UI toolkit | GTK 4 (gtk4-rs), for its text view (tags, marks, embedded widgets and pictures) |
| 2 | MiniLM input is padded to 128 tokens and the padding is averaged in; thresholds were tuned that way | Kept |
| 3 | Two sentence splitters disagree (`text::sentence_bounds` and `textclean::sentence_starts`) | Both kept as they were |
| 4 | T5 sampling used numpy's generator | Rust's generator: variations differ from Python's for a seed |
| 5 | Object and table menus were unreachable with a Mac right-click | Fixed (Control+click works too) |
| 6 | Models had no checksums | SHA-256 checked after download |
| 7 | `mdrender.py` duplicated `md.rs` | Ported as it was (`core::mdpreview`) to keep the preview identical |
| 8 | `native.read` claimed HTML input | No HTML reader, as before |
| 9 | `latexbuild.parse_log` turned MiKTeX's "Fatal error" line into a junk error | Fixed |
| 10 | Speller suggestions, thesaurus candidates and Lite suggestions broke ties in set order | Fixed order; tests compare tied items as a group |
| 11 | Python thesaurus builder output depended on string hashing | `cargo xtask thesaurus` uses file order; the shipped data file is unchanged |
| 12 | An error in a UI callback ended only that callback in Tk | GTK ends the program: the text is saved to `recovered.txt` and the next start says where |
| 13 | Window position (Tk geometry "WxH+X+Y") | GTK 4 cannot place windows: size only, capped to the screen |

## Running and building

See the README: `cargo xtask run`, `cargo test`, `cargo xtask package`. Pushing a `v*` tag builds the three
installers on GitHub Actions.
