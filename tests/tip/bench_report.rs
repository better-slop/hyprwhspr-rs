use crate::live_harness::{FastVadMode, SAMPLE_RATE_HZ};
use crate::resource_timeline::TipPhaseSample;
use crate::resource_usage::ResourceDelta;
use comfy_table::modifiers::UTF8_ROUND_CORNERS;
use comfy_table::presets::UTF8_FULL;
use comfy_table::{Cell, CellAlignment, ContentArrangement, Row, Table};
use hyprwhspr_rs::transcription::{BackendMetrics, BackendPhaseMetric};
use std::time::Duration;

const DASH: &str = "—";

pub(crate) struct TipBenchmarkInput<'a> {
    pub(crate) provider_label: &'static str,
    pub(crate) provider_id: &'static str,
    pub(crate) fast_vad_mode: FastVadMode,
    pub(crate) original_samples: usize,
    pub(crate) sample_rate_hz: u32,
    pub(crate) preprocess_duration: Duration,
    pub(crate) provider_init_duration: Duration,
    pub(crate) audio_sent_samples: usize,
    pub(crate) fast_vad_duration: Option<Duration>,
    pub(crate) fast_vad_segments: Option<usize>,
    pub(crate) fast_vad_dropped_samples: Option<usize>,
    pub(crate) backend_wall_duration: Duration,
    pub(crate) backend_metrics: &'a BackendMetrics,
    pub(crate) normalize_duration: Duration,
    pub(crate) total_duration: Duration,
    pub(crate) resource_delta: ResourceDelta,
    pub(crate) timeline_samples: &'a [TipPhaseSample],
    pub(crate) raw_transcript: &'a str,
    pub(crate) normalized: &'a str,
    pub(crate) expected: &'a str,
}

pub(crate) fn print_case_report(input: TipBenchmarkInput<'_>) {
    println!();
    println!("{}", render_benchmark_table(&input));
    println!();
    println!("provider_id: {}", input.provider_id);
    println!("fast_vad: {}", input.fast_vad_mode.label());
    if let Some(segments) = input.fast_vad_segments {
        println!("fast_vad_segments: {segments}");
    }
    if let Some(dropped) = input.fast_vad_dropped_samples {
        println!("fast_vad_dropped_samples: {dropped}");
    }
    println!("cpu_user_ms: {:.3}", ms(input.resource_delta.user_cpu));
    println!("cpu_system_ms: {:.3}", ms(input.resource_delta.system_cpu));
    println!(
        "self_cpu_ms: {:.3}",
        ms(input.resource_delta.self_user_cpu + input.resource_delta.self_system_cpu)
    );
    println!(
        "child_cpu_ms: {:.3}",
        ms(input.resource_delta.child_user_cpu + input.resource_delta.child_system_cpu)
    );
    println!("cpu_total_ms: {:.3}", ms(input.resource_delta.total_cpu));
    println!(
        "cpu_percent: {}",
        input
            .resource_delta
            .cpu_percent
            .map(|value| format!("{value:.1}"))
            .unwrap_or_else(|| DASH.to_string())
    );
    println!(
        "rss_start_kb: {}",
        kb_text(input.resource_delta.rss_start_kb)
    );
    println!("rss_end_kb: {}", kb_text(input.resource_delta.rss_end_kb));
    println!(
        "rss_delta_kb: {}",
        input
            .resource_delta
            .rss_delta_kb
            .map(|value| value.to_string())
            .unwrap_or_else(|| DASH.to_string())
    );
    println!(
        "rss_high_water_kb: {}",
        kb_text(input.resource_delta.high_water_rss_kb)
    );
    println!("max_rss_kb: {}", kb_text(input.resource_delta.max_rss_kb));
    println!("--- resource timeline ---");
    for sample in input.timeline_samples {
        println!(
            "{} wall_ms={:.3} cpu_ms={:.3} self_cpu_ms={:.3} child_cpu_ms={:.3} cpu_percent={} rss_delta_kb={} rss_hwm_kb={} bytes_in={} bytes_out={}",
            sample.name,
            ms(sample.wall_duration),
            ms(sample.resource_delta.total_cpu),
            ms(sample.resource_delta.self_user_cpu + sample.resource_delta.self_system_cpu),
            ms(sample.resource_delta.child_user_cpu + sample.resource_delta.child_system_cpu),
            sample
                .resource_delta
                .cpu_percent
                .map(|value| format!("{value:.1}"))
                .unwrap_or_else(|| DASH.to_string()),
            sample
                .resource_delta
                .rss_delta_kb
                .map(|value| value.to_string())
                .unwrap_or_else(|| DASH.to_string()),
            kb_text(sample.resource_delta.high_water_rss_kb),
            sample
                .bytes_in
                .map(|value| value.to_string())
                .unwrap_or_else(|| DASH.to_string()),
            sample
                .bytes_out
                .map(|value| value.to_string())
                .unwrap_or_else(|| DASH.to_string())
        );
    }
    if !input.backend_metrics.phases.is_empty() {
        println!("--- backend phase timeline ---");
        for phase in &input.backend_metrics.phases {
            println!(
                "{} wall_ms={:.3} cpu_ms={:.3} self_cpu_ms={:.3} child_cpu_ms={:.3} cpu_percent={} rss_delta_kb={} rss_hwm_kb={} max_rss_kb={} bytes_in={} bytes_out={}",
                phase.name,
                ms(phase.wall_duration),
                ms(phase.resource_delta.total_cpu),
                ms(phase.resource_delta.self_user_cpu + phase.resource_delta.self_system_cpu),
                ms(phase.resource_delta.child_user_cpu + phase.resource_delta.child_system_cpu),
                phase
                    .resource_delta
                    .cpu_percent
                    .map(|value| format!("{value:.1}"))
                    .unwrap_or_else(|| DASH.to_string()),
                phase
                    .resource_delta
                    .rss_delta_kb
                    .map(|value| value.to_string())
                    .unwrap_or_else(|| DASH.to_string()),
                kb_text(phase.resource_delta.high_water_rss_kb),
                kb_text(phase.resource_delta.max_rss_kb),
                phase
                    .bytes_in
                    .map(|value| value.to_string())
                    .unwrap_or_else(|| DASH.to_string()),
                phase
                    .bytes_out
                    .map(|value| value.to_string())
                    .unwrap_or_else(|| DASH.to_string())
            );
        }
    }
    println!("--- raw transcript ---");
    println!("{}", input.raw_transcript);
    println!("--- normalized transcript ---");
    println!("{}", input.normalized);
    println!("--- expected transform fixture ---");
    println!("{}", input.expected);
    println!("=== end TIP benchmark ===");
}

fn render_benchmark_table(input: &TipBenchmarkInput<'_>) -> String {
    let mut table = Table::new();
    table
        .load_preset(UTF8_FULL)
        .apply_modifier(UTF8_ROUND_CORNERS)
        .set_content_arrangement(ContentArrangement::DynamicFullWidth)
        .force_no_tty();

    table.set_header(vec![
        Cell::new(format!(
            "TIP Benchmark · {} · {}",
            input.provider_label,
            input.fast_vad_mode.label()
        )),
        Cell::new("DUR (ms)"),
        Cell::new("Audio (ms)"),
        Cell::new("Audio (KB)"),
        Cell::new("CPU (ms)"),
        Cell::new("CPU %"),
        Cell::new("RSS Δ KB"),
    ]);

    for column in 1..7 {
        if let Some(col) = table.column_mut(column) {
            col.set_cell_alignment(CellAlignment::Right);
        }
    }

    let original_audio_ms = audio_ms(input.original_samples, input.sample_rate_hz);
    let original_audio_kb = raw_audio_kb(input.original_samples);
    let sent_audio_ms = audio_ms(input.audio_sent_samples, SAMPLE_RATE_HZ);
    let sent_audio_kb = input
        .backend_metrics
        .encoded_bytes
        .map(|bytes| bytes as f64 / 1024.0)
        .or_else(|| Some(raw_audio_kb(input.audio_sent_samples)));
    let processed_audio_kb = raw_audio_kb(input.audio_sent_samples);
    let saved_audio_ms =
        (original_audio_ms >= sent_audio_ms).then_some(original_audio_ms - sent_audio_ms);
    let saved_audio_kb =
        (original_audio_kb >= processed_audio_kb).then_some(original_audio_kb - processed_audio_kb);
    let saved_audio_pct = if original_audio_ms > 0.0 {
        Some((saved_audio_ms.unwrap_or_default() / original_audio_ms) * 100.0)
    } else {
        None
    };

    table.add_row(row(
        "Provider init",
        Some(ms(input.provider_init_duration)),
        None,
        None,
        phase_cpu_ms(input, "backend.init"),
        phase_cpu_percent(input, "backend.init"),
        phase_rss_delta_kb(input, "backend.init"),
    ));
    table.add_row(row(
        "Rec. active",
        Some(original_audio_ms),
        Some(original_audio_ms),
        Some(original_audio_kb),
        None,
        None,
        None,
    ));
    table.add_row(row(
        "Processing",
        Some(ms(input.preprocess_duration)),
        None,
        None,
        phase_cpu_ms(input, "preprocess.fast_vad"),
        phase_cpu_percent(input, "preprocess.fast_vad"),
        phase_rss_delta_kb(input, "preprocess.fast_vad"),
    ));
    table.add_row(row(
        "Fast VAD Trim",
        input.fast_vad_duration.map(ms),
        Some(sent_audio_ms),
        Some(processed_audio_kb),
        input
            .fast_vad_duration
            .and_then(|_| phase_cpu_ms(input, "preprocess.fast_vad")),
        input
            .fast_vad_duration
            .and_then(|_| phase_cpu_percent(input, "preprocess.fast_vad")),
        input
            .fast_vad_duration
            .and_then(|_| phase_rss_delta_kb(input, "preprocess.fast_vad")),
    ));
    table.add_row(row(
        "Encode",
        input.backend_metrics.encode_duration.map(ms),
        None,
        input
            .backend_metrics
            .encoded_bytes
            .map(|bytes| bytes as f64 / 1024.0),
        backend_encode_phase(input).map(backend_phase_cpu_ms),
        backend_encode_phase(input).and_then(|phase| phase.resource_delta.cpu_percent),
        backend_encode_phase(input).and_then(backend_phase_rss_delta_kb),
    ));
    table.add_row(row(
        "Upload",
        input.backend_metrics.upload_duration.map(ms),
        None,
        None,
        None,
        None,
        None,
    ));
    table.add_row(row(
        "Response",
        input.backend_metrics.response_duration.map(ms),
        None,
        None,
        None,
        None,
        None,
    ));
    table.add_row(row(
        "Transcription",
        Some(ms(input.backend_metrics.transcription_duration)),
        Some(sent_audio_ms),
        sent_audio_kb,
        backend_transcription_phase(input).map(backend_phase_cpu_ms),
        backend_transcription_phase(input).and_then(|phase| phase.resource_delta.cpu_percent),
        backend_transcription_phase(input).and_then(backend_phase_rss_delta_kb),
    ));
    table.add_row(row(
        "Transcribe wall",
        Some(ms(input.backend_wall_duration)),
        Some(sent_audio_ms),
        sent_audio_kb,
        phase_cpu_ms(input, "backend.transcribe"),
        phase_cpu_percent(input, "backend.transcribe"),
        phase_rss_delta_kb(input, "backend.transcribe"),
    ));
    table.add_row(row(
        "Normalization",
        Some(ms(input.normalize_duration)),
        None,
        None,
        phase_cpu_ms(input, "normalize.total"),
        phase_cpu_percent(input, "normalize.total"),
        phase_rss_delta_kb(input, "normalize.total"),
    ));
    table.add_row(row(
        "Total",
        Some(ms(input.total_duration)),
        None,
        None,
        Some(ms(input.resource_delta.total_cpu)),
        input.resource_delta.cpu_percent,
        input.resource_delta.rss_delta_kb.map(|value| value as f64),
    ));
    table.add_row(Row::from(vec![
        Cell::new("Fast VAD Savings"),
        savings_cell(saved_audio_ms, saved_audio_pct),
        ms_cell(saved_audio_ms),
        kb_cell(saved_audio_kb),
        empty_cell(),
        empty_cell(),
        empty_cell(),
    ]));

    table.trim_fmt()
}

fn phase_cpu_ms(input: &TipBenchmarkInput<'_>, name: &str) -> Option<f64> {
    phase(input, name).map(|sample| ms(sample.resource_delta.total_cpu))
}

fn phase_cpu_percent(input: &TipBenchmarkInput<'_>, name: &str) -> Option<f64> {
    phase(input, name).and_then(|sample| sample.resource_delta.cpu_percent)
}

fn phase_rss_delta_kb(input: &TipBenchmarkInput<'_>, name: &str) -> Option<f64> {
    phase(input, name)
        .and_then(|sample| sample.resource_delta.rss_delta_kb.map(|value| value as f64))
}

fn phase<'a>(input: &'a TipBenchmarkInput<'_>, name: &str) -> Option<&'a TipPhaseSample> {
    input
        .timeline_samples
        .iter()
        .rev()
        .find(|sample| sample.name == name)
}

fn backend_encode_phase<'a>(input: &'a TipBenchmarkInput<'_>) -> Option<&'a BackendPhaseMetric> {
    input.backend_metrics.phases.iter().find(|phase| {
        matches!(
            phase.name,
            "backend.encode.flac" | "backend.whisper.temp_wav"
        )
    })
}

fn backend_transcription_phase<'a>(
    input: &'a TipBenchmarkInput<'_>,
) -> Option<&'a BackendPhaseMetric> {
    input
        .backend_metrics
        .phases
        .iter()
        .find(|phase| matches!(phase.name, "backend.groq.request" | "backend.whisper.cli"))
}

fn backend_phase_cpu_ms(phase: &BackendPhaseMetric) -> f64 {
    ms(phase.resource_delta.total_cpu)
}

fn backend_phase_rss_delta_kb(phase: &BackendPhaseMetric) -> Option<f64> {
    phase.resource_delta.rss_delta_kb.map(|value| value as f64)
}

fn row(
    label: &str,
    duration_ms: Option<f64>,
    audio_ms: Option<f64>,
    audio_kb: Option<f64>,
    cpu_ms: Option<f64>,
    cpu_pct: Option<f64>,
    rss_delta_kb: Option<f64>,
) -> Row {
    Row::from(vec![
        Cell::new(label),
        ms_cell(duration_ms),
        ms_cell(audio_ms),
        kb_cell(audio_kb),
        ms_cell(cpu_ms),
        pct_cell(cpu_pct),
        kb_cell(rss_delta_kb),
    ])
}

fn ms(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1000.0
}

fn audio_ms(samples: usize, sample_rate: u32) -> f64 {
    samples as f64 / sample_rate as f64 * 1000.0
}

fn raw_audio_kb(samples: usize) -> f64 {
    (samples * std::mem::size_of::<f32>()) as f64 / 1024.0
}

fn ms_cell(value: Option<f64>) -> Cell {
    value_cell(value)
}

fn kb_cell(value: Option<f64>) -> Cell {
    value_cell(value)
}

fn pct_cell(value: Option<f64>) -> Cell {
    value_cell(value)
}

fn savings_cell(value: Option<f64>, pct: Option<f64>) -> Cell {
    let content = match (value, pct) {
        (Some(v), Some(p)) => format!("{v:.1} ({p:.1}%)"),
        (Some(v), None) => format!("{v:.1}"),
        _ => DASH.to_string(),
    };
    Cell::new(content).set_alignment(CellAlignment::Right)
}

fn value_cell(value: Option<f64>) -> Cell {
    let content = value
        .map(|value| format!("{value:.1}"))
        .unwrap_or_else(|| DASH.to_string());
    Cell::new(content).set_alignment(CellAlignment::Right)
}

fn empty_cell() -> Cell {
    Cell::new(DASH).set_alignment(CellAlignment::Right)
}

fn kb_text(value: Option<u64>) -> String {
    value
        .map(|value| value.to_string())
        .unwrap_or_else(|| DASH.to_string())
}
