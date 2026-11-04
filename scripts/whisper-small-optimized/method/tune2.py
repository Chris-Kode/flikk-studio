"""v2: the no-speech reader is fit with hard counter-examples (short or late speech in a
mostly silent window), on the layer with the widest gap, and its threshold sits in the
middle of the gap between the most no-speech-like speech and the least no-speech-like
silence.  usage: tune2.py L b [zmed] [shrink]"""
import sys, os, json
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from circuit import *
from evalmodel import DEV


def pooled():
    fp = torch.load(f"{S}/ws/feats_pool.pt", weights_only=False)
    parts = [fp] + [torch.load(f"{S}/ws/feats_{n}.pt", weights_only=False) for n in ["hard", "auto", "multi"]
                    if os.path.exists(f"{S}/ws/feats_{n}.pt")]
    X = torch.cat([p["X"] for p in parts]); y = np.concatenate([p["y"] for p in parts])
    more = np.concatenate([p["more"] for p in parts])
    pos = np.concatenate([p["pos"] for p in parts])
    # no speech: the step that picks the first word; speech: every step while words are still to come
    keep = ((y == 1) & (pos == 3)) | ((y == 0) & (more == 1))
    return X[torch.from_numpy(keep)], y[keep], fp["usage"]


def gaps(X, y, shrink=0.3):
    rep = {}
    for L in range(12):
        XL = X[:, L].float().numpy()
        w = direction(XL, y, shrink)
        p = XL @ w
        sp, ns = p[y == 0], p[y == 1]
        rep[L] = dict(gap=float(ns.min() - sp.max()), sd=float(p.std()), sp_max=float(sp.max()), ns_min=float(ns.min()),
                      ns_p01=float(np.quantile(ns, 0.01)), sp_p9999=float(np.quantile(sp, 0.9999)))
    return rep


def build2(model, X, y, usage, L, b, zmed=8.0, shrink=0.3):
    XL = X[:, L].float().numpy()
    w = direction(XL, y, shrink)
    p = XL @ w
    sp_max, ns_lo = float(np.quantile(p[y == 0], 0.9999)), float(np.quantile(p[y == 1], 0.02))
    t = (sp_max + ns_lo) / 2
    s = zmed / max(1e-3, (np.median(p[y == 1]) - t))
    neurons = least_used(model, usage, L, 1)
    write(model, L, neurons, w, t, s, b, eot_direction(model))
    return dict(L=L, neurons=neurons, t=t, s=s, b=b, zmed=zmed, sp_max=sp_max, ns_p01=ns_lo,
                ns_above=float((p[y == 1] > t).mean()), z_speech_max=float(s * (sp_max - t)))


if __name__ == "__main__":
    torch.set_num_threads(int(os.environ.get("THREADS", "4")))
    X, y, usage = pooled()
    if sys.argv[1] == "gaps":
        for L, r in gaps(X, y).items():
            print(L, {k: round(v, 2) for k, v in r.items()})
        sys.exit()
    L, b = int(sys.argv[1]), float(sys.argv[2])
    zmed = float(sys.argv[3]) if len(sys.argv) > 3 else 8.0
    model = load_model()
    info = build2(model, X, y, usage, L, b, zmed)
    res, outs = run(model, DEV)
    tag = f"v2_L{L}_b{b}_z{zmed}"
    json.dump(dict(info=info, res=res, outs=outs), open(f"{S}/ws/tune/{tag}.json", "w"), ensure_ascii=False, indent=1)
    print(tag, json.dumps(info), json.dumps(res))
