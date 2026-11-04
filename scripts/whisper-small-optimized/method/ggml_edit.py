"""Apply a fc2-column edit to a whisper.cpp ggml model file (f16/f32/q8_0 tensors).

Reads the legacy whisper.cpp ggml layout (magic, hparams, mel filters, vocab,
then tensors), and rewrites only the bytes of the decoder fc2 tensors
(decoder.blocks.L.mlp.2.weight) that the edit touches. Every other byte of the
file is copied unchanged.

usage: ggml_edit.py in.bin edit.npz out.bin
edit.npz: v (768,), alpha (scalar), layers (k,), cols (k, n) -- per layer the columns
"""
import sys, struct, numpy as np

QK8_0 = 32
TYPES = {0: "f32", 1: "f16", 8: "q8_0"}


def read_tensors(buf):
    o = 0
    magic, = struct.unpack_from("<I", buf, o); o += 4
    assert magic == 0x67676D6C, hex(magic)
    hp = struct.unpack_from("<11i", buf, o); o += 44
    n_mel, n_fft = struct.unpack_from("<ii", buf, o); o += 8
    o += 4 * n_mel * n_fft
    n_vocab, = struct.unpack_from("<i", buf, o); o += 4
    for _ in range(n_vocab):
        ln, = struct.unpack_from("<I", buf, o); o += 4 + ln
    tensors = {}
    while o < len(buf):
        n_dims, name_len, ttype = struct.unpack_from("<iii", buf, o); o += 12
        ne = struct.unpack_from(f"<{n_dims}i", buf, o); o += 4 * n_dims
        name = buf[o:o + name_len].decode(); o += name_len
        n = int(np.prod(ne))
        if ttype == 0:
            size = 4 * n
        elif ttype == 1:
            size = 2 * n
        elif ttype == 8:
            size = n // QK8_0 * (2 + QK8_0)
        else:
            raise ValueError(f"type {ttype} of {name}")
        tensors[name] = (ttype, ne, o, size)
        o += size
    return hp, tensors


def q8_decode(raw, ne):
    blocks = np.frombuffer(raw, dtype=np.dtype([("d", "<f2"), ("q", "i1", QK8_0)]))
    x = blocks["q"].astype(np.float32) * blocks["d"].astype(np.float32)[:, None]
    return x.reshape(ne[1], ne[0])


def q8_encode(x):
    b = x.reshape(-1, QK8_0).astype(np.float32)
    amax = np.abs(b).max(1)
    d = amax / 127.0
    inv = np.where(d > 0, 1.0 / np.where(d > 0, d, 1), 0)
    q = np.round(b * inv[:, None]).astype(np.int8)
    out = np.empty(len(b), dtype=np.dtype([("d", "<f2"), ("q", "i1", QK8_0)]))
    out["d"] = d.astype(np.float16)
    out["q"] = q
    return out.tobytes()


def decode(ttype, raw, ne):
    if ttype == 8:
        return q8_decode(raw, ne)
    dt = np.float32 if ttype == 0 else np.float16
    return np.frombuffer(raw, dtype=dt).astype(np.float32).reshape(ne[1], ne[0])


def encode(ttype, x):
    if ttype == 8:
        return q8_encode(x)
    return x.astype(np.float32 if ttype == 0 else np.float16).tobytes()


def main(src, edit, dst):
    buf = bytearray(open(src, "rb").read())
    hp, tensors = read_tensors(buf)
    e = np.load(edit, allow_pickle=True)
    v = e["v"].astype(np.float32)
    alpha = float(e["alpha"])
    picks = e["picks"].item()
    changed = 0
    for l, cols in picks.items():
        name = f"decoder.blocks.{l}.mlp.2.weight"
        ttype, ne, off, size = tensors[name]
        W = decode(ttype, bytes(buf[off:off + size]), ne)  # 768 x 3072, column j = neuron j
        cols = np.array(sorted(cols))
        sub = W[:, cols]
        W[:, cols] = sub - alpha * np.outer(v, v @ sub)
        new = encode(ttype, W)
        assert len(new) == size
        if ttype == 8:  # keep the original bytes of every block the edit did not reach
            bs = 2 + QK8_0
            touched = np.zeros(W.shape, bool)
            touched[:, cols] = True
            touched = touched.reshape(-1, QK8_0).any(1)
            old = np.frombuffer(bytes(buf[off:off + size]), np.uint8).reshape(-1, bs)
            nw = np.frombuffer(new, np.uint8).reshape(-1, bs).copy()
            nw[~touched] = old[~touched]
            new = nw.tobytes()
        buf[off:off + size] = new
        changed += len(cols) * W.shape[0]
        print(f"{name} [{TYPES[ttype]}]: {len(cols)} columns edited")
    open(dst, "wb").write(buf)
    print("weights changed:", changed)


if __name__ == "__main__":
    main(*sys.argv[1:4])
