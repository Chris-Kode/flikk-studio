import sys, os, json
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from common import *
out = f"{S}/sets"
def dump(name, clips, lang, kind):
    d = f"{out}/{name}"; os.makedirs(d, exist_ok=True)
    meta = []
    for k, a, *t in clips:
        p = f"{d}/{k}.f32"; np.asarray(a, np.float32).tofile(p)
        meta.append(dict(path=p, ref=t[0] if t else ""))
    json.dump(dict(lang=lang, kind=kind, clips=meta), open(f"{d}/meta.json", "w"))
if __name__ == "__main__" and "head" in sys.argv:
    en = speech_en(); es_test = speech_es(60); es_dev = speech_es(40, offset=240)
    dump("test_en_head", with_head(en[:40], "test"), "en", "speech")
    dump("test_es_head", with_head(es_test[:40], "test"), "es", "speech")
    dump("dev_es_head", with_head(es_dev, "dev"), "es", "speech")
    sys.exit()
for split in ["dev", "test"]:
    ns = nonspeech_set(split, 63)
    dump(f"{split}_ns", ns, None, "nonspeech")
en = speech_en(); es_test = speech_es(60); es_dev = speech_es(40, offset=240)
dump("test_en", en, "en", "speech"); dump("test_es", es_test, "es", "speech"); dump("dev_es", es_dev, "es", "speech")
dump("test_en_tail", with_tail(en[:40], "test"), "en", "speech")
dump("test_es_tail", with_tail(es_test[:40], "test"), "es", "speech")
dump("dev_es_tail", with_tail(es_dev, "dev"), "es", "speech")
print("ok")
