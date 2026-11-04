"""Download the speech the edit was fitted and measured on into the work folder:
LibriSpeech (73 clips), MLS dev in Spanish, French, German, Italian and
Portuguese, and the LibriVox reading used for English training speech."""
import os, sys, tarfile
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from huggingface_hub import hf_hub_download, list_repo_files

S = os.environ.get("WHISPER_EDIT_WORK") or os.path.join(
    os.environ.get("FLIKK_HOME") or os.path.expanduser("~/.flikk"), "whisper-edit")
data = f"{S}/data"
hf_hub_download("hf-internal-testing/librispeech_asr_dummy", "clean/validation-00000-of-00001.parquet",
                repo_type="dataset", local_dir=f"{data}/ls_en")
os.makedirs(f"{S}/audio", exist_ok=True)
p = hf_hub_download("Narsil/asr_dummy", "canterville.ogg", repo_type="dataset", local_dir=f"{S}/audio")
mls = "facebook/multilingual_librispeech"
files = list_repo_files(mls, repo_type="dataset")
shards = {"spanish": ["10367_10282_000", "133_1378_000", "2308_1378_000"]}
for lang in ["spanish", "french", "german", "italian", "portuguese"]:
    out = f"{data}/mls_{'es' if lang == 'spanish' else lang}"
    os.makedirs(out, exist_ok=True)
    tars = sorted(f for f in files if f"mls_{lang}/dev/audio/" in f and f.endswith(".tar.gz"))
    if lang in shards:
        tars = [f for f in tars if any(s in f for s in shards[lang])]
    else:
        tars = tars[:2]
    for f in tars:
        tarfile.open(hf_hub_download(mls, f, repo_type="dataset", local_dir=out)).extractall(out)
    hf_hub_download(mls, f"data/mls_{lang}/dev/transcripts.txt", repo_type="dataset", local_dir=out)
    print(lang, len([x for x in os.listdir(out) if x.endswith(".flac")]), "clips")
