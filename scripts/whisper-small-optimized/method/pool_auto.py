"""No-speech positives with the language the model itself detects (auto mode)."""
import sys, os
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from feats import *
torch.set_num_threads(int(os.environ.get("THREADS", "4")))
model = load_model()
d = torch.load(f"{S}/enc/pool.pt")
idx = [i for i, m in enumerate(d["meta"]) if m["ns"] == 1]
enc = d["enc"][idx]
with torch.no_grad():
    lids = []
    for i in range(0, len(enc), 16):
        lids += model.detect_language(encoder_outputs=BaseModelOutput(last_hidden_state=enc[i:i + 16].float())).view(-1).tolist()
langs = [TOK.convert_ids_to_tokens(l)[2:-2] for l in lids]
import collections; print(collections.Counter(langs).most_common(10))
out = features(model, enc, langs, [1] * len(enc))
torch.save(out, f"{S}/ws/feats_auto.pt")
print("positions", len(out["y"]))
