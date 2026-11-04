import sys, os, pickle
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from feats import *
torch.set_num_threads(int(os.environ.get("THREADS", "4")))
model = load_model()
items = pickle.load(open(f"{S}/ws/hard_items.pkl", "rb"))
encs = []
with torch.no_grad():
    for i in range(0, len(items), 8):
        f = PROC.feature_extractor([c[1] for c in items[i:i + 8]], sampling_rate=SR, return_tensors="pt").input_features
        encs.append(model.model.encoder(f).last_hidden_state.half()); print("enc", i, flush=True)
enc = torch.cat(encs)
out = features(model, enc, [c[2] for c in items], [c[3] for c in items])
torch.save(out, f"{S}/ws/feats_hard.pt")
print("positions", len(out["y"]))
