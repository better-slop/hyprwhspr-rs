#!/usr/bin/env bash
set -euo pipefail

provider="${HYPRWHSPR_TIP_PROVIDERS:-${1:-whisper_cpp}}"
fast_vad="${HYPRWHSPR_TIP_FAST_VAD:-${2:-enabled}}"

cargo build --bin tip-profile

if ! command -v heaptrack >/dev/null 2>&1; then
  echo "heaptrack is required for allocation-stack profiling" >&2
  echo "Install heaptrack, then rerun: $0 ${provider} ${fast_vad}" >&2
  exit 127
fi

echo "profiling target/debug/tip-profile"
echo "provider ${provider}"
echo "fast_vad ${fast_vad}"

heaptrack --record-only target/debug/tip-profile --provider "${provider}" --fast-vad "${fast_vad}"

if command -v heaptrack_print >/dev/null 2>&1; then
  latest_profile="$(ls -t heaptrack.*.zst heaptrack.*.gz 2>/dev/null | head -n 1)"
  if [[ -n "${latest_profile}" ]]; then
    heaptrack_print --print-leaks --print-peaks "${latest_profile}"
  fi
fi
