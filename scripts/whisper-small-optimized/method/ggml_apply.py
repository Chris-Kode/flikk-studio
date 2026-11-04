"""Carry a weight edit made on the Hugging Face Whisper small over to a
whisper.cpp ggml file: every value the edit changed is written into the
matching ggml tensor, and only the q8_0 blocks holding a changed value are
re-quantized; every other byte is copied.

usage: ggml_apply.py original.bin edited_hf_dir out.bin
"""
import sys, os, re, glob, numpy as np
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from safetensors.numpy import load_file
from ggml_edit import read_tensors, decode, encode, QK8_0



def q8_write(raw, W, changed):
    """Write the changed values of W into q8_0 blocks. A block whose new values
    fit its existing scale keeps that scale and every other value's bits; only a
    block they do not fit is re-quantized."""
    Q8 = np.dtype([("d", "<f2"), ("q", "i1", QK8_0)])
    blocks = np.frombuffer(raw, Q8).copy()
    x = W.reshape(-1, QK8_0); m = changed.reshape(-1, QK8_0)
    for i in np.where(m.any(1))[0]:
        d = float(blocks["d"][i])
        vals = x[i][m[i]]
        if d > 0 and np.abs(vals).max() <= 127 * d:
            q = blocks["q"][i].copy(); q[m[i]] = np.round(vals / d).astype(np.int8); blocks["q"][i] = q
        else:
            amax = np.abs(x[i]).max(); nd = amax / 127.0
            blocks["d"][i] = np.float16(nd)
            blocks["q"][i] = np.round(x[i] / float(np.float16(nd))).astype(np.int8) if nd > 0 else 0
    return blocks.tobytes()


def ggml_name(hf):
    m = re.match(r"model\.decoder\.layers\.(\d+)\.(fc1|fc2)\.(weight|bias)$", hf)
    if not m:
        raise ValueError(f"no ggml name for {hf}")
    l, fc, kind = m.groups()
    return f"decoder.blocks.{l}.mlp.{0 if fc == 'fc1' else 2}.{kind}"


def main(src, edited_dir, dst):
    from huggingface_hub import hf_hub_download
    orig = load_file(hf_hub_download("openai/whisper-small", "model.safetensors"))
    new = load_file(f"{edited_dir}/model.safetensors")
    buf = bytearray(open(src, "rb").read())
    _, tensors = read_tensors(buf)
    total = 0
    for k, v in new.items():
        if k not in orig or np.array_equal(orig[k], v):
            continue
        changed = orig[k] != v
        name = ggml_name(k)
        ttype, ne, off, size = tensors[name]
        if v.ndim == 1:
            cur = np.frombuffer(bytes(buf[off:off + size]), np.float32 if ttype == 0 else np.float16).astype(np.float32)
            cur[changed] = v[changed]
            buf[off:off + size] = cur.astype(np.float32 if ttype == 0 else np.float16).tobytes()
        else:
            W = decode(ttype, bytes(buf[off:off + size]), ne)
            assert W.shape == v.shape, (name, W.shape, v.shape)
            W[changed] = v[changed]
            if ttype == 8:
                buf[off:off + size] = q8_write(bytes(buf[off:off + size]), W, changed)
            else:
                buf[off:off + size] = encode(ttype, W)
        total += int(changed.sum())
        print(f"{name}: {int(changed.sum())} values")
    open(dst, "wb").write(buf)
    print("values changed:", total)


if __name__ == "__main__":
    main(*sys.argv[1:4])
