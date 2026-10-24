#!/usr/bin/env python3
"""Make the test PDFs in this folder (run once; the PDFs are kept in the repository).

    .venv/Scripts/python.exe tests/corpus/pdf/make_pdfs.py

document.pdf  a formatted document (headings, lists, a table, a see-through picture, a chart, code) laid out
              by rewrite_native.render_pdf
sample.pdf    examples/sample.tex built by pdflatex (in a temporary folder)
rotated.pdf   page 1 of document.pdf turned by 90 degrees, then page 1 again placed as a group (form
              XObject) covering most of a new page
"""
import io
import os
import shutil
import subprocess
import sys
import tempfile

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(os.path.dirname(os.path.dirname(HERE)))
sys.path.insert(0, ROOT)

from rewrite import docmodel as dm, native  # noqa: E402


def picture(w=96, h=64):
    """An RGBA PNG: a colour gradient, half see-through on the right."""
    px = bytearray()
    for y in range(h):
        for x in range(w):
            px += bytes((x * 255 // (w - 1), y * 255 // (h - 1), 160, 255 if x < w // 2 else 110))
    return native.image_encode_png(bytes(px), w, h, 4)


def document_pdf():
    R, P = dm.Run, dm.Paragraph
    para = ("Training large models takes a significant amount of computing time, and most of it is spent "
            "on data that the model has already learned well. We show a simple schedule that skips such "
            "examples and halves the cost while keeping the accuracy of the full run.")
    cell = lambda t, b=False: [P(content=[R(text=t, b=b)])]
    blocks = [
        P(content=[R(text="Efficient Training")], style="title"),
        P(content=[R(text="Introduction")], style="h1"),
        P(content=[R(text=para[:96]), R(text=" most of it ", b=True), R(text=para[96:150], i=True),
                   R(text=para[150:])]),
        P(content=[R(text="Skip examples the model already knows.")], list=("bullet", 0)),
        P(content=[R(text="Keep a small sample of them for checking.")], list=("bullet", 0)),
        P(content=[R(text="Never skip the rare classes.")], list=("bullet", 1)),
        P(content=[R(text="Method")], style="h2"),
        P(content=[R(text="Score every example.")], list=("number", 0)),
        P(content=[R(text="Drop the easiest third.")], list=("number", 0)),
        P(content=[R(text="Train on the rest.")], list=("number", 0)),
        dm.Table(rows=[[cell("Model", True), cell("Hours", True), cell("Accuracy", True)],
                       [cell("Baseline"), cell("120"), cell("91.2")],
                       [cell("Ours"), cell("61"), cell("91.0")]]),
        P(content=[R(text="Table 1. "), R(text="Training cost and accuracy.")], style="caption"),
        P(content=[dm.Image(data=picture(), ext="png", width=192, height=128, alt="gradient")], align="center"),
        P(content=[R(text="Figure 1. A picture with a see-through part.")], style="caption"),
        dm.PageBreak(),
        P(content=[R(text="Results")], style="h1"),
        P(content=[R(text=para)]),
        P(content=[dm.Chart(kind="bar", title="Hours per run", categories=["A", "B", "C"],
                            series=[("Baseline", [120, 80, 95]), ("Ours", [61, 42, 50])],
                            width=360, height=200)]),
        dm.CodeBlock(text="def skip(score):\n    return score < 0.1\n", lang="python"),
        P(content=[R(text="The schedule is short, and it works with any optimiser.")]),
    ]
    doc = dm.Document(blocks=blocks, title="Efficient Training")
    return bytes(native.render_pdf(dm.to_native(doc)))


def sample_pdf():
    tex = os.path.join(ROOT, "examples", "sample.tex")
    exe = shutil.which("pdflatex") or os.path.join(os.environ.get("LOCALAPPDATA", ""), "Programs", "MiKTeX",
                                                   "miktex", "bin", "x64", "pdflatex.exe")
    with tempfile.TemporaryDirectory() as tmp:
        shutil.copy(tex, os.path.join(tmp, "sample.tex"))
        for _ in range(2):
            subprocess.run([exe, "-interaction=nonstopmode", "-halt-on-error", "sample.tex"], cwd=tmp,
                           stdout=subprocess.DEVNULL, check=False)
        with open(os.path.join(tmp, "sample.pdf"), "rb") as f:
            return f.read()


def rotated_pdf(source):
    import pypdfium2 as pdfium
    src = pdfium.PdfDocument(source)
    out = pdfium.PdfDocument.new()
    out.import_pages(src, [0])
    out[0].set_rotation(90)
    w, h = src.get_page_size(0)
    page = out.new_page(w, h)
    xo = src.page_as_xobject(0, out)
    obj = xo.as_pageobject()
    obj.set_matrix(pdfium.PdfMatrix().scale(0.9, 0.9).translate(w * 0.05, h * 0.05))
    page.insert_obj(obj)
    page.gen_content()
    buf = io.BytesIO()
    out.save(buf)
    return buf.getvalue()


def main():
    doc = document_pdf()
    made = {"document.pdf": doc, "sample.pdf": sample_pdf(), "rotated.pdf": rotated_pdf(doc)}
    for name, data in made.items():
        with open(os.path.join(HERE, name), "wb") as f:
            f.write(data)
        print(f"{name}: {len(data)} bytes")


if __name__ == "__main__":
    main()
