"""Apply the Whisper small no-speech edit (16 decoder MLP neurons, 24,592 values).

  python apply.py ggml   ggml-small-q8_0.bin   ggml-small-q8_0-silence-fix.bin
  python apply.py hf     openai/whisper-small  ./whisper-small-silence-fix

ggml: works on whisper.cpp model files with f32, f16 or q8_0 decoder MLP tensors;
only the q8_0 blocks that hold a changed value are re-quantized, every other byte
is copied. hf: needs `transformers` and `torch`.

The edit: 16 of the least used MLP neurons of decoder layer 5 (each in its own
32-value q8_0 block) are rewritten so that they read the model's own no-speech
signal at the step that picks the first word and, past a threshold no speech in
the fitting data reached, write toward end-of-text. Their output columns stay
within each block's existing range, so in a q8_0 file no other value changes.
"""
import os
import struct
import sys

import numpy as np

HERE = os.path.dirname(os.path.abspath(__file__))
QK8_0 = 32
Q8 = np.dtype([("d", "<f2"), ("q", "i1", QK8_0)])


def load_edit():
    e = np.load(os.path.join(HERE, "edit.npz"))
    return int(e["layer"]), e["neurons"], e["fc1_rows"], e["fc1_bias"], e["fc2_cols"]


def ggml_tensors(buf):
    o = 4
    assert struct.unpack_from("<I", buf, 0)[0] == 0x67676D6C, "not a whisper.cpp ggml file"
    o += 44
    n_mel, n_fft = struct.unpack_from("<ii", buf, o); o += 8 + 4 * n_mel * n_fft
    n_vocab, = struct.unpack_from("<i", buf, o); o += 4
    for _ in range(n_vocab):
        o += 4 + struct.unpack_from("<I", buf, o)[0]
    out = {}
    while o < len(buf):
        n_dims, name_len, ttype = struct.unpack_from("<iii", buf, o); o += 12
        ne = struct.unpack_from(f"<{n_dims}i", buf, o); o += 4 * n_dims
        name = buf[o:o + name_len].decode(); o += name_len
        n = int(np.prod(ne))
        size = {0: 4 * n, 1: 2 * n, 8: n // QK8_0 * Q8.itemsize}[ttype]
        out[name] = (ttype, ne, o, size)
        o += size
    return out


def get(buf, t):
    ttype, ne, off, size = t
    raw = bytes(buf[off:off + size])
    if ttype == 8:
        b = np.frombuffer(raw, Q8)
        x = b["q"].astype(np.float32) * b["d"].astype(np.float32)[:, None]
    else:
        x = np.frombuffer(raw, np.float32 if ttype == 0 else np.float16).astype(np.float32)
    return x.reshape(ne[::-1]) if len(ne) > 1 else x


def put(buf, t, x, changed):
    ttype, ne, off, size = t
    if ttype != 8:
        buf[off:off + size] = x.astype(np.float32 if ttype == 0 else np.float16).tobytes()
        return
    # A block whose new values fit its existing scale keeps that scale and every
    # other value's bits; only a block they do not fit is re-quantized.
    blocks = np.frombuffer(bytes(buf[off:off + size]), Q8).copy()
    xb = x.reshape(-1, QK8_0).astype(np.float32); mb = changed.reshape(-1, QK8_0)
    for i in np.where(mb.any(1))[0]:
        d = float(blocks["d"][i]); vals = xb[i][mb[i]]
        if d > 0 and np.abs(vals).max() <= 127 * d:
            q = blocks["q"][i].copy(); q[mb[i]] = np.round(vals / d).astype(np.int8); blocks["q"][i] = q
        else:
            nd = float(np.float16(np.abs(xb[i]).max() / 127.0))
            blocks["d"][i] = np.float16(nd)
            blocks["q"][i] = np.round(xb[i] / nd).astype(np.int8) if nd > 0 else 0
    buf[off:off + size] = blocks.tobytes()


def apply_ggml(src, dst):
    L, j, row, bias, col = load_edit()
    buf = bytearray(open(src, "rb").read())
    ts = ggml_tensors(buf)
    pre = f"decoder.blocks.{L}.mlp"
    w1, b1, w2 = ts[f"{pre}.0.weight"], ts[f"{pre}.0.bias"], ts[f"{pre}.2.weight"]
    W1 = get(buf, w1); m1 = np.zeros(W1.shape, bool); W1[j] = row; m1[j] = True; put(buf, w1, W1, m1)
    B1 = get(buf, b1); m = np.zeros(B1.shape, bool); B1[j] = bias; m[j] = True; put(buf, b1, B1, m)
    W2 = get(buf, w2); m2 = np.zeros(W2.shape, bool); W2[:, j] = col; m2[:, j] = True; put(buf, w2, W2, m2)
    open(dst, "wb").write(buf)
    print(f"wrote {dst}: decoder layer {L}, neurons {list(map(int, j))} ({m1.sum() + m.sum() + m2.sum():,} values)")


def apply_hf(src, dst):
    import torch
    from transformers import WhisperForConditionalGeneration, WhisperProcessor
    L, j, row, bias, col = load_edit()
    model = WhisperForConditionalGeneration.from_pretrained(src)
    mlp = model.model.decoder.layers[L]
    idx = torch.from_numpy(np.asarray(j, dtype=np.int64))
    with torch.no_grad():
        mlp.fc1.weight[idx] = torch.from_numpy(row)
        mlp.fc1.bias[idx] = torch.from_numpy(np.asarray(bias, dtype=np.float32))
        mlp.fc2.weight[:, idx] = torch.from_numpy(col)
    model.save_pretrained(dst)
    WhisperProcessor.from_pretrained(src).save_pretrained(dst)
    print(f"wrote {dst}: decoder layer {L}, neurons {list(map(int, j))}")


if __name__ == "__main__":
    kind, src, dst = sys.argv[1:4]
    {"ggml": apply_ggml, "hf": apply_hf}[kind](src, dst)
