"""Beat This! (MIT, CPJKU) as ONNX for flikk-analysis's beat tracker.

The graph takes 32 kHz mono samples and returns beat and downbeat logits
for each 20 ms frame, centred on multiples of 20 ms from the first sample.
Inside it: a windowed-sinc resampler to 22.05 kHz (Beat This! uses soxr),
the model's log-mel front end (`LogMelSpect`: 1024-point Hann frames, hop
441, 128 slaney mels from 30 Hz to 11 kHz, magnitude normalized by frame
length, log1p(1000 x)) and the `final0` checkpoint. Feed it up to 30 s
(1500 frames) at a time; its predictions in the outer 6 frames of each
chunk are unreliable, so chunks overlap by 12 frames.

    pip install beat-this
    python export_beat_this.py --out beat-this-final0.onnx
"""

import argparse

import numpy as np
import onnxruntime as ort
import torch
import torch.nn as nn
import torchaudio
from beat_this.inference import Audio2Frames, load_model
from rotary_embedding_torch import RotaryEmbedding

from common import RATE, Resample, export


class Tracker(nn.Module):
    def __init__(self, net):
        super().__init__()
        # The rotary embeddings cache their frequencies in a buffer, which
        # an exported graph cannot write to.
        for m in net.modules():
            if isinstance(m, RotaryEmbedding):
                m.cache_if_possible = False
        self.resample = Resample(RATE, 22050)
        self.mel = torchaudio.transforms.MelSpectrogram(
            sample_rate=22050, n_fft=1024, hop_length=441, f_min=30, f_max=11000,
            n_mels=128, mel_scale="slaney", normalized="frame_length", power=1,
        )
        self.net = net

    def forward(self, samples):
        x = self.resample(samples.unsqueeze(0))
        spect = torch.log1p(1000 * self.mel(x)).transpose(1, 2)
        out = self.net(spect)
        return out["beat"][0], out["downbeat"][0]


def drums(seconds: float, bpm: float) -> np.ndarray:
    """A kick on every beat, a snare on 2 and 4 and eighth-note hats."""
    rng = np.random.default_rng(0)
    out = np.zeros(int(seconds * RATE), np.float32)
    beat = 60.0 / bpm
    t = np.arange(int(0.25 * RATE)) / RATE
    kick = np.sin(2 * np.pi * 55 * t * (1 + 2 * np.exp(-t * 30))) * np.exp(-t * 12)
    snare = rng.standard_normal(t.size) * np.exp(-t * 20) * 0.5
    hat = rng.standard_normal(t.size) * np.exp(-t * 80) * 0.2
    for k in range(int(seconds / beat * 2)):
        at = int(k * beat / 2 * RATE)
        parts = [hat] + ([kick] if k % 2 == 0 else []) + ([snare] if k % 4 == 2 else [])
        for part in parts:
            n = min(part.size, out.size - at)
            if n > 0:
                out[at:at + n] += part[:n].astype(np.float32)
    return out * 0.5


def main():
    p = argparse.ArgumentParser()
    p.add_argument("--checkpoint", default="final0")
    p.add_argument("--out", default="beat-this-final0.onnx")
    a = p.parse_args()

    net = load_model(a.checkpoint, "cpu")
    tracker = Tracker(net).eval()
    length = torch.export.Dim("samples", min=RATE, max=RATE * 31)
    export(tracker, (torch.randn(10 * RATE) * 0.1,), a.out, ["samples"],
           ["beat", "downbeat"], {"samples": {0: length}})

    # Close to Beat This!'s own pipeline, which resamples with soxr.
    audio = drums(20.0, 124.0)
    want_beat, want_down = Audio2Frames(a.checkpoint, "cpu")(audio.astype(np.float64), RATE)
    session = ort.InferenceSession(a.out, providers=["CPUExecutionProvider"])
    beat, down = session.run(None, {"samples": audio})
    n = min(len(beat), len(want_beat))
    print(f"frames: {len(beat)} (reference {len(want_beat)})")
    print(f"beat logits: max difference {np.abs(beat[:n] - want_beat[:n].numpy()).max():.3f}")
    print(f"downbeat logits: max difference {np.abs(down[:n] - want_down[:n].numpy()).max():.3f}")
    peaks = np.flatnonzero((beat > 0) & (beat == np.maximum.reduce(
        [np.roll(beat, s) for s in range(-3, 4)])))
    print("beats at", np.round(peaks / 50, 2)[:12], "expected every", round(60 / 124, 3))


if __name__ == "__main__":
    main()
