import sys, os
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from common import *
from export_sets import dump
if sys.argv[1] == "pool":
    es = speech_es(60, offset=60); en = train_speech_en(40)
    items = [(k, a, "es", 0) for k, a, _ in embedded(es, "train", 1)]
    items += [(k, a, "en", 0) for k, a, _ in embedded(en, "train", 2)]
    items += [(k, a, "en", 0) for k, a, _ in embedded(en[:20], "train", 3, digital=True)]
    items += [(k, a, "es", 0) for k, a, _ in embedded(es[:30], "train", 4, crop=None, window=(18, 29))]
    import pickle; pickle.dump(items, open(f"{S}/ws/hard_items.pkl", "wb"))
else:
    es_dev = speech_es(40, offset=240); es_test = speech_es(60); en = speech_en()
    dump("dev_es_short", embedded(es_dev, "dev", 5), "es", "short")
    dump("dev_es_late", embedded(es_dev, "dev", 6, crop=None, window=(18, 29)), "es", "speech")
    dump("test_es_short", embedded(es_test[:40], "test", 7), "es", "short")
    dump("test_en_short", embedded(en[:40], "test", 8), "en", "short")
    dump("test_es_late", embedded(es_test[:40], "test", 9, crop=None, window=(18, 29)), "es", "speech")
    dump("test_en_late", embedded(en[:40], "test", 10, crop=None, window=(18, 29)), "en", "speech")
print("ok")
