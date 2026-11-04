import sys, os, json, glob
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from common import S, spoken, loops, wer, norm
refs = {}
for d in [x for x in glob.glob(f"{S}/sets/*/meta.json") if "/long/" not in x]:
    for c in json.load(open(d))["clips"]: refs[c["path"]] = c["ref"]
for m in json.load(open(f"{S}/sets/long/meta.json")): refs[m["path"]] = m["ref"]
def rd(tag, f):
    rows = [l.rstrip("\n").split("\t") for l in open(f"{S}/ws/wcpp_{tag}/{f}.tsv")]
    return [(r[0], r[1], r[2] if len(r) > 2 else "") for r in rows]
# The two runs of wcpp2.sh to compare, by tag: `score_wcpp.py orig v4`.
TAGS = sys.argv[1:3] or ["orig", "v4"]
out = {}
for f in ["ns_es", "ns_auto", "long_es", "long_en", "es", "en"]:
    for tag in TAGS:
        rows = rd(tag, f)
        if f.startswith("ns"):
            r = dict(n=len(rows), halluc=sum(bool(spoken(t)) for _, _, t in rows) / len(rows), loop=sum(loops(t) for _, _, t in rows) / len(rows))
        else:
            hyp = [t for _, _, t in rows]; ref = [refs[p] for p, _, _ in rows]
            r = dict(n=len(rows), wer=round(wer(ref, hyp), 4), words=sum(len(norm(h).split()) for h in hyp), ref_words=sum(len(norm(x).split()) for x in ref), loop=sum(loops(t) for t in hyp))
        out[f"{f}:{tag}"] = r
        print(f, tag, r)
json.dump(out, open(f"{S}/ws/wcpp_scores.json", "w"), indent=1)
