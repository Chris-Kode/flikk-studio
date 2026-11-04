"""Evaluate the saved edit (tune3.py ... save) on the held-out test sets."""
import sys, os, json
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from fasteval import *
from evalmodel import TEST
torch.set_num_threads(int(os.environ.get("THREADS", "4")))
model = load_model(f"{S}/model/v4")
res, outs = run(model, TEST)
json.dump(dict(res=res, outs=outs), open(f"{S}/ws/edit4_test.json", "w"), ensure_ascii=False, indent=1)
print("done")
