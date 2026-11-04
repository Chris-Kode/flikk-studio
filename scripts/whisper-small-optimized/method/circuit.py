"""Weight-space programming of a no-speech -> end-of-text circuit in Whisper small.

The model already computes whether a window holds speech; at the step where it
picks the first word it ignores that, and over silence end-of-text sits at about
-8.6 nats. The edit rewrites a few of the least used MLP neurons of a late
decoder layer:

  fc1.weight[j] = s * w        reads a no-speech direction w of the MLP input
  fc1.bias[j]   = -s * t       fires only past threshold t (off on all speech seen)
  fc2.weight[:, j] = b * g     writes toward end-of-text

w and t come from the original model's activations on the training pool only.
"""
import sys, os, json
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from fasteval import *

EOT = TOK.convert_tokens_to_ids("<|endoftext|>")


def auc(pos, neg):
    allv = np.concatenate([pos, neg]); r = allv.argsort().argsort() + 1
    return (r[: len(pos)].sum() - len(pos) * (len(pos) + 1) / 2) / (len(pos) * len(neg))


def direction(X, y, shrink=0.3):
    """Shrunk LDA direction for y=1 vs y=0 (unit)."""
    X1, X0 = X[y == 1], X[y == 0]
    d = X1.mean(0) - X0.mean(0)
    C = np.cov(np.concatenate([X1 - X1.mean(0), X0 - X0.mean(0)]).T)
    C = (1 - shrink) * C + shrink * np.trace(C) / len(C) * np.eye(len(C))
    w = np.linalg.solve(C, d)
    return w / np.linalg.norm(w)


def separation(fp, fd, layers=range(12)):
    rep = {}
    for L in layers:
        Xp = fp["X"][:, L].float().numpy(); yp = fp["y"]
        Xd = fd["X"][:, L].float().numpy(); yd = fd["y"]
        w = direction(Xp, yp)
        pp, pd = Xp @ w, Xd @ w
        t = np.quantile(pp[yp == 0], 0.999)
        rep[L] = dict(auc_pool=round(auc(pp[yp == 1], pp[yp == 0]), 4), auc_dev=round(auc(pd[yd == 1], pd[yd == 0]), 4),
                      dev_ns_above=round(float((pd[yd == 1] > t).mean()), 3), dev_sp_above=round(float((pd[yd == 0] > t).mean()), 4),
                      dev_decision_ns_above=round(float((pd[(yd == 1) & (fd["pos"] == 3)] > t).mean()), 3))
    return rep


def eot_direction(model):
    """Residual direction that raises end of text over every other token, through
    the final layer norm."""
    W = model.proj_out.weight.detach().numpy()  # vocab x 768
    gamma = model.model.decoder.layer_norm.weight.detach().numpy()
    others = np.delete(W, EOT, 0).mean(0)
    g = gamma * (W[EOT] - others)
    g = g - g.mean()
    return g / np.linalg.norm(g)


def least_used(model, usage, L, n):
    W2 = model.model.decoder.layers[L].fc2.weight.detach()
    score = usage[L] * W2.norm(dim=0)
    return score.argsort()[:n].tolist()


def write(model, L, neurons, w, t, s, b, g):
    lay = model.model.decoder.layers[L]
    with torch.no_grad():
        for j in neurons:
            lay.fc1.weight[j] = torch.tensor(w * s, dtype=torch.float32)
            lay.fc1.bias[j] = float(-s * t)
            lay.fc2.weight[:, j] = torch.tensor(g * b, dtype=torch.float32)
