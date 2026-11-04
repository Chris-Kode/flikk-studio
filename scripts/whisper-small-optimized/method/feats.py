"""Per-position decoder features of the ORIGINAL model on cached encoder outputs:
greedy decode, then a teacher-forced pass recording each layer's MLP input
(post-LN, what fc1 reads) and neuron usage (mean |activation| on speech)."""
import sys, os, pickle
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from fasteval import *

EOT = TOK.convert_tokens_to_ids("<|endoftext|>")
SOT = TOK.convert_tokens_to_ids("<|startoftranscript|>")
TRANSCRIBE = TOK.convert_tokens_to_ids("<|transcribe|>")
TS0 = TOK.convert_tokens_to_ids("<|0.00|>")


@torch.no_grad()
def features(model, enc, langs, labels, batch=16, max_new=120):
    """Returns X: (N_pos, 12, 768) fp16 MLP inputs, y: (N_pos,) labels, clip: (N_pos,)
    clip index, pos: position, usage: (12, 3072) mean |act| over label-0 positions."""
    dec = model.model.decoder
    Xs, ys, cs, ps, mores = [], [], [], [], []
    usage = torch.zeros(12, 3072)
    nuse = 0
    for i in range(0, len(enc), batch):
        e = enc[i:i + batch].float()
        lg = langs[i:i + batch]
        seqs = decode_ids(model, e, list(lg), max_new)
        full = [[SOT, TOK.convert_tokens_to_ids(f"<|{l}|>"), TRANSCRIBE] + s for l, s in zip(lg, seqs)]
        T = max(len(f) for f in full)
        ids = torch.tensor([f + [EOT] * (T - len(f)) for f in full])
        xin, act = {}, {}
        hooks = []
        for l in range(12):
            hooks.append(dec.layers[l].fc1.register_forward_hook(lambda m, inp, out, l=l: xin.__setitem__(l, inp[0])))
            hooks.append(dec.layers[l].fc2.register_forward_hook(lambda m, inp, out, l=l: act.__setitem__(l, inp[0])))
        model(encoder_outputs=(e,), decoder_input_ids=ids)
        for h in hooks:
            h.remove()
        X = torch.stack([xin[l] for l in range(12)], 2)  # B, T, 12, 768
        A = torch.stack([act[l] for l in range(12)], 2)  # B, T, 12, 3072
        for b, f in enumerate(full):
            y = labels[i + b]
            last = len(f) - 1  # position predicting the token after the last one
            # positions 3.. that predict a token (the last one predicts after EOT: skip)
            rng = list(range(3, last)) if f[-1] == EOT else list(range(3, last + 1))
            if y == 0 and f[-1] == EOT:
                rng = rng[:-1] if len(rng) > 1 else rng  # the one predicting EOT is right either way
            if not rng:
                continue
            Xs.append(X[b, rng].half()); ys += [y] * len(rng); cs += [i + b] * len(rng); ps += rng
            # whether text is still to come after the token position p predicts
            mores += [int(any(t < EOT for t in f[p + 1:])) for p in rng]
            if y == 0:
                usage += A[b, rng].abs().sum(0)
                nuse += len(rng)
        print("feats", i + len(e), "/", len(enc), flush=True)
    return dict(X=torch.cat(Xs), y=np.array(ys), clip=np.array(cs), pos=np.array(ps), more=np.array(mores), usage=usage / max(1, nuse))


if __name__ == "__main__":
    torch.set_num_threads(int(os.environ.get("THREADS", "4")))
    model = load_model()
    which = sys.argv[1]
    if which == "pool":
        d = torch.load(f"{S}/enc/pool.pt")
        enc, meta = d["enc"], d["meta"]
        langs = [m["lang"] for m in meta]; labels = [m["ns"] for m in meta]
    else:  # dev: non-speech in es and en, and Spanish speech with and without noise first
        encs, langs, labels = [], [], []
        for name, lang, y in [("dev_ns", "es", 1), ("dev_ns", "en", 1), ("dev_es", "es", 0), ("dev_es_head", "es", 0)]:
            e = torch.load(f"{CACHE}/{name}.pt"); encs.append(e); langs += [lang] * len(e); labels += [y] * len(e)
        enc = torch.cat(encs)
    out = features(model, enc, langs, labels)
    torch.save(out, f"{S}/ws/feats_{which}.pt")
    print("positions", len(out["y"]), "pos", int(out["y"].sum()))
