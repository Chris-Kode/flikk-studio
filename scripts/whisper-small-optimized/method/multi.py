"""Other languages: training counter-examples, no-speech positives under many language
prompts, and dev/test sets of French, German, Italian and Portuguese speech."""
import sys, os, pickle
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from feats import *
from export_sets import dump
LANGS = ["fr", "de", "it", "pt"]
if sys.argv[1] == "sets":
    for l in LANGS:
        dump(f"dev_{l}", speech_mls(l, 12, offset=30), l, "speech")
        dump(f"test_{l}", speech_mls(l, 20, offset=42), l, "speech")
        dump(f"test_{l}_short", embedded(speech_mls(l, 20, offset=42), "test", 20 + LANGS.index(l)), l, "short")
    print("ok"); sys.exit()
torch.set_num_threads(int(os.environ.get("THREADS", "4")))
model = load_model()
items = []
for i, l in enumerate(LANGS):
    sp = speech_mls(l, 30)
    items += [(a, l, 0) for k, a, _ in sp]
    items += [(a, l, 0) for k, a, _ in embedded(sp[:15], "train", 40 + i)]
ns = nonspeech_set("train", 112)
rng = np.random.default_rng(3)
others = ["fr", "de", "it", "pt", "ja", "zh", "ru", "si", "km", "ka", "ko", "ar", "hi", "nl", "pl", "tr", "cy", "nn", "haw", "la"]
items += [(a, str(rng.choice(others)), 1) for k, a in ns]
encs = []
with torch.no_grad():
    for i in range(0, len(items), 8):
        f = PROC.feature_extractor([c[0] for c in items[i:i + 8]], sampling_rate=SR, return_tensors="pt").input_features
        encs.append(model.model.encoder(f).last_hidden_state.half()); print("enc", i, flush=True)
out = features(model, torch.cat(encs), [c[1] for c in items], [c[2] for c in items])
torch.save(out, f"{S}/ws/feats_multi.pt")
print("positions", len(out["y"]))
