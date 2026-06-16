# TIP live transcription tests

Ignored, expensive, prod-ish tests for transcription + normalization correctness and timing.

## Method

- Input audio: `tests/fixtures/standard-script-recording.wav`
- Expected output: `tests/fixtures/golden-script-expected-transform.txt`
- Config: real `ConfigManager::load()`
- Overrides: always empty `word_overrides`
- Providers: configured by the provider matrix in `live_transcription.rs`
- Metrics: timing table, backend metrics, CPU from `getrusage`, RSS from `/proc/self/status`
- Failures: wrapped text-pipeline block plus line-numbered `similar` diff with inline word highlights

`word_overrides` stay out of this suite on purpose. They are user-local aliases after the shared pipeline and can hide ITN/spacing bugs.

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

Individual cases:

```bash
cargo test --test tip_live_transcription tip_groq_without_fast_vad_matches_expected_transform -- --ignored --nocapture
cargo test --test tip_live_transcription tip_groq_with_fast_vad_matches_expected_transform -- --ignored --nocapture
cargo test --test tip_live_transcription tip_whisper_cpp_without_fast_vad_matches_expected_transform -- --ignored --nocapture
cargo test --test tip_live_transcription tip_whisper_cpp_with_fast_vad_matches_expected_transform -- --ignored --nocapture
```
