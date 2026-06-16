#!/usr/bin/env bash
set -euo pipefail

case_name="${1:-tip_profile_live_transcription_finishes_without_assertion}"

build_output="$(cargo test --test tip_live_transcription --no-run 2>&1)"
printf '%s\n' "${build_output}"

test_binary="$(
  printf '%s\n' "${build_output}" \
    | sed -n 's/^.*Executable .* (\(.*\))$/\1/p' \
    | tail -n 1
)"

if [[ -z "${test_binary}" ]]; then
  test_binary="$(
    find target/debug/deps \
      -maxdepth 1 \
      -type f \
      -perm -111 \
      -name 'tip_live_transcription-*' \
      -printf '%T@ %p\n' \
      | sort -nr \
      | awk 'NR == 1 { print $2 }'
  )"
fi

if [[ -z "${test_binary}" ]]; then
  echo "tip_live_transcription test binary not found" >&2
  exit 1
fi

if ! "${test_binary}" --list | grep -Fq "${case_name}: test"; then
  echo "selected test binary does not contain ${case_name}: ${test_binary}" >&2
  echo "available tests:" >&2
  "${test_binary}" --list >&2
  exit 1
fi

echo "profiling ${test_binary}"
echo "case ${case_name}"

if ! command -v heaptrack >/dev/null 2>&1; then
  echo "heaptrack is required for allocation-stack profiling" >&2
  echo "Install heaptrack, then rerun: $0 ${case_name}" >&2
  exit 127
fi

# Use the profile-only test by default. It runs the same live provider/mode
# matrix but reports correctness diffs instead of panicking, so heaptrack sees
# normal teardown and can show which allocations are freed at process exit.
heaptrack --record-only "${test_binary}" --ignored --nocapture --exact "${case_name}"

if command -v heaptrack_print >/dev/null 2>&1; then
  latest_profile="$(ls -t heaptrack.*.zst heaptrack.*.gz 2>/dev/null | head -n 1)"
  if [[ -n "${latest_profile}" ]]; then
    heaptrack_print "${latest_profile}"
  fi
fi
