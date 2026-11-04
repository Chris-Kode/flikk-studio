"""Training pool for the edit (disjoint from dev/test): encoder outputs, greedy
decodes of the ORIGINAL model, and for every decoded position the MLP input
(fc1 input, post-LN) and neuron activations (fc2 input) of chosen layers."""
import sys, os, pickle
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from fasteval import *

EOT = TOK.convert_tokens_to_ids("<|endoftext|>")
SOT = TOK.convert_tokens_to_ids("<|startoftranscript|>")
TRANSCRIBE = TOK.convert_tokens_to_ids("<|transcribe|>")


def pool_items():
    ns = nonspeech_set("train", 112)
    es = speech_es(60, offset=60)
    en = train_speech_en(40)
    items = [(k, a, "es", 1) for k, a in ns[:56]] + [(k, a, "en", 1) for k, a in ns[56:]]
    items += [(k, a, "es", 0) for k, a, _ in es] + [(k, a, "en", 0) for k, a, _ in en]
    items += [(k, a, "es", 0) for k, a, _ in with_head(es[:30], "train")]
    items += [(k, a, "en", 0) for k, a, _ in with_head(en[:20], "train")]
    return items


@torch.no_grad()
def build(model, out, batch=8):
    items = pool_items()
    enc_all, meta = [], []
    for i in range(0, len(items), batch):
        ch = items[i:i + batch]
        f = PROC.feature_extractor([c[1] for c in ch], sampling_rate=SR, return_tensors="pt").input_features
        enc_all.append(model.model.encoder(f).last_hidden_state.half())
        meta += [dict(key=c[0], lang=c[2], ns=c[3]) for c in ch]
        print("enc", i + len(ch), "/", len(items), flush=True)
    enc = torch.cat(enc_all)
    torch.save(dict(enc=enc, meta=meta), out)


if __name__ == "__main__":
    torch.set_num_threads(int(os.environ.get("THREADS", "4")))
    build(load_model(), f"{S}/enc/pool.pt")
