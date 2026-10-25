<p align="center">
  <img src="rewrite/assets/icon256.png" width="88" alt="Rewrite icon">
</p>

<h1 align="center">Rewrite</h1>

<p align="center">
  An offline editor that suggests better words, phrases and sentences, and checks spelling and grammar.<br>
  Everything runs on your own computer.
</p>

![Clicking a word shows alternatives that fit the sentence](docs/screenshot.png)

## Features

- **Suggestions that fit the sentence.** Alternatives for a word, a phrase or a whole sentence, ranked by context.
- **Auto-rewrite.** Rewrite a selection or the whole document, then apply everything or review each change.
- **Spelling and grammar.** Red and blue underlines, each with a one-click fix.
- **Live preview.** Rendered Markdown as you type, the PDF of a LaTeX file, or a formatted document as it prints.
- **Markup stays intact.** LaTeX, Markdown, HTML and code are never changed.
- **Word, OpenDocument and RTF.** Open and save .docx, .odt, .fodt, .rtf and .doc with their formatting: styles,
  lists, pictures, tables, charts, formulas, footnotes and links, with a formatting toolbar to edit them.
- **PDF.** Highlight, underline, strike out, add notes, draw, white out, add text and pictures, change a line of text
  in place, and turn, move, delete, insert or merge pages.
- **Export and convert.** Write any of them as PDF, Word, OpenDocument, RTF, HTML, Markdown, LaTeX or plain text.
  PDFs become editable documents with headings, lists, tables and pictures; the language model mends words broken
  at line ends and finds sentence boundaries.
- **Private.** No account and no GPU, and it works without internet.

## Install

Download the file for your system from the [latest release](https://github.com/SamratDuttaOfficial/rewrite/releases/latest).

| System | File | How |
|---|---|---|
| Windows 10/11 | `Rewrite-2.0.1-Windows-Setup.exe` | Run it. No admin rights needed. |
| macOS 11+ (Apple Silicon) | `Rewrite-2.0.1-macOS-arm64.dmg` | Drag to Applications. The first time, right-click and choose **Open**. |
| Debian / Ubuntu | `rewrite_2.0.1_amd64.deb` | `sudo apt install ./rewrite_2.0.1_amd64.deb` |
| Other Linux | `Rewrite-2.0.1-x86_64.AppImage` | `chmod +x` it, then run it. |

On first start, Rewrite offers to download three small models (about 400 MB, once). If you skip this, it runs in
Lite mode with the built-in thesaurus.

## Use

| To | Do this |
|---|---|
| See alternatives | Click a word, or select a phrase |
| Rephrase a sentence | **Ctrl+E** |
| Rewrite a selection or the whole document | **Ctrl+Shift+R** |
| Fix a spelling or grammar slip | Click the underlined word |
| Find and replace | **Ctrl+F**, **Ctrl+H** (or **Ctrl+R**); **F3** finds the next match |
| Open a file | **Ctrl+O**, or drop it on the window |
| Preview Markdown, LaTeX or a formatted document | **Ctrl+P**; **F5** rebuilds |
| Export or convert | **Ctrl+Shift+E** |
| Format text in a Word or OpenDocument file | **Ctrl+B**, **Ctrl+I**, **Ctrl+U**, **Ctrl+K** for a link, or the toolbar |

On macOS, use **⌘** instead of **Ctrl**. Press **F1** in the app for the full guide.

Rewrite opens txt, md, tex, html and source code as text; docx, doc, odt, fodt and rtf as formatted documents; and
pdf files in its PDF view. The LaTeX preview and LaTeX exports need a LaTeX installation (MiKTeX, MacTeX or TeX Live);
saving as .doc, and PDF exports typeset by LibreOffice, need LibreOffice.

## Run from source

```bash
cargo xtask run
```

Rewrite is written in Rust with GTK 4. You need [Rust](https://rustup.rs) and GTK 4's development files:

| System | GTK 4 |
|---|---|
| Windows | [MSYS2](https://www.msys2.org), then `pacman -S mingw-w64-ucrt-x86_64-gtk4 mingw-w64-ucrt-x86_64-pkgconf mingw-w64-ucrt-x86_64-gcc`, with `C:\msys64\ucrt64\bin` on `PATH` and Rust's `stable-x86_64-pc-windows-gnu` toolchain |
| macOS | `brew install gtk4 pkg-config` |
| Debian / Ubuntu | `sudo apt install libgtk-4-dev` |

The first run downloads ONNX Runtime and PDFium into `runtime/`. `cargo test` runs the tests.

## Build the installers

```bash
cargo xtask package
```

This builds the installer for the current system into `dist/installers/` (on Windows it needs Inno Setup 6; on
Linux, `linuxdeploy` for the AppImage). Pushing a version tag such as `v1.0.0` builds all three on GitHub Actions
and attaches them to the release.

## Credits

Made by **Samrat Dutta** · [github.com/samratduttaofficial](https://github.com/samratduttaofficial)

Rewording someone else's text does not make it your own work. Cite your sources.

<sub>WordNet 3.0 © Princeton University · word frequencies from `wordfreq` (CC BY-SA 4.0) · RoBERTa-base (MIT) ·
all-MiniLM-L6-v2 (Apache-2.0) · `humarin/chatgpt_paraphraser_on_T5_base` (OpenRAIL), run with ONNX Runtime ·
lemminflect (MIT) · PDFium (BSD) · GTK (LGPL).</sub>
