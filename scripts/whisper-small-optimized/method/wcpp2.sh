#!/bin/bash
# whisper.cpp stock decoding (beam 5, temperature fallback, conditioned on previous text) for one model
S=${WHISPER_EDIT_WORK:-${FLIKK_HOME:-$HOME/.flikk}/whisper-edit}
# cargo build --release --manifest-path method/wcli/Cargo.toml --target-dir "$S/wcli-target"
W=${WCLI:-$S/wcli-target/release/wcli}; tag=$1; bin=$2; o=$S/ws/wcpp_$tag; rm -rf $o; mkdir -p $o
NS=$(ls $S/sets/test_ns/*.f32 | awk 'NR%5==1')
ES=$(ls $S/sets/test_es/*.f32 | head -8); EN=$(ls $S/sets/test_en/*.f32 | head -8)
$W $bin es $NS > $o/ns_es.tsv 2>/dev/null
$W $bin auto $NS > $o/ns_auto.tsv 2>/dev/null
$W $bin es $S/sets/long/es0.f32 > $o/long_es.tsv 2>/dev/null
$W $bin en $S/sets/long/en1.f32 > $o/long_en.tsv 2>/dev/null
$W $bin es $ES > $o/es.tsv 2>/dev/null
$W $bin en $EN > $o/en.tsv 2>/dev/null
echo done $tag
