# Parity corpus

Inputs for `tools/parity_dump.py`, which records what the Python modules return for them into
`tests/fixtures/*.json`. The Rust ports are tested against those fixtures.

- `prose.txt`: sentences with abbreviations, misspellings, grammar slips and wordy phrases.
- `edge.md`, `edge.tex`, `edge.html`, `code.*`: markup and source code for protected spans and the preview parser.
- `logs/`: LaTeX logs (paths replaced) for the log parser.
- The examples in `examples/` are read too.

Add inputs here, never in `examples/`, and run `python tools/parity_dump.py` again.
