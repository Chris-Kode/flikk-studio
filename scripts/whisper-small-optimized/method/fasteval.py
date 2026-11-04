"""Fast eval of decoder edits: encoder outputs are cached (edits only touch the
decoder), greedy decoding with timestamps, no fallback, no gates."""
import sys, os, json, time
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from common import *
from transformers.modeling_outputs import BaseModelOutput

SETS = f"{S}/sets"
CACHE = f"{S}/enc"


def load_set(name):
    m = json.load(open(f"{SETS}/{name}/meta.json"))
    return m


@torch.no_grad()
def cache(model, name, batch=8):
    os.makedirs(CACHE, exist_ok=True)
    p = f"{CACHE}/{name}.pt"
    if os.path.exists(p):
        return
    m = load_set(name)
    outs = []
    for i in range(0, len(m["clips"]), batch):
        aud = [np.fromfile(c["path"], np.float32) for c in m["clips"][i:i + batch]]
        f = PROC.feature_extractor(aud, sampling_rate=SR, return_tensors="pt").input_features
        outs.append(model.model.encoder(f).last_hidden_state.half())
        print(name, i + len(aud), flush=True)
    torch.save(torch.cat(outs), p)


@torch.no_grad()
def decode(model, enc, lang, batch=16, max_new=120):
    texts = []
    for i in range(0, len(enc), batch):
        e = enc[i:i + batch].float()
        dummy = torch.zeros(len(e), 80, 3000)
        seqs = decode_ids(model, e, lang, max_new)
        texts += TOK.batch_decode(seqs, skip_special_tokens=True)
    return texts


RULES = None


def lang_ids(model, e, lang):
    if lang is None:  # the model's own language detection, as whisper does
        return model.detect_language(encoder_outputs=BaseModelOutput(last_hidden_state=e)).view(-1).tolist()
    return [TOK.convert_tokens_to_ids(f"<|{lang}|>")] * len(e)


def decode_ids(model, e, lang, max_new=120):
    global RULES
    from greedy import Rules, decode as gdecode
    if RULES is None:
        RULES = Rules(TOK, model.generation_config)
    if isinstance(lang, list):
        lids = [TOK.convert_tokens_to_ids(f"<|{l}|>") for l in lang]
    else:
        lids = lang_ids(model, e, lang)
    return gdecode(model, e, lids, RULES, max_new)


def run(model, plan, verbose=False):
    """plan: list of (set name, language override or None to use the set's)."""
    res, outs = {}, {}
    for name, lang in plan:
        m = load_set(name)
        enc = torch.load(f"{CACHE}/{name}.pt")
        lg = lang if lang != "set" else m["lang"]
        hyps = decode(model, enc, lg)
        key = f"{name}:{lg or 'auto'}"
        outs[key] = hyps
        if m["kind"] == "short":
            res[key] = dict(spoke=round(float(np.mean([bool(spoken(h)) for h in hyps])), 3))
        elif m["kind"] == "nonspeech":
            res[key] = dict(halluc=round(float(np.mean([bool(spoken(h)) for h in hyps])), 3),
                            loop=round(float(np.mean([loops(h) for h in hyps])), 3),
                            words=round(float(np.mean([len(spoken(h).split()) for h in hyps])), 2))
        else:
            refs = [c["ref"] for c in m["clips"]]
            res[key] = dict(wer=round(wer(refs, hyps), 4), loop=round(float(np.mean([loops(h) for h in hyps])), 3))
    return res, outs


if __name__ == "__main__":
    torch.set_num_threads(int(os.environ.get("THREADS", "4")))
    model = load_model()
    for name in sys.argv[1:]:
        cache(model, name)
