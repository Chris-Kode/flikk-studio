#!/bin/bash
# Pre-fetch crates in Claude Code cloud sessions. No-op locally.
set -euo pipefail

if [ "${CLAUDE_CODE_REMOTE:-}" != "true" ]; then
  exit 0
fi

cd "${CLAUDE_PROJECT_DIR:-$(dirname "$0")/..}"
cargo fetch --locked

# Software Vulkan (lavapipe) so the GPUI window opens under Xvfb for screenshots:
#   xvfb-run -a -s "-screen 0 1440x900x24" target/debug/koo-editor
if [ ! -e /usr/share/vulkan/icd.d/lvp_icd.json ] && command -v apt-get >/dev/null; then
  apt-get install -y -q mesa-vulkan-drivers >/dev/null 2>&1 \
    || { apt-get update -q >/dev/null 2>&1 && apt-get install -y -q mesa-vulkan-drivers >/dev/null 2>&1; } \
    || echo "mesa-vulkan-drivers not installed; headless screenshots unavailable" >&2
fi
