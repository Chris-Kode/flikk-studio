#!/bin/bash
# Pre-fetch crates in Claude Code cloud sessions. No-op locally.
set -euo pipefail

if [ "${CLAUDE_CODE_REMOTE:-}" != "true" ]; then
  exit 0
fi

cd "${CLAUDE_PROJECT_DIR:-$(dirname "$0")/..}"
cargo fetch --locked
