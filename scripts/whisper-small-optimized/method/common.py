"""Eval harness for Whisper small: hallucination on non-speech, loops, WER on speech.

Only the model is evaluated; decoding is plain greedy (temperature 0, no fallback,
no VAD, no no-speech gate) so that any change comes from the weights.
"""
import glob, os, re, zlib, unicodedata, io
import numpy as np, torch, soundfile as sf
import pandas as pd, jiwer
from transformers import WhisperForConditionalGeneration, WhisperProcessor

# Work folder for data, caches and models: never inside the repository.
S = os.environ.get("WHISPER_EDIT_WORK") or os.path.join(
    os.environ.get("FLIKK_HOME") or os.path.expanduser("~/.flikk"), "whisper-edit")
for _d in ("ws/tune", "ws/acts", "sets", "enc", "model"):
    os.makedirs(f"{S}/{_d}", exist_ok=True)
SR = 16000
torch.set_num_threads(4)
PROC = WhisperProcessor.from_pretrained("openai/whisper-small")
TOK = PROC.tokenizer


def load_model(path="openai/whisper-small"):
    m = WhisperForConditionalGeneration.from_pretrained(path)
    m.eval()
    return m


# ---------------------------------------------------------------- audio
def _colored(rng, n, kind):
    w = rng.standard_normal(n)
    if kind == "white":
        x = w
    else:
        f = np.fft.rfftfreq(n, 1 / SR)
        f[0] = f[1]
        spec = np.fft.rfft(w)
        if kind == "pink":
            spec /= np.sqrt(f)
        elif kind == "brown":
            spec /= f
        elif kind == "lowpass":  # room tone / air conditioning
            spec *= 1 / (1 + (f / 300) ** 4)
        x = np.fft.irfft(spec, n)
    return x / (np.sqrt(np.mean(x ** 2)) + 1e-12)


def nonspeech(seed, kind, secs, db):
    rng = np.random.default_rng(seed)
    n = int(secs * SR)
    t = np.arange(n) / SR
    if kind in ("white", "pink", "brown", "lowpass"):
        x = _colored(rng, n, kind)
    elif kind == "hum":
        base = rng.choice([50, 60])
        x = sum(np.sin(2 * np.pi * base * k * t + rng.uniform(0, 6)) / k for k in range(1, 6))
        x = x + 0.1 * _colored(rng, n, "pink")
    elif kind == "silence":  # digital silence with dither
        x = _colored(rng, n, "white") * 1e-2
    elif kind == "clicks":  # keyboard / mouth clicks over room tone
        x = 0.2 * _colored(rng, n, "lowpass")
        for at in rng.integers(0, n - 200, size=int(secs * rng.uniform(0.5, 4))):
            x[at:at + 160] += rng.standard_normal(160) * np.hanning(160) * 6
    elif kind == "fan":  # hum plus brown rumble, held out from training
        x = _colored(rng, n, "brown") + 0.5 * np.sin(2 * np.pi * 120 * t)
    elif kind == "breath":  # slow swells of filtered noise, held out
        env = 0.5 + 0.5 * np.sin(2 * np.pi * t / rng.uniform(2, 4)) ** 8
        x = _colored(rng, n, "pink") * env
    else:
        raise ValueError(kind)
    x = x / (np.sqrt(np.mean(x ** 2)) + 1e-12)
    if kind == "silence":
        db = min(db, -70)
    return (x * 10 ** (db / 20)).astype(np.float32)


TRAIN_KINDS = ["white", "pink", "brown", "lowpass", "hum", "silence", "clicks"]
TEST_KINDS = TRAIN_KINDS + ["fan", "breath"]


def nonspeech_set(split, n):
    base = {"train": 0, "dev": 50_000, "test": 100_000}[split]
    kinds = TEST_KINDS if split == "test" else TRAIN_KINDS
    out = []
    for i in range(n):
        rng = np.random.default_rng(base + i)
        kind = kinds[i % len(kinds)]
        secs = float(rng.uniform(4, 30))
        db = float(rng.uniform(-65, -25))
        out.append((f"{kind}{i}", nonspeech(base + i, kind, secs, db)))
    return out


def speech_en():
    df = pd.read_parquet(f"{S}/data/ls_en/clean/validation-00000-of-00001.parquet")
    out = []
    for _, r in df.iterrows():
        a, sr = sf.read(io.BytesIO(r["audio"]["bytes"]), dtype="float32")
        out.append((r["id"], a, r["text"]))
    return out


def speech_es(n=None, offset=0):
    tr = dict(l.rstrip("\n").split("\t", 1) for l in open(f"{S}/data/mls_es/data/mls_spanish/dev/transcripts.txt"))
    files = sorted(glob.glob(f"{S}/data/mls_es/*.flac"))
    files = files[offset:] if n is None else files[offset:offset + n]
    out = []
    for f in files:
        k = os.path.basename(f)[:-5]
        a, sr = sf.read(f, dtype="float32")
        out.append((k, a, tr[k]))
    return out


def with_tail(clips, split):
    """Speech followed by 10-20 s of non-speech in the same window: where
    Whisper goes on writing after the speech ends."""
    ns = nonspeech_set(split, len(clips))
    out = []
    for (k, a, t), (nk, n) in zip(clips, ns):
        room = 30 * SR - len(a)
        n = n[: max(0, min(len(n), room))]
        if len(n) < 8 * SR:
            n = np.resize(n, min(room, 12 * SR)).astype(np.float32)
        out.append((f"{k}+{nk}", np.concatenate([a, n]), t))
    return out


# ---------------------------------------------------------------- decoding
@torch.no_grad()
def transcribe(model, audios, language, batch=8, timestamps=True, max_new=200):
    texts = []
    for i in range(0, len(audios), batch):
        feats = PROC.feature_extractor(audios[i:i + batch], sampling_rate=SR, return_tensors="pt").input_features
        ids = model.generate(
            feats, language=language, task="transcribe", return_timestamps=timestamps,
            max_new_tokens=max_new, num_beams=1, do_sample=False,
        )
        texts += TOK.batch_decode(ids, skip_special_tokens=True)
    return texts


@torch.no_grad()
def no_speech_prob(model, audios, batch=8):
    nosp = TOK.convert_tokens_to_ids("<|nocaptions|>")
    sot = TOK.convert_tokens_to_ids("<|startoftranscript|>")
    out = []
    for i in range(0, len(audios), batch):
        feats = PROC.feature_extractor(audios[i:i + batch], sampling_rate=SR, return_tensors="pt").input_features
        dec = torch.full((feats.shape[0], 1), sot)
        logits = model(input_features=feats, decoder_input_ids=dec).logits[:, 0]
        out += logits.softmax(-1)[:, nosp].tolist()
    return out


# ---------------------------------------------------------------- metrics
NOTE = re.compile(r"[\(\[][^\)\]]*[\)\]]|♪|♫|\*[^*]*\*")


def norm(s):
    s = NOTE.sub(" ", s.lower())
    s = unicodedata.normalize("NFKC", s)
    s = re.sub(r"[^\w\s']", " ", s)
    return " ".join(s.split())


def spoken(text):
    """Text Whisper wrote, without its notes on what is not speech."""
    return norm(text)


def compression(text):
    b = text.encode()
    return len(b) / max(1, len(zlib.compress(b))) if b else 0.0


def loops(text):
    """OpenAI's test for a looping decode (gzip ratio > 2.4), or the same
    word run three times in a row."""
    w = spoken(text).split()
    if compression(" ".join(w)) > 2.4:
        return True
    for p in range(1, 8):
        for s in range(0, len(w) - 3 * p + 1):
            if w[s:s + p] == w[s + p:s + 2 * p] == w[s + 2 * p:s + 3 * p]:
                return True
    return False


def wer(refs, hyps):
    r = [norm(x) for x in refs]
    h = [norm(x) for x in hyps]
    return jiwer.wer(r, h)


def evaluate(model, sets, verbose=False):
    res = {}
    for name, (kind, clips, lang) in sets.items():
        audios = [c[1] for c in clips]
        hyps = transcribe(model, audios, lang)
        if kind == "nonspeech":
            hall = [bool(spoken(h)) for h in hyps]
            lp = [loops(h) for h in hyps]
            res[name] = dict(halluc=float(np.mean(hall)), loop=float(np.mean(lp)),
                             words=float(np.mean([len(spoken(h).split()) for h in hyps])))
            if verbose:
                for c, h in zip(clips, hyps):
                    if spoken(h):
                        print(f"  [{name}] {c[0]}: {h[:120]!r}")
        else:
            refs = [c[2] for c in clips]
            lp = [loops(h) for h in hyps]
            res[name] = dict(wer=wer(refs, hyps), loop=float(np.mean(lp)))
            if verbose:
                for c, h in zip(clips, hyps):
                    if loops(h) or len(norm(h).split()) > 1.3 * len(norm(c[2]).split()) + 3:
                        print(f"  [{name}] {c[0]}: {h[:160]!r}")
    return res


def with_head(clips, split):
    """3-8 s of non-speech before the speech: Whisper must still hear it."""
    ns = nonspeech_set(split, len(clips))
    out = []
    for i, ((k, a, t), (nk, n)) in enumerate(zip(clips, ns)):
        rng = np.random.default_rng(i + 7)
        L = int(rng.uniform(3, 8) * SR)
        n = np.resize(n, L).astype(np.float32)
        a = a[: 30 * SR - L - SR]
        out.append((f"{nk}+{k}", np.concatenate([n, a]), t))
    return out


def embedded(clips, split, seed, crop=(1.2, 3.0), window=(10, 29), digital=False):
    """Short speech somewhere inside a long stretch of non-speech (or digital
    silence): a window that is mostly no speech, but not all."""
    ns = nonspeech_set(split, len(clips))
    out = []
    for i, ((k, a, t), (nk, n)) in enumerate(zip(clips, ns)):
        rng = np.random.default_rng(seed + i)
        L = min(len(a), int(rng.uniform(*crop) * SR)) if crop else len(a)
        s0 = int(rng.integers(0, max(1, len(a) - L)))
        sp = a[s0:s0 + L]
        W = max(int(rng.uniform(*window) * SR), L + SR)
        bed = np.zeros(W, np.float32) if digital else np.resize(n, W).astype(np.float32)
        at = int(rng.integers(0, W - L))
        x = bed.copy(); x[at:at + L] += sp
        out.append((f"{nk}~{k}@{at / SR:.1f}", x, t if not crop else ""))
    return out


MLS = {"fr": "french", "de": "german", "it": "italian", "pt": "portuguese"}


def speech_mls(lang, n=None, offset=0):
    d = f"{S}/data/mls_{MLS[lang]}"
    tr = dict(l.rstrip("\n").split("\t", 1) for l in open(f"{d}/data/mls_{MLS[lang]}/dev/transcripts.txt"))
    files = sorted(glob.glob(f"{d}/*.flac"))
    files = files[offset:] if n is None else files[offset:offset + n]
    return [(os.path.basename(f)[:-5], sf.read(f, dtype="float32")[0], tr[os.path.basename(f)[:-5]]) for f in files]


# English training speech: chunks of a LibriVox reading (Narsil/asr_dummy canterville.ogg).
def train_speech_en(n=80):
    sr = sf.info(f"{S}/audio/canterville.ogg").samplerate
    a, sr = sf.read(f"{S}/audio/canterville.ogg", dtype="float32", frames=1500 * sr)
    return chunks(a, sr, n, seed=7)


def chunks(a, sr, n, seed):
    import librosa
    if a.ndim > 1:
        a = a.mean(1)
    a = librosa.resample(a, orig_sr=sr, target_sr=SR)
    rng = np.random.default_rng(seed)
    out = []
    for i in range(n):
        L = int(rng.uniform(5, 25) * SR)
        s = int(rng.integers(30 * SR, len(a) - L))
        out.append((f"cant{i}", a[s:s + L].astype(np.float32), ""))
    return out
