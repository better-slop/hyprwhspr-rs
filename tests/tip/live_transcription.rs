use anyhow::{bail, Context, Result};
use hyprwhspr_rs::audio::FastVad;
use hyprwhspr_rs::config::{Config, ConfigManager, TranscriptionProvider};
use hyprwhspr_rs::text::NormalizeTextService;
use hyprwhspr_rs::transcription::{BackendMetrics, TranscriptionBackend};
use hyprwhspr_rs::whisper::WhisperVadOptions;
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

const SAMPLE_RATE_HZ: u32 = 16_000;

#[test]
#[ignore = "TIP golden test: expected to fail until NormalizeTextService reaches the desired target"]
fn tip_normalization_golden_script_matches_expected_transform() -> Result<()> {
    let input = read_fixture_text("golden-script.txt")?;
    let expected = read_fixture_text("golden-script-expected-transform.txt")?;
    let actual = normalize_without_overrides(&input);

    assert_text_eq("NormalizeTextService golden script", &expected, &actual);
    Ok(())
}

#[tokio::test]
#[ignore = "TIP live inference: calls Groq and consumes paid/remote compute"]
async fn tip_groq_without_fast_vad_matches_expected_transform() -> Result<()> {
    run_live_case(TranscriptionProvider::Groq, FastVadMode::Disabled).await
}

#[tokio::test]
#[ignore = "TIP live inference: calls Groq and consumes paid/remote compute"]
async fn tip_groq_with_fast_vad_matches_expected_transform() -> Result<()> {
    run_live_case(TranscriptionProvider::Groq, FastVadMode::Enabled).await
}

#[tokio::test]
#[ignore = "TIP live inference: runs local whisper.cpp"]
async fn tip_whisper_cpp_without_fast_vad_matches_expected_transform() -> Result<()> {
    run_live_case(TranscriptionProvider::WhisperCpp, FastVadMode::Disabled).await
}

#[tokio::test]
#[ignore = "TIP live inference: runs local whisper.cpp"]
async fn tip_whisper_cpp_with_fast_vad_matches_expected_transform() -> Result<()> {
    run_live_case(TranscriptionProvider::WhisperCpp, FastVadMode::Enabled).await
}

async fn run_live_case(provider: TranscriptionProvider, fast_vad_mode: FastVadMode) -> Result<()> {
    let fixture = load_standard_recording()?;
    let expected = read_fixture_text("golden-script-expected-transform.txt")?;
    let config_manager = ConfigManager::load().context("load real hyprwhspr-rs config")?;
    let mut config = config_manager.get();
    config.transcription.provider = provider.clone();
    config.word_overrides.clear();
    config.transcription.whisper_cpp.vad.enabled = false;

    /*
    ```text
    This TIP suite intentionally normalizes with an empty word_overrides map.

    Do not change that for convenience. These tests are measuring the built-in
    transcription -> ITN -> NormalizeTextService contract for the standard script
    fixture. Custom word_overrides are user-specific aliases layered after the
    shared pipeline; they can mask missing ITN rules, hide spacing bugs, and make
    benchmark output depend on a developer's private config. If this fixture fails,
    fix the shared normalization pipeline or update the checked-in expected fixture
    after an intentional behavior change. Do not make the fixture pass by teaching
    one local config to rewrite the words.
    ```
    */
    let preprocessed = preprocess_for_case(&config, fast_vad_mode, &fixture)?;
    let vad_options = WhisperVadOptions::disabled();
    let backend = TranscriptionBackend::build(&config_manager, &config, vad_options)
        .with_context(|| format!("build {} backend", provider_label(&provider)))?;

    backend
        .initialize()
        .with_context(|| format!("initialize {} backend", provider_label(&provider)))?;

    let transcribe_start = Instant::now();
    let result = backend
        .transcribe(preprocessed.audio.clone())
        .await
        .with_context(|| format!("transcribe with {}", provider_label(&provider)))?;
    let wall_duration = transcribe_start.elapsed();
    let normalized = normalize_without_overrides(&result.text);

    print_case_report(CaseReport {
        provider: provider_label(&provider),
        fast_vad_mode,
        original_samples: fixture.samples.len(),
        audio_sent_samples: preprocessed.audio.len(),
        fast_vad_duration: preprocessed.fast_vad_duration,
        fast_vad_segments: preprocessed.fast_vad_segments,
        fast_vad_dropped_samples: preprocessed.fast_vad_dropped_samples,
        backend_wall_duration: wall_duration,
        backend_metrics: &result.metrics,
        raw_transcript: &result.text,
        normalized: &normalized,
        expected: &expected,
    });

    assert_text_eq(
        &format!(
            "{} {} normalized transcript",
            provider_label(&provider),
            fast_vad_mode.label()
        ),
        &expected,
        &normalized,
    );

    Ok(())
}

fn preprocess_for_case(
    config: &Config,
    mode: FastVadMode,
    fixture: &FixtureAudio,
) -> Result<PreprocessedAudio> {
    match mode {
        FastVadMode::Disabled => Ok(PreprocessedAudio {
            audio: fixture.samples.clone(),
            fast_vad_duration: None,
            fast_vad_segments: None,
            fast_vad_dropped_samples: None,
        }),
        FastVadMode::Enabled => {
            let mut fast_vad_config = config.fast_vad.clone();
            fast_vad_config.enabled = true;
            let mut vad = FastVad::maybe_new(&fast_vad_config, fixture.sample_rate_hz)
                .context("initialize Earshot fast VAD")?
                .context("Earshot fast VAD should be enabled for this case")?;

            let start = Instant::now();
            let outcome = vad
                .trim(&fixture.samples)
                .context("trim fixture with fast VAD")?;
            let duration = start.elapsed();

            if outcome.trimmed_audio.is_empty() {
                bail!("fast VAD removed the entire standard script fixture");
            }

            Ok(PreprocessedAudio {
                audio: outcome.trimmed_audio,
                fast_vad_duration: Some(duration),
                fast_vad_segments: Some(outcome.segments),
                fast_vad_dropped_samples: Some(outcome.dropped_samples),
            })
        }
    }
}

fn normalize_without_overrides(text: &str) -> String {
    NormalizeTextService::new(HashMap::new()).normalize(text)
}

fn load_standard_recording() -> Result<FixtureAudio> {
    load_pcm_s16le_mono_wav(&fixture_path("standard-script-recording.wav")?)
}

fn read_fixture_text(name: &str) -> Result<String> {
    fs::read_to_string(fixture_path(name)?).with_context(|| format!("read fixture {name}"))
}

fn fixture_path(name: &str) -> Result<PathBuf> {
    Ok(Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join(name))
}

fn load_pcm_s16le_mono_wav(path: &Path) -> Result<FixtureAudio> {
    let bytes = fs::read(path).with_context(|| format!("read WAV fixture {}", path.display()))?;
    if bytes.len() < 44 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        bail!("{} is not a RIFF/WAVE file", path.display());
    }

    let mut cursor = 12usize;
    let mut fmt: Option<WavFmt> = None;
    let mut data: Option<&[u8]> = None;

    while cursor + 8 <= bytes.len() {
        let id = &bytes[cursor..cursor + 4];
        let len = u32::from_le_bytes(bytes[cursor + 4..cursor + 8].try_into().unwrap()) as usize;
        let start = cursor + 8;
        let end = start.saturating_add(len);
        if end > bytes.len() {
            bail!("{} contains a truncated WAV chunk", path.display());
        }

        match id {
            b"fmt " => {
                if len < 16 {
                    bail!("{} contains a short fmt chunk", path.display());
                }
                fmt = Some(WavFmt {
                    audio_format: u16::from_le_bytes(bytes[start..start + 2].try_into().unwrap()),
                    channels: u16::from_le_bytes(bytes[start + 2..start + 4].try_into().unwrap()),
                    sample_rate_hz: u32::from_le_bytes(
                        bytes[start + 4..start + 8].try_into().unwrap(),
                    ),
                    bits_per_sample: u16::from_le_bytes(
                        bytes[start + 14..start + 16].try_into().unwrap(),
                    ),
                });
            }
            b"data" => data = Some(&bytes[start..end]),
            _ => {}
        }

        cursor = end + (len % 2);
    }

    let fmt = fmt.context("WAV fixture missing fmt chunk")?;
    let data = data.context("WAV fixture missing data chunk")?;
    if fmt.audio_format != 1
        || fmt.channels != 1
        || fmt.sample_rate_hz != SAMPLE_RATE_HZ
        || fmt.bits_per_sample != 16
    {
        bail!(
            "standard script WAV must be PCM s16le mono 16 kHz; got format={}, channels={}, sample_rate={}, bits={}",
            fmt.audio_format,
            fmt.channels,
            fmt.sample_rate_hz,
            fmt.bits_per_sample
        );
    }
    if data.len() % 2 != 0 {
        bail!("WAV data chunk has an odd byte length");
    }

    let samples = data
        .chunks_exact(2)
        .map(|chunk| i16::from_le_bytes([chunk[0], chunk[1]]) as f32 / i16::MAX as f32)
        .collect::<Vec<_>>();

    Ok(FixtureAudio {
        samples,
        sample_rate_hz: fmt.sample_rate_hz,
    })
}

fn assert_text_eq(label: &str, expected: &str, actual: &str) {
    if expected == actual {
        return;
    }

    panic!(
        "{label} mismatch\n\n--- expected fixture ---\n{expected}\n\n--- actual output ---\n{actual}\n\n--- line diff ---\n{}",
        line_diff(expected, actual)
    );
}

fn line_diff(expected: &str, actual: &str) -> String {
    let expected_lines = expected.lines().collect::<Vec<_>>();
    let actual_lines = actual.lines().collect::<Vec<_>>();
    let max = expected_lines.len().max(actual_lines.len());
    let mut out = String::new();

    for idx in 0..max {
        let line_no = idx + 1;
        match (expected_lines.get(idx), actual_lines.get(idx)) {
            (Some(expected), Some(actual)) if expected == actual => {}
            (Some(expected), Some(actual)) => {
                out.push_str(&format!("line {line_no}\n- {expected}\n+ {actual}\n"));
            }
            (Some(expected), None) => {
                out.push_str(&format!("line {line_no}\n- {expected}\n+ <missing>\n"))
            }
            (None, Some(actual)) => {
                out.push_str(&format!("line {line_no}\n- <missing>\n+ {actual}\n"))
            }
            (None, None) => {}
        }
    }

    out
}

fn print_case_report(report: CaseReport<'_>) {
    println!();
    println!("=== TIP live transcription benchmark ===");
    println!("provider: {}", report.provider);
    println!("fast_vad: {}", report.fast_vad_mode.label());
    println!("original_samples: {}", report.original_samples);
    println!("audio_sent_samples: {}", report.audio_sent_samples);
    println!(
        "audio_sent_duration_s: {:.3}",
        report.audio_sent_samples as f64 / SAMPLE_RATE_HZ as f64
    );
    if let Some(duration) = report.fast_vad_duration {
        println!("fast_vad_ms: {:.3}", duration_ms(duration));
    }
    if let Some(segments) = report.fast_vad_segments {
        println!("fast_vad_segments: {segments}");
    }
    if let Some(dropped) = report.fast_vad_dropped_samples {
        println!("fast_vad_dropped_samples: {dropped}");
    }
    println!(
        "backend_wall_ms: {:.3}",
        duration_ms(report.backend_wall_duration)
    );
    print_backend_metrics(report.backend_metrics);
    println!("--- raw transcript ---");
    println!("{}", report.raw_transcript);
    println!("--- normalized transcript ---");
    println!("{}", report.normalized);
    println!("--- expected transform fixture ---");
    println!("{}", report.expected);
    println!("=== end TIP benchmark ===");
}

fn print_backend_metrics(metrics: &BackendMetrics) {
    if let Some(duration) = metrics.encode_duration {
        println!("backend_encode_ms: {:.3}", duration_ms(duration));
    }
    if let Some(bytes) = metrics.encoded_bytes {
        println!("backend_encoded_bytes: {bytes}");
    }
    if let Some(duration) = metrics.upload_duration {
        println!("backend_upload_ms: {:.3}", duration_ms(duration));
    }
    if let Some(duration) = metrics.response_duration {
        println!("backend_response_parse_ms: {:.3}", duration_ms(duration));
    }
    println!(
        "backend_transcription_ms: {:.3}",
        duration_ms(metrics.transcription_duration)
    );
}

fn duration_ms(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1000.0
}

fn provider_label(provider: &TranscriptionProvider) -> &'static str {
    match provider {
        TranscriptionProvider::WhisperCpp => "whisper_cpp",
        TranscriptionProvider::Groq => "groq",
        TranscriptionProvider::Gemini => "gemini",
        TranscriptionProvider::Parakeet => "parakeet",
        TranscriptionProvider::Custom(_) => "custom",
    }
}

#[derive(Debug, Clone, Copy)]
enum FastVadMode {
    Disabled,
    Enabled,
}

impl FastVadMode {
    fn label(self) -> &'static str {
        match self {
            Self::Disabled => "without_fast_vad",
            Self::Enabled => "with_fast_vad",
        }
    }
}

#[derive(Debug)]
struct FixtureAudio {
    samples: Vec<f32>,
    sample_rate_hz: u32,
}

#[derive(Debug)]
struct PreprocessedAudio {
    audio: Vec<f32>,
    fast_vad_duration: Option<Duration>,
    fast_vad_segments: Option<usize>,
    fast_vad_dropped_samples: Option<usize>,
}

#[derive(Debug)]
struct WavFmt {
    audio_format: u16,
    channels: u16,
    sample_rate_hz: u32,
    bits_per_sample: u16,
}

struct CaseReport<'a> {
    provider: &'static str,
    fast_vad_mode: FastVadMode,
    original_samples: usize,
    audio_sent_samples: usize,
    fast_vad_duration: Option<Duration>,
    fast_vad_segments: Option<usize>,
    fast_vad_dropped_samples: Option<usize>,
    backend_wall_duration: Duration,
    backend_metrics: &'a BackendMetrics,
    raw_transcript: &'a str,
    normalized: &'a str,
    expected: &'a str,
}
