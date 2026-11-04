"""Long recordings with pauses, as an editor sees them: test speech clips with
2-12 s of test non-speech between them, for whisper.cpp's own long-form decoding."""
import sys, os, json
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from common import *
out = f"{S}/sets/long"; os.makedirs(out, exist_ok=True)
meta = []
for name, clips, lang in [("es", speech_es(60), "es"), ("en", speech_en(), "en")]:
    ns = nonspeech_set("test", 63)
    for part in range(2):
        rng = np.random.default_rng(part + (0 if lang == "es" else 10))
        sel = clips[part * 12:(part + 1) * 12]
        pieces, refs = [], []
        for i, (k, a, t) in enumerate(sel):
            gap = ns[(part * 12 + i) % len(ns)][1]
            g = np.resize(gap, int(rng.uniform(2, 12) * SR)).astype(np.float32)
            pieces += [g, a]; refs.append(t)
        pieces.append(np.resize(ns[part][1], 15 * SR).astype(np.float32))  # ends on 15 s of noise
        x = np.concatenate(pieces)
        p = f"{out}/{name}{part}.f32"; x.tofile(p)
        meta.append(dict(path=p, lang=lang, ref=" ".join(refs), secs=len(x) / SR))
json.dump(meta, open(f"{out}/meta.json", "w"), ensure_ascii=False)
print([(m["path"].split("/")[-1], round(m["secs"])) for m in meta])
