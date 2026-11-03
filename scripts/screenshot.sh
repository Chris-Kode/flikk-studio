#!/bin/bash
# Launch the editor under Xvfb and save a screenshot (Linux dev containers only).
#   scripts/screenshot.sh out.png [seconds-to-wait]
set -euo pipefail

out="$(realpath -m "${1:?usage: screenshot.sh out.png [wait]}")"
wait="${2:-8}"
cd "$(dirname "$0")/.."
cargo build -q -p flikk
export FLIKK_HOME="${FLIKK_HOME:-/tmp/flikk-dev}"
xvfb-run -a -s "-screen 0 1440x900x24" bash -c "
  target/debug/flikk & pid=\$!
  sleep $wait
  import -window root '$out'
  kill \$pid"
echo "$out"
