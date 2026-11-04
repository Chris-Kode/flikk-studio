"""Greedy Whisper decoding with OpenAI's rules (suppress blank at the start,
suppressed non-speech tokens, timestamp pairing, increasing timestamps, max
initial timestamp 1 s, timestamp when their total probability wins), on cached
encoder outputs. No temperature fallback, no no-speech gate."""
import torch
from transformers.cache_utils import EncoderDecoderCache, DynamicCache


class Rules:
    def __init__(self, tok, gen_config):
        self.eot = tok.convert_tokens_to_ids("<|endoftext|>")
        self.sot = tok.convert_tokens_to_ids("<|startoftranscript|>")
        self.ts0 = tok.convert_tokens_to_ids("<|0.00|>")
        self.transcribe = tok.convert_tokens_to_ids("<|transcribe|>")
        self.space = tok.convert_tokens_to_ids("Ġ")
        sup = list(gen_config.suppress_tokens or [])
        sup += [tok.convert_tokens_to_ids(t) for t in ["<|startoftranscript|>", "<|translate|>", "<|transcribe|>",
                                                       "<|startoflm|>", "<|startofprev|>", "<|nocaptions|>",
                                                       "<|notimestamps|>"]]
        sup += list(range(self.eot + 2, self.ts0))  # languages and task tokens
        self.suppress = torch.tensor(sorted(set(t for t in sup if t != self.eot)))
        self.max_initial = 50  # 1.0 s in 0.02 s steps


@torch.no_grad()
def decode(model, enc, lang_ids, rules, max_new=120):
    B = len(enc)
    prompt = torch.tensor([[rules.sot, l, rules.transcribe] for l in lang_ids])
    seqs = [[] for _ in range(B)]
    done = [False] * B
    cache = EncoderDecoderCache(DynamicCache(), DynamicCache())
    inp = prompt
    for step in range(max_new):
        out = model(encoder_outputs=(enc,), decoder_input_ids=inp, past_key_values=cache, use_cache=True)
        cache = out.past_key_values
        logits = out.logits[:, -1].float().clone()
        logits[:, rules.suppress] = -float("inf")
        nxt = []
        for b in range(B):
            lg = logits[b]
            s = seqs[b]
            if done[b]:
                nxt.append(rules.eot); continue
            if not s:
                lg[: rules.ts0] = -float("inf")  # starts with a timestamp (blank and eot suppressed too)
                lg[rules.ts0 + rules.max_initial + 1:] = -float("inf")
            else:
                last_ts = s[-1] >= rules.ts0
                pen_ts = len(s) < 2 or s[-2] >= rules.ts0
                if last_ts:
                    if pen_ts:
                        lg[rules.ts0:] = -float("inf")
                    else:
                        lg[: rules.eot] = -float("inf")
                tss = [t for t in s if t >= rules.ts0]
                if tss:
                    lim = tss[-1] if (last_ts and not pen_ts) else tss[-1] + 1
                    lg[rules.ts0:lim] = -float("inf")
                lp = lg.log_softmax(-1)
                if torch.logsumexp(lp[rules.ts0:], 0) > lp[: rules.ts0].max():
                    lg[: rules.ts0] = -float("inf")
            t = int(lg.argmax())
            s.append(t)
            if t == rules.eot:
                done[b] = True
            nxt.append(t)
        if all(done):
            break
        inp = torch.tensor(nxt)[:, None]
    return [s[: s.index(rules.eot) + 1] if rules.eot in s else s for s in seqs]
