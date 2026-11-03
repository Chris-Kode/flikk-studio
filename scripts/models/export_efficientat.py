"""EfficientAT mn10_as (MIT, Florian Schmid) as ONNX for flikk-analysis's
sound tagger.

The graph takes 32 kHz mono samples, (windows, samples), and returns the
527 AudioSet class probabilities of each window. Inside it: the log-mel
front end of EfficientAT's `AugmentMelSTFT` in eval mode (pre-emphasis,
800-sample Hann window in a 1024-point frame, hop 320, 128 Kaldi mel banks
from 0 to 15 kHz, log, (x + 4.5) / 5), the MobileNetV3 and a sigmoid.

    git clone https://github.com/fschmid56/EfficientAT
    python export_efficientat.py --repo EfficientAT --out efficientat-mn10-as.onnx
"""

import argparse
import os
import sys

import numpy as np
import onnxruntime as ort
import torch
import torch.nn as nn
from torchaudio.compliance.kaldi import get_mel_banks

from common import RATE, export


class Frontend(nn.Module):
    def __init__(self):
        super().__init__()
        banks, _ = get_mel_banks(128, 1024, RATE, 0.0, 15000.0, 100.0, -500.0, 1.0)
        self.register_buffer("mel", nn.functional.pad(banks, (0, 1)))
        self.register_buffer("window", torch.hann_window(800, periodic=False))
        self.register_buffer("emphasis", torch.tensor([[[-0.97, 1.0]]]))

    def forward(self, x):
        x = nn.functional.conv1d(x.unsqueeze(1), self.emphasis).squeeze(1)
        spec = torch.stft(
            x, 1024, hop_length=320, win_length=800, window=self.window,
            center=True, return_complex=True,
        )
        power = torch.view_as_real(spec).pow(2).sum(-1)
        mel = torch.matmul(self.mel, power)
        return ((mel + 1e-5).log() + 4.5) / 5.0


class Tagger(nn.Module):
    def __init__(self, net):
        super().__init__()
        self.frontend = Frontend()
        self.net = net

    def forward(self, samples):
        x = self.frontend(samples).unsqueeze(1)
        for layer in self.net.features:
            x = layer(x)
        logits = self.net.classifier(x).reshape(samples.shape[0], -1)
        return torch.sigmoid(logits)


def main():
    p = argparse.ArgumentParser()
    p.add_argument("--repo", required=True, help="clone of github.com/fschmid56/EfficientAT")
    p.add_argument("--out", default="efficientat-mn10-as.onnx")
    a = p.parse_args()
    out = os.path.abspath(a.out)
    os.chdir(a.repo)
    sys.path.insert(0, os.getcwd())
    from models.mn.model import get_model
    from models.preprocess import AugmentMelSTFT

    net = get_model(width_mult=1.0, pretrained_name="mn10_as").eval()
    tagger = Tagger(net).eval()

    batch = torch.export.Dim("windows", min=1, max=256)
    length = torch.export.Dim("samples", min=RATE // 2, max=RATE * 60)
    example = torch.randn(2, 2 * RATE) * 0.1
    export(tagger, (example,), out, ["samples"], ["probabilities"],
           {"samples": {0: batch, 1: length}})

    # Same answers as EfficientAT's own pipeline.
    reference = AugmentMelSTFT(n_mels=128, sr=RATE, win_length=800, hopsize=320).eval()
    rng = np.random.default_rng(0)
    session = ort.InferenceSession(out, providers=["CPUExecutionProvider"])
    for n in (RATE, 2 * RATE, 10 * RATE + 123):
        x = torch.from_numpy(rng.standard_normal((3, n)).astype(np.float32) * 0.1)
        with torch.no_grad():
            want = torch.sigmoid(net(reference(x).unsqueeze(1))[0]).numpy()
        got = session.run(None, {"samples": x.numpy()})[0]
        print(f"{n} samples: max difference {np.abs(want - got).max():.2e}")


if __name__ == "__main__":
    main()
