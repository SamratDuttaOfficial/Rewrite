import os

import numpy as np


def session(path):
    import onnxruntime as ort

    so = ort.SessionOptions()
    so.intra_op_num_threads = max(1, min(8, (os.cpu_count() or 2) // 2))
    so.graph_optimization_level = ort.GraphOptimizationLevel.ORT_ENABLE_ALL
    so.log_severity_level = 3
    return ort.InferenceSession(path, so, providers=["CPUExecutionProvider"])


def log_softmax(x):
    x = x - x.max(axis=-1, keepdims=True)
    return x - np.log(np.exp(x).sum(axis=-1, keepdims=True))


def pad(seqs, pad_id):
    n = max(len(s) for s in seqs)
    ids = np.full((len(seqs), n), pad_id, dtype=np.int64)
    mask = np.zeros((len(seqs), n), dtype=np.int64)
    for i, s in enumerate(seqs):
        ids[i, :len(s)] = s
        mask[i, :len(s)] = 1
    return ids, mask
