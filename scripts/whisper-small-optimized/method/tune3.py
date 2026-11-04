"""v4: the same circuit, laid out so it survives q8_0 quantization untouched.

q8_0 stores 32 consecutive inputs of a row with one scale. A big output column
raised the scale of every block it sits in and coarsened its 31 neighbours. Here
the push is shared by k least-used neurons in k different blocks, each output
column kept within its neighbours' existing range (scale unchanged, neighbours
byte-identical), and the strength moved into the neurons' own input rows and
f32 biases.   usage: tune3.py k P [dev|none]   (P = total push at the median no-speech input)"""
import sys, os, json
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from tune2 import *


def build3(model, X, y, usage, L=5, k=16, P=1280.0, shrink=0.3):
    XL = X[:, L].float().numpy()
    w = direction(XL, y, shrink)
    p = XL @ w
    sp_max, ns_lo = float(np.quantile(p[y == 0], 0.9999)), float(np.quantile(p[y == 1], 0.02))
    t = (sp_max + ns_lo) / 2
    med = float(np.median(p[y == 1]) - t)
    g = eot_direction(model)
    W2 = model.model.decoder.layers[L].fc2.weight.detach().numpy()  # 768 x 3072
    score = (usage[L] * torch.tensor(W2).norm(dim=0)).numpy()
    neurons, used = [], set()
    for j in np.argsort(score):
        if j // 32 not in used:
            neurons.append(int(j)); used.add(j // 32)
        if len(neurons) == k:
            break
    lay = model.model.decoder.layers[L]
    info = dict(L=L, k=k, P=P, t=t, neurons=neurons, beta=[], z_ns_median=[], z_speech_max=[])
    with torch.no_grad():
        for j in neurons:
            blk = W2[:, (j // 32) * 32:(j // 32) * 32 + 32].copy()
            blk[:, j % 32] = 0
            beta = float((np.abs(blk).max(1) / np.maximum(np.abs(g), 1e-9)).min()) * 0.999
            z_med = P / k / beta            # GELU(z) ~ z for large z
            s = z_med / med
            lay.fc1.weight[j] = torch.tensor(w * s, dtype=torch.float32)
            lay.fc1.bias[j] = float(-s * t)
            lay.fc2.weight[:, j] = torch.tensor(g * beta, dtype=torch.float32)
            info["beta"].append(beta); info["z_ns_median"].append(z_med); info["z_speech_max"].append(s * (sp_max - t))
    info["fc1_row_max"] = float(max(abs(lay.fc1.weight[j]).max() for j in neurons))
    info["bias_min"] = float(min(lay.fc1.bias[j] for j in neurons))
    return info


if __name__ == "__main__":
    torch.set_num_threads(int(os.environ.get("THREADS", "4")))
    k, P = int(sys.argv[1]), float(sys.argv[2])
    X, y, usage = pooled()
    orig = load_model(); model = load_model()
    info = build3(model, X, y, usage, 5, k, P)
    info["values_changed"] = sum(int((a != b).sum()) for a, b in zip(orig.state_dict().values(), model.state_dict().values()))
    print(json.dumps({kk: (v if not isinstance(v, list) else [round(x, 3) for x in v][:4]) for kk, v in info.items()}), flush=True)
    tag = f"v4_k{k}_P{P}"
    if len(sys.argv) > 3 and sys.argv[3] == "save":
        out = f"{S}/model/v4"; model.save_pretrained(out); json.dump(info, open(f"{out}/edit.json", "w"), indent=1)
    res, outs = run(model, DEV)
    json.dump(dict(info=info, res=res, outs=outs), open(f"{S}/ws/tune/{tag}.json", "w"), ensure_ascii=False, indent=1)
    print(tag, json.dumps(res))
