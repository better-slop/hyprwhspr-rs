# TIP live transcription tests

Ignored, expensive, prod-ish tests for transcription + normalization correctness and timing.

## Method

- Input audio: `tests/fixtures/standard-script-recording.wav`
- Expected output: `tests/fixtures/golden-script-expected-transform.txt`
- Config: real `ConfigManager::load()`
- Overrides: always empty `word_overrides`
- Providers: configured by the provider matrix in `live_transcription.rs`
- Metrics: timing table, backend metrics, phase CPU from `getrusage`, RSS from `/proc/self/status`
- Failures: wrapped text-pipeline block plus `similar` unified diff and colorized inline diff

`word_overrides` stay out of this suite on purpose. They are user-local aliases after the shared pipeline and can hide ITN/spacing bugs.

## Profiling model

- Always-on: `TipResourceTimeline` wraps fixture load, config load, fast VAD, backend init, backend transcription, and normalization.
- Backend internals: Groq and whisper.cpp emit backend phase metrics for encode/request/temp-WAV/CLI; request upload/response splits still come from `BackendMetrics` timing fields.
- Test allocation stacks: use `tests/tip/profile.sh` with `heaptrack`; it targets a report-only test so the process exits cleanly and heaptrack can show frees.
- App allocation stacks: use `tests/tip/profile-app.sh` or `target/debug/tip-profile`; this avoids libtest frames and is the better leak-attribution target.

## Run

Cheap compile gate:

```bash
cargo test --all-targets
```

Current normalization target:

```bash
cargo test --test tip_live_transcription tip_normalization_golden_script_matches_expected_transform -- --ignored --nocapture
```

All selected live cases:

```bash
cargo test --test tip_live_transcription tip_selected_live_transcription_matches_expected_transform -- --ignored --nocapture
```

Target providers/modes:

```bash
HYPRWHSPR_TIP_PROVIDERS=groq HYPRWHSPR_TIP_FAST_VAD=disabled cargo test --test tip_live_transcription tip_selected_live_transcription_matches_expected_transform -- --ignored --nocapture
HYPRWHSPR_TIP_PROVIDERS=whisper_cpp HYPRWHSPR_TIP_FAST_VAD=enabled cargo test --test tip_live_transcription tip_selected_live_transcription_matches_expected_transform -- --ignored --nocapture
HYPRWHSPR_TIP_PROVIDERS=groq,whisper_cpp HYPRWHSPR_TIP_FAST_VAD=both cargo test --test tip_live_transcription tip_selected_live_transcription_matches_expected_transform -- --ignored --nocapture
```

Allocation-stack profile:

```bash
HYPRWHSPR_TIP_PROVIDERS=whisper_cpp HYPRWHSPR_TIP_FAST_VAD=enabled tests/tip/profile.sh
```

Standalone app profile:

```bash
tests/tip/profile-app.sh whisper_cpp enabled
```

Manual standalone app profile:

```bash
cargo build --bin tip-profile --features tip-profile
heaptrack --record-only target/debug/tip-profile --provider whisper_cpp --fast-vad enabled
heaptrack_print --print-leaks --print-peaks heaptrack.*.zst
```

Profile a specific clean-exit test:

```bash
HYPRWHSPR_TIP_PROVIDERS=groq HYPRWHSPR_TIP_FAST_VAD=disabled tests/tip/profile.sh tip_profile_live_transcription_finishes_without_assertion
```

Individual cases:

```bash
cargo test --test tip_live_transcription tip_groq_without_fast_vad_matches_expected_transform -- --ignored --nocapture
cargo test --test tip_live_transcription tip_groq_with_fast_vad_matches_expected_transform -- --ignored --nocapture
cargo test --test tip_live_transcription tip_whisper_cpp_without_fast_vad_matches_expected_transform -- --ignored --nocapture
cargo test --test tip_live_transcription tip_whisper_cpp_with_fast_vad_matches_expected_transform -- --ignored --nocapture
```
