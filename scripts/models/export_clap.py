"""LAION's larger_clap_general (Apache-2.0) as two ONNX graphs for
koo-analysis's sound search.

Audio: 10 s of 32 kHz mono samples, (windows, 320000), in; one unit-length
512-number vector per window out. Shorter windows are repeated to fill the
10 s and padded with zeros, as `ClapFeatureExtractor` does ("repeatpad").
Inside it: a windowed-sinc resampler to 48 kHz, the feature extractor's
log-mel spectrogram (1024-point Hann frames, hop 480, 64 slaney mels from
50 Hz to 14 kHz, power in dB) and the audio tower with its projection.

Text: RoBERTa token ids and attention mask, (texts, tokens), in; one
unit-length 512-number vector per text out. The tokenizer files are the
model's own `vocab.json` and `merges.txt`.

The audio graph runs on every stretch of audio, so its MatMuls are dynamic
int8 (cosine 0.997 to the float model); the text graph runs once per query
and keeps float activations with 8-bit weights in blocks of 32 and an int8
embedding table (cosine 0.998). `--keep-float` skips both.

    python export_clap.py --out-dir .
"""

import argparse
import os

import numpy as np
import onnxruntime as ort
import torch
import torch.nn as nn
from transformers import ClapModel, ClapProcessor

from common import RATE, Resample, export, quantize, quantize_weights

MODEL = "laion/larger_clap_general"
SAMPLES = 10 * RATE


class Audio(nn.Module):
    def __init__(self, model, mel_filters):
        super().__init__()
        self.resample = Resample(RATE, 48000)
        self.register_buffer("mel", torch.from_numpy(mel_filters.T.astype(np.float32)))
        self.register_buffer("window", torch.hann_window(1024, periodic=True))
        self.model = model

    def forward(self, samples):
        x = self.resample(samples)
        spec = torch.stft(x, 1024, hop_length=480, window=self.window, center=True,
                          pad_mode="reflect", return_complex=True)
        power = torch.view_as_real(spec).pow(2).sum(-1)
        mel = torch.matmul(self.mel, power).clamp(min=1e-10)
        db = 10.0 * torch.log10(mel)
        features = db.transpose(1, 2).unsqueeze(1)
        return self.model.get_audio_features(input_features=features).pooler_output


class Text(nn.Module):
    def __init__(self, model):
        super().__init__()
        self.model = model

    def forward(self, input_ids, attention_mask):
        return self.model.get_text_features(
            input_ids=input_ids, attention_mask=attention_mask
        ).pooler_output


def cos(a, b):
    a = a / np.linalg.norm(a, axis=-1, keepdims=True)
    b = b / np.linalg.norm(b, axis=-1, keepdims=True)
    return (a * b).sum(-1)


def main():
    p = argparse.ArgumentParser()
    p.add_argument("--out-dir", default=".")
    p.add_argument("--keep-float", action="store_true")
    a = p.parse_args()
    os.makedirs(a.out_dir, exist_ok=True)
    model = ClapModel.from_pretrained(MODEL).eval()
    processor = ClapProcessor.from_pretrained(MODEL)
    fe = processor.feature_extractor

    audio_path = os.path.join(a.out_dir, "larger-clap-general-audio.f32.onnx")
    text_path = os.path.join(a.out_dir, "larger-clap-general-text.f32.onnx")
    windows = torch.export.Dim("windows", min=1, max=64)
    export(Audio(model, fe.mel_filters_slaney), (torch.randn(2, SAMPLES) * 0.1,), audio_path,
           ["samples"], ["embedding"], {"samples": {0: windows}})
    texts = torch.export.Dim("texts", min=1, max=64)
    tokens = torch.export.Dim("tokens", min=2, max=77)
    ids = processor.tokenizer(["a dog barks", "glass breaking"], padding=True, return_tensors="pt")
    export(Text(model), (ids["input_ids"], ids["attention_mask"]), text_path,
           ["input_ids", "attention_mask"], ["embedding"],
           {"input_ids": {0: texts, 1: tokens}, "attention_mask": {0: texts, 1: tokens}})

    final = {"audio": audio_path, "text": text_path}
    if not a.keep_float:
        final = {k: v.replace(".f32.onnx", ".onnx") for k, v in final.items()}
        quantize(audio_path, final["audio"])
        quantize_weights(text_path, final["text"])

    # Against transformers on the same audio at 48 kHz.
    rng = np.random.default_rng(0)
    t = np.arange(SAMPLES) / RATE
    clips = np.stack([
        (np.sin(2 * np.pi * 440 * t) * (t % 1 < 0.3)).astype(np.float32) * 0.3,
        (rng.standard_normal(SAMPLES) * np.exp(-(t % 0.5) * 20)).astype(np.float32) * 0.2,
    ])
    up = Resample(RATE, 48000)(torch.from_numpy(clips)).numpy()
    with torch.no_grad():
        inputs = fe(list(up), sampling_rate=48000, return_tensors="pt")
        want_audio = model.get_audio_features(**inputs).pooler_output.numpy()
        texts = ["a beeping tone", "rain on a roof", "a door slams"]
        tok = processor.tokenizer(texts, padding=True, return_tensors="np")
        want_text = model.get_text_features(
            input_ids=torch.from_numpy(tok["input_ids"]),
            attention_mask=torch.from_numpy(tok["attention_mask"]),
        ).pooler_output.numpy()
    for name, path in final.items():
        s = ort.InferenceSession(path, providers=["CPUExecutionProvider"])
        if name == "audio":
            got = s.run(None, {"samples": clips})[0]
            print(f"audio {os.path.basename(path)}: cosine to reference {cos(got, want_audio)}")
        else:
            got = s.run(None, {"input_ids": tok["input_ids"].astype(np.int64),
                               "attention_mask": tok["attention_mask"].astype(np.int64)})[0]
            print(f"text {os.path.basename(path)}: cosine to reference {cos(got, want_text)}")
    print("token ids", [list(r) for r in tok["input_ids"]])


if __name__ == "__main__":
    main()
