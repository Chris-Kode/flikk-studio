# Sound model exports

The three sound models koo-analysis runs (`crates/koo-analysis/src/tagger.rs`,
`beats.rs`, `clap.rs`) are ONNX exports made by these scripts. Every graph
takes 32 kHz mono samples and holds its model's own front end, so analysis
decodes a file once for all of them. Each script checks its graph against the
original pipeline and prints how far apart they are.

| Script | Writes | From | Licence | Hosted at |
| --- | --- | --- | --- | --- |
| `export_efficientat.py` | `efficientat-mn10-as.onnx` | EfficientAT `mn10_as` (GitHub release of fschmid56/EfficientAT) | MIT | `Chris-Kode/efficientat-mn10-as-onnx` |
| `export_beat_this.py` | `beat-this-final0.onnx` | Beat This! `final0` (JKU's server, through the `beat-this` package) | MIT | `Chris-Kode/beat-this-final0-onnx` |
| `export_clap.py` | `larger-clap-general-audio.onnx`, `larger-clap-general-text.onnx` | `laion/larger_clap_general` on Hugging Face | Apache-2.0 | `Chris-Kode/larger-clap-general-onnx` |

## Build

```sh
python3 -m venv venv
venv/bin/pip install torch==2.14.1 torchaudio==2.11.0 torchvision==0.29.1 --index-url https://download.pytorch.org/whl/cpu
venv/bin/pip install -r requirements.txt
git clone https://github.com/fschmid56/EfficientAT
venv/bin/python export_efficientat.py --repo EfficientAT
venv/bin/python export_beat_this.py
venv/bin/python export_clap.py
```

`export_clap.py` also leaves the float graphs (`*.f32.onnx`) it quantized;
they are not shipped.

## Publish

Each repository holds its `.onnx` files, the script that made them with
`common.py`, the source model's licence text, and a README saying it is a
modified (exported, and for CLAP quantized) version of that model:

```sh
huggingface-cli upload Chris-Kode/efficientat-mn10-as-onnx . --include "efficientat-mn10-as.onnx" "export_efficientat.py" "common.py" "LICENSE" "README.md"
```

The URLs in the Rust files read `resolve/main/`. Once a file is up, pin its
URL to the commit, as `forced.rs` does for the aligner. Until then, point
`KOOEDITOR_TAGGER_MODEL`, `KOOEDITOR_BEATS_MODEL`, `KOOEDITOR_CLAP_AUDIO_MODEL`
and `KOOEDITOR_CLAP_TEXT_MODEL` at local copies.
