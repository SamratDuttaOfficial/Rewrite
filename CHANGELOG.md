# Changelog

Installers for every version are on the [releases page](https://github.com/SamratDuttaOfficial/Rewrite/releases).

## 2.0.3 (14 Nov 2026)

- The Linux AppImages start on older systems again (Ubuntu 22.04, Debian 12 and similar). 2.0.2 needed glibc 2.39.
- No changes on Windows or macOS.

## 2.0.2 (12 Nov 2026)

- Fixed the smart models on Windows. The 2.0.1 installers shipped the wrong file for the ONNX Runtime libraries, so the models couldn't be downloaded or used.

## 2.0.1 (25 Oct 2026)

Rewrite is now one program written in Rust, with a GTK 4 interface. Settings and downloaded models carry over.

- Word suggestions and sentence rewrites run about 3 to 5 times faster.
- A compact title bar with the icon, menus and file name in one row; macOS uses its own menu bar.
- Sharp icons at every size on Windows, macOS and Linux.
- Downloaded models are checked against SHA-256 hashes.
- If Rewrite stops unexpectedly, the text you were editing is saved and the next start says where.
- Fixed: small, faint icons on scaled Windows screens, and the Linux installers missing from 2.0.0.

## 1.2.1 (16 Oct 2026)

- References added to the .bib now show up in the LaTeX preview, even when an old `.bbl` sits next to the document.
- Papers with a `.bbl` and no `.bib` (arXiv sources) still show their references.

## 1.2.0 (16 Oct 2026)

- Find and replace: Ctrl+F finds as you type, Ctrl+H or Ctrl+R adds a replace field, with case, whole-word and regular expression options.
- Rephrase sentence moved to Ctrl+E.
- One undo step per change for suggestions, reviewed changes and "Revert everything".

## 1.1.0 (12 Oct 2026)

- Icon toolbar, smaller default text, a denser side panel and a status bar.
- With the preview open, alternatives and sentence rewrites sit side by side.

## 1.0.0 (12 Oct 2026)

First release: word, phrase and sentence suggestions, auto-rewrite with review, spelling and grammar checks, and a live Markdown and LaTeX preview. Everything runs on your own computer.
