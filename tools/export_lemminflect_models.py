#!/usr/bin/env python3
"""Export lemminflect's two pickled networks to a small binary file each, for crates/lang.

    .venv/Scripts/python.exe tools/export_lemminflect_models.py

Reads assets/lemminflect/model_{lemma,infl}.pkl.gz and writes assets/lemminflect/model_{lemma,infl}.bin:

    b"LIM1", then little-endian u32 n_in, n_hidden, n_out,
    f32 W1[n_in][n_hidden], b1[n_hidden], W2[n_hidden][n_out], b2[n_out],
    then the output classes as UTF-8, one per line.

Both networks are Input(9, 28) -> Flatten -> Dense(relu) -> Dense(softmax); the script checks that.
Run once; the output is kept in the repository.
"""
import gzip
import os
import pickle
import struct

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
DIR = os.path.join(ROOT, "assets", "lemminflect")


def export(name):
    with gzip.open(os.path.join(DIR, f"model_{name}.pkl.gz"), "rb") as f:
        d = pickle.load(f)  # a DataContainer's __dict__: config, weights, meta
    layers = [(layer["class_name"], layer["config"].get("activation")) for layer in d["config"]["layers"]]
    assert layers == [("InputLayer", None), ("Flatten", None), ("Dense", "relu"), ("Dense", "softmax")], layers
    assert tuple(d["config"]["layers"][0]["config"]["batch_input_shape"]) == (None, 9, 28)
    w1, b1, w2, b2 = d["weights"]
    n_in, n_hidden = w1.shape
    n_out = w2.shape[1]
    assert n_in == 9 * 28 and b1.shape == (n_hidden,) and w2.shape == (n_hidden, n_out) and b2.shape == (n_out,)
    classes = d["meta"]["output_enum"]
    assert len(classes) == n_out and all("\n" not in c for c in classes)
    out = os.path.join(DIR, f"model_{name}.bin")
    with open(out, "wb") as f:
        f.write(b"LIM1" + struct.pack("<3I", n_in, n_hidden, n_out))
        for a in (w1, b1, w2, b2):
            f.write(a.astype("<f4").tobytes(order="C"))
        f.write("\n".join(classes).encode("utf-8"))
    print(f"{out}: {n_in}->{n_hidden}->{n_out}, {os.path.getsize(out)} bytes")


if __name__ == "__main__":
    for name in ("lemma", "infl"):
        export(name)
