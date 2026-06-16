use crate::bench_report::{TipBenchmarkInput, print_case_report};
use crate::diff_report::{assert_text_eq, print_text_diff_report};
use crate::resource_timeline::TipResourceTimeline;
use anyhow::{Context, Result, bail};
use hyprwhspr_rs::audio::FastVad;
use hyprwhspr_rs::config::{Config, ConfigManager, TranscriptionProvider};
use hyprwhspr_rs::text::NormalizeTextService;
use hyprwhspr_rs::transcription::TranscriptionBackend;
use hyprwhspr_rs::whisper::WhisperVadOptions;
use std::collections::HashMap;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

pub(crate) const SAMPLE_RATE_HZ: u32 = 16_000;
const PROVIDERS_ENV: &str = "HYPRWHSPR_TIP_PROVIDERS";
const FAST_VAD_ENV: &str = "HYPRWHSPR_TIP_FAST_VAD";

const PROVIDERS: &[ProviderSpec] = &[
    ProviderSpec {
        id: "groq",
        label: "Groq",
        provider: groq_provider,
    },
    ProviderSpec {
        id: "whisper_cpp",
        label: "whisper.cpp",
        provider: whisper_cpp_provider,
    },
];

pub(crate) fn run_normalization_golden_assertion() -> Result<()> {
    let input = read_fixture_text("golden-script.txt")?;
    let expected = read_fixture_text("golden-script-expected-transform.txt")?;
    let actual = normalize_without_overrides(&input);

    assert_text_eq("NormalizeTextService golden script", &expected, &actual);
    Ok(())
}

pub(crate) async fn run_selected_live_cases(correctness_mode: CorrectnessMode) -> Result<()> {
    let providers = selected_providers()?;
    let modes = selected_fast_vad_modes()?;

    for provider in providers {
        for mode in &modes {
            run_live_case(provider, *mode, correctness_mode).await?;
        }
    }

    Ok(())
}

pub(crate) async fn run_provider_case(
    provider_id: &str,
    fast_vad_mode: FastVadMode,
    correctness_mode: CorrectnessMode,
) -> Result<()> {
    run_live_case(
        provider_by_id(provider_id)?,
        fast_vad_mode,
        correctness_mode,
    )
    .await
}

async fn run_live_case(
    provider: &'static ProviderSpec,
    fast_vad_mode: FastVadMode,
    correctness_mode: CorrectnessMode,
) -> Result<()> {
    let mut timeline = TipResourceTimeline::new();
    let fixture = timeline.measure("fixture.load_wav", None, None, load_standard_recording)?;
    timeline.set_latest_bytes_out(
        "fixture.load_wav",
        fixture.samples.len() * std::mem::size_of::<f32>(),
    );
    let expected = timeline.measure("fixture.load_expected", None, None, || {
        read_fixture_text("golden-script-expected-transform.txt")
    })?;
    timeline.set_latest_bytes_out("fixture.load_expected", expected.len());
    let config_manager = timeline.measure("config.load", None, None, || {
        ConfigManager::load().context("load real hyprwhspr-rs config")
    })?;
    let mut config = config_manager.get();
    config.transcription.provider = (provider.provider)();
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
    let preprocessed = timeline.measure(
        "preprocess.fast_vad",
        Some(fixture.samples.len() * std::mem::size_of::<f32>()),
        None,
        || preprocess_for_case(&config, fast_vad_mode, &fixture),
    )?;
    timeline.set_latest_bytes_out(
        "preprocess.fast_vad",
        preprocessed.audio.len() * std::mem::size_of::<f32>(),
    );
    let preprocess_duration = timeline
        .samples()
        .iter()
        .rev()
        .find(|sample| sample.name == "preprocess.fast_vad")
        .map(|sample| sample.wall_duration)
        .unwrap_or_default();

    let vad_options = WhisperVadOptions::disabled();
    let backend = timeline.measure("backend.init", None, None, || {
        let backend = TranscriptionBackend::build(&config_manager, &config, vad_options)
            .with_context(|| format!("build {} backend", provider.id))?;
        backend
            .initialize()
            .with_context(|| format!("initialize {} backend", provider.id))?;
        Ok(backend)
    })?;
    let provider_init_duration = timeline
        .samples()
        .iter()
        .rev()
        .find(|sample| sample.name == "backend.init")
        .map(|sample| sample.wall_duration)
        .unwrap_or_default();

    let result = timeline
        .measure_async(
            "backend.transcribe",
            Some(preprocessed.audio.len() * std::mem::size_of::<f32>()),
            None,
            || async {
                backend
                    .transcribe(preprocessed.audio.clone())
                    .await
                    .with_context(|| format!("transcribe with {}", provider.id))
            },
        )
        .await?;
    timeline.set_latest_bytes_out("backend.transcribe", result.text.len());
    let wall_duration = timeline
        .samples()
        .iter()
        .rev()
        .find(|sample| sample.name == "backend.transcribe")
        .map(|sample| sample.wall_duration)
        .unwrap_or_default();

    let normalized = timeline.measure("normalize.total", Some(result.text.len()), None, || {
        Ok(normalize_without_overrides(&result.text))
    })?;
    timeline.set_latest_bytes_out("normalize.total", normalized.len());
    let normalize_duration = timeline
        .samples()
        .iter()
        .rev()
        .find(|sample| sample.name == "normalize.total")
        .map(|sample| sample.wall_duration)
        .unwrap_or_default();
    let total_duration = timeline.elapsed();
    let resource_delta = timeline.total_delta();

    print_case_report(TipBenchmarkInput {
        provider_label: provider.label,
        provider_id: provider.id,
        fast_vad_mode,
        original_samples: fixture.samples.len(),
        sample_rate_hz: fixture.sample_rate_hz,
        preprocess_duration,
        provider_init_duration,
        audio_sent_samples: preprocessed.audio.len(),
        fast_vad_duration: preprocessed.fast_vad_duration,
        fast_vad_segments: preprocessed.fast_vad_segments,
        fast_vad_dropped_samples: preprocessed.fast_vad_dropped_samples,
        backend_wall_duration: wall_duration,
        backend_metrics: &result.metrics,
        normalize_duration,
        total_duration,
        resource_delta,
        timeline_samples: timeline.samples(),
        raw_transcript: &result.text,
        normalized: &normalized,
        expected: &expected,
    });

    let label = format!(
        "{} {} normalized transcript",
        provider.id,
        fast_vad_mode.label()
    );
    match correctness_mode {
        CorrectnessMode::Assert => assert_text_eq(&label, &expected, &normalized),
        CorrectnessMode::ReportOnly => print_text_diff_report(&label, &expected, &normalized),
    }

    Ok(())
}

fn selected_providers() -> Result<Vec<&'static ProviderSpec>> {
    let Some(raw) = env::var(PROVIDERS_ENV)
        .ok()
        .filter(|value| !value.trim().is_empty())
    else {
        return Ok(PROVIDERS.iter().collect());
    };

    if raw.trim().eq_ignore_ascii_case("all") {
        return Ok(PROVIDERS.iter().collect());
    }

    raw.split(',')
        .map(|part| provider_by_id(part.trim()))
        .collect::<Result<Vec<_>>>()
}

fn selected_fast_vad_modes() -> Result<Vec<FastVadMode>> {
    let Some(raw) = env::var(FAST_VAD_ENV)
        .ok()
        .filter(|value| !value.trim().is_empty())
    else {
        return Ok(vec![FastVadMode::Disabled, FastVadMode::Enabled]);
    };

    match raw.trim().to_ascii_lowercase().as_str() {
        "all" | "both" => Ok(vec![FastVadMode::Disabled, FastVadMode::Enabled]),
        "on" | "yes" | "true" | "enabled" | "with" | "with_fast_vad" => {
            Ok(vec![FastVadMode::Enabled])
        }
        "off" | "no" | "false" | "disabled" | "without" | "without_fast_vad" => {
            Ok(vec![FastVadMode::Disabled])
        }
        other => bail!("unknown {FAST_VAD_ENV} value '{other}'; use both, enabled, or disabled"),
    }
}

fn provider_by_id(id: &str) -> Result<&'static ProviderSpec> {
    PROVIDERS
        .iter()
        .find(|provider| provider.id == id)
        .with_context(|| {
            format!(
                "unknown provider '{id}'; known providers: {}",
                provider_ids()
            )
        })
}

fn provider_ids() -> String {
    PROVIDERS
        .iter()
        .map(|provider| provider.id)
        .collect::<Vec<_>>()
        .join(", ")
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

fn groq_provider() -> TranscriptionProvider {
    TranscriptionProvider::Groq
}

fn whisper_cpp_provider() -> TranscriptionProvider {
    TranscriptionProvider::WhisperCpp
}

#[derive(Debug, Clone, Copy)]
struct ProviderSpec {
    id: &'static str,
    label: &'static str,
    provider: fn() -> TranscriptionProvider,
}

#[derive(Debug, Clone, Copy)]
pub(crate) enum FastVadMode {
    Disabled,
    Enabled,
}

impl FastVadMode {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Disabled => "without_fast_vad",
            Self::Enabled => "with_fast_vad",
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub(crate) enum CorrectnessMode {
    Assert,
    ReportOnly,
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
