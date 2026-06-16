#!/usr/bin/env bash
set -euo pipefail

case_name="${1:-tip_selected_live_transcription_matches_expected_transform}"

cargo test --test tip_live_transcription --no-run

test_binary="$(
  find target/debug/deps \
    -maxdepth 1 \
    -type f \
    -perm -111 \
    -name 'tip_live_transcription-*' \
    | sort \
    | tail -n 1
)"

if [[ -z "${test_binary}" ]]; then
  echo "tip_live_transcription test binary not found" >&2
  exit 1
fi

if ! command -v heaptrack >/dev/null 2>&1; then
  echo "heaptrack is required for allocation-stack profiling" >&2
  echo "Install heaptrack, then rerun: $0 ${case_name}" >&2
  exit 127
fi

heaptrack "${test_binary}" --ignored --nocapture "${case_name}"

if command -v heaptrack_print >/dev/null 2>&1; then
  latest_profile="$(ls -t heaptrack.*.gz 2>/dev/null | head -n 1)"
  if [[ -n "${latest_profile}" ]]; then
    heaptrack_print "${latest_profile}"
  fi
fi
