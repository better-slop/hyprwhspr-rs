use anyhow::Result;
use clap::Parser;

#[path = "../../tests/tip/bench_report.rs"]
mod bench_report;
#[path = "../../tests/tip/diff_report.rs"]
mod diff_report;
#[allow(dead_code)]
#[path = "../../tests/tip/live_harness.rs"]
mod live_harness;
#[path = "../../tests/tip/resource_timeline.rs"]
mod resource_timeline;
#[path = "../../tests/tip/resource_usage.rs"]
mod resource_usage;

use live_harness::{run_selected_live_cases, CorrectnessMode};

#[derive(Debug, Parser)]
#[command(
    name = "tip-profile",
    about = "Run the standard TIP transcription fixture outside libtest"
)]
struct Args {
    /// Comma-separated providers: groq, whisper_cpp, or all.
    #[arg(long)]
    provider: Option<String>,

    /// Fast VAD mode: enabled, disabled, or both.
    #[arg(long = "fast-vad")]
    fast_vad: Option<String>,
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();

    if let Some(provider) = args.provider {
        std::env::set_var("HYPRWHSPR_TIP_PROVIDERS", provider);
    }
    if let Some(fast_vad) = args.fast_vad {
        std::env::set_var("HYPRWHSPR_TIP_FAST_VAD", fast_vad);
    }

    run_selected_live_cases(CorrectnessMode::ReportOnly).await
}
