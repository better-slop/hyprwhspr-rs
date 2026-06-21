use anyhow::Result;

mod bench_report;
mod correctness_score;
mod diff_report;
mod live_harness;
mod resource_timeline;
mod resource_usage;

use live_harness::{
    CorrectnessMode, FastVadMode, run_normalization_golden_assertion, run_provider_case,
    run_selected_live_cases,
};

#[test]
#[ignore = "TIP golden test: expected to fail until NormalizeTextService reaches the desired target"]
fn tip_normalization_golden_script_matches_expected_transform() -> Result<()> {
    run_normalization_golden_assertion()
}

#[tokio::test]
#[ignore = "TIP live inference: selected providers/modes consume remote or local compute"]
async fn tip_selected_live_transcription_matches_expected_transform() -> Result<()> {
    run_selected_live_cases(CorrectnessMode::Assert).await
}

#[tokio::test]
#[ignore = "TIP profiling: runs live inference and exits cleanly for heaptrack/flame charts"]
async fn tip_profile_live_transcription_finishes_without_assertion() -> Result<()> {
    run_selected_live_cases(CorrectnessMode::ReportOnly).await
}

#[tokio::test]
#[ignore = "TIP live inference: calls Groq and consumes paid/remote compute"]
async fn tip_groq_without_fast_vad_matches_expected_transform() -> Result<()> {
    run_provider_case("groq", FastVadMode::Disabled, CorrectnessMode::Assert).await
}

#[tokio::test]
#[ignore = "TIP live inference: calls Groq and consumes paid/remote compute"]
async fn tip_groq_with_fast_vad_matches_expected_transform() -> Result<()> {
    run_provider_case("groq", FastVadMode::Enabled, CorrectnessMode::Assert).await
}

#[tokio::test]
#[ignore = "TIP live inference: runs local whisper.cpp"]
async fn tip_whisper_cpp_without_fast_vad_matches_expected_transform() -> Result<()> {
    run_provider_case(
        "whisper_cpp",
        FastVadMode::Disabled,
        CorrectnessMode::Assert,
    )
    .await
}

#[tokio::test]
#[ignore = "TIP live inference: runs local whisper.cpp"]
async fn tip_whisper_cpp_with_fast_vad_matches_expected_transform() -> Result<()> {
    run_provider_case("whisper_cpp", FastVadMode::Enabled, CorrectnessMode::Assert).await
}
