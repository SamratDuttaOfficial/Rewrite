# Parity corpus

Inputs whose results under the earlier Python version (commit `0da855b`) are recorded in `tests/fixtures/*.json`.
The Rust crates are tested against those results (see `crates/testkit`).

- `prose.txt`: sentences with abbreviations, misspellings, grammar slips and wordy phrases.
- `edge.md`, `edge.tex`, `edge.html`, `code.*`: markup and source code for protected spans and the preview parser.
- `logs/`: LaTeX logs (user name masked) for the log parser.
- `pdf/`: PDFs for reading and converting PDFs.
- The examples in `examples/` were read too.

The Python version is gone, so the fixtures can no longer be recorded again; new behaviour gets ordinary tests.
