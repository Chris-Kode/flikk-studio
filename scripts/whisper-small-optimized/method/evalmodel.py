"""Evaluate a model (HF dir or the original) on dev or test with cached encoder outputs."""
import sys, os, json
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from fasteval import *
DEV = [("dev_ns", "es"), ("dev_ns", "en"), ("dev_ns", None), ("dev_es", "set"), ("dev_es_tail", "set"), ("dev_es_head", "set"), ("dev_es_short", "set"), ("dev_es_late", "set")]
TEST = [("test_ns", "es"), ("test_ns", "en"), ("test_ns", None), ("test_en", "set"), ("test_es", "set"),
        ("test_en_tail", "set"), ("test_es_tail", "set"), ("test_en_head", "set"), ("test_es_head", "set"), ("test_es_short", "set"), ("test_en_short", "set"), ("test_es_late", "set"), ("test_en_late", "set")]
HARD = [("test_es_short", "set"), ("test_en_short", "set"), ("test_es_late", "set"), ("test_en_late", "set")]
DEVML = [("dev_ns", "fr"), ("dev_ns", "de"), ("dev_fr", "set"), ("dev_de", "set"), ("dev_it", "set"), ("dev_pt", "set")]
TESTML = [("test_ns", "fr"), ("test_ns", "de"), ("test_ns", "ja"), ("test_fr", "set"), ("test_de", "set"), ("test_it", "set"), ("test_pt", "set"),
          ("test_fr_short", "set"), ("test_de_short", "set"), ("test_it_short", "set"), ("test_pt_short", "set")]
DEV = DEV + DEVML
TEST = TEST + TESTML
DEVHARD = [("dev_es_short", "set"), ("dev_es_late", "set")]
HEAD = [("test_en_head", "set"), ("test_es_head", "set")]
if __name__ == "__main__":
    torch.set_num_threads(int(os.environ.get("THREADS", "4")))
    path, split, out = sys.argv[1], sys.argv[2], sys.argv[3]
    model = load_model(path)
    res, outs = run(model, {"dev": DEV, "test": TEST, "head": HEAD, "hard": HARD, "devhard": DEVHARD, "devml": DEVML, "testml": TESTML}[split])
    json.dump(dict(res=res, outs=outs), open(out, "w"), ensure_ascii=False, indent=1)
    print(json.dumps(res, indent=1))
