use crate::config::CustomProviderConfig;
use crate::transcription::audio::{encode_to_flac, encode_to_wav, EncodedAudio};
use crate::transcription::postprocess::clean_transcription;
use crate::transcription::{BackendMetrics, TranscriptionResult};
use anyhow::{Context, Result};
use reqwest::{header, multipart, Client, Url};
use serde::Deserialize;
use std::cmp;
use std::time::{Duration, Instant};
use tokio::time::sleep;
use tracing::{info, warn};

#[derive(Clone)]
pub struct CustomOpenAiTranscriber {
    name: String,
    label: String,
    client: Client,
    endpoint: Url,
    api_key: Option<String>,
    model: String,
    audio_format: AudioFormat,
    headers: Vec<(String, String)>,
    body: Vec<(String, String)>,
    prompt: String,
    request_timeout: Duration,
    max_retries: u32,
}

impl CustomOpenAiTranscriber {
    pub fn new(
        name: &str,
        config: &CustomProviderConfig,
        request_timeout: Duration,
        max_retries: u32,
        prompt: String,
    ) -> Result<Self> {
        let endpoint = if is_absolute_endpoint(&config.endpoint) {
            resolve_endpoint(None, &config.endpoint)?
        } else {
            let base_url = config.base_url.resolve("base_url")?;
            resolve_endpoint(Some(&base_url), &config.endpoint)?
        };
        let api_key = config.api_key.resolve("api_key")?;

        let client = Client::builder()
            .user_agent(format!("hyprwhspr-rs (custom:{name})"))
            .connect_timeout(Duration::from_secs(10))
            .timeout(request_timeout)
            .pool_idle_timeout(Duration::from_secs(30))
            .build()
            .context("Failed to build custom OpenAI-compatible HTTP client")?;

        Ok(Self {
            name: name.to_string(),
            label: config
                .label
                .clone()
                .unwrap_or_else(|| format!("Custom ({name})")),
            client,
            endpoint,
            api_key,
            model: config.model.clone(),
            audio_format: AudioFormat::from_config(&config.audio_format)?,
            headers: config
                .headers
                .iter()
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect(),
            body: config
                .body
                .iter()
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect(),
            prompt,
            request_timeout,
            max_retries,
        })
    }

    pub fn initialize(&self) -> Result<()> {
        if self.model.trim().is_empty() {
            anyhow::bail!(
                "model is required for custom transcription provider '{}'",
                self.name
            );
        }

        info!(
            "✅ {} transcription ready (model: {}, endpoint: {}, timeout: {:?})",
            self.label, self.model, self.endpoint, self.request_timeout
        );
        Ok(())
    }

    pub fn provider_name(&self) -> &str {
        &self.label
    }

    pub async fn transcribe(&self, audio_data: Vec<f32>) -> Result<TranscriptionResult> {
        if audio_data.is_empty() {
            return Ok(TranscriptionResult {
                text: String::new(),
                metrics: BackendMetrics::default(),
            });
        }

        let duration_secs = audio_data.len() as f32 / 16000.0;
        info!(
            provider = self.provider_name(),
            "🧠 Transcribing {:.2}s of audio via custom OpenAI-compatible provider", duration_secs
        );

        let encode_start = Instant::now();
        let encoded = match self.audio_format {
            AudioFormat::Wav => encode_to_wav(&audio_data).await?,
            AudioFormat::Flac => encode_to_flac(&audio_data).await?,
        };
        let encode_duration = encode_start.elapsed();
        let encoded_len = encoded.data.len();

        let transcribe_start = Instant::now();
        let (raw, timings) = self.send_with_retry(&encoded).await?;
        let transcription_duration = transcribe_start.elapsed();
        let cleaned = clean_transcription(&raw, &self.prompt);

        if cleaned.is_empty() {
            warn!("{} returned empty or non-speech transcription", self.label);
        } else {
            info!("✅ Transcription ({}): {}", self.label, cleaned);
        }

        let metrics = BackendMetrics {
            encode_duration: Some(encode_duration),
            encoded_bytes: Some(encoded_len),
            upload_duration: Some(timings.upload),
            response_duration: Some(timings.response),
            transcription_duration,
        };

        Ok(TranscriptionResult {
            text: cleaned,
            metrics,
        })
    }

    async fn send_with_retry(&self, audio: &EncodedAudio) -> Result<(String, NetworkTimings)> {
        let attempts = cmp::max(1, self.max_retries.saturating_add(1));

        for attempt in 0..attempts {
            match self.send_once(audio).await {
                Ok(result) => return Ok(result),
                Err(err) => {
                    if attempt + 1 == attempts {
                        return Err(err);
                    }

                    warn!(
                        attempt = attempt + 1,
                        max_attempts = attempts,
                        "Custom transcription attempt failed: {}",
                        err
                    );

                    let backoff = Duration::from_millis(500 * (1 << attempt));
                    sleep(backoff).await;
                }
            }
        }

        Err(anyhow::anyhow!("Unknown custom transcription failure"))
    }

    async fn send_once(&self, audio: &EncodedAudio) -> Result<(String, NetworkTimings)> {
        let mut form = multipart::Form::new()
            .text("model", self.model.clone())
            .text("response_format", "json".to_string());

        for (key, value) in &self.body {
            form = form.text(key.clone(), value.clone());
        }

        if !self.prompt.trim().is_empty() && !self.body.iter().any(|(key, _)| key == "prompt") {
            form = form.text("prompt", self.prompt.clone());
        }

        let file_part = multipart::Part::stream(audio.data.clone())
            .file_name(self.audio_format.file_name())
            .mime_str(audio.content_type)
            .context("Failed to set custom provider audio content type")?;

        form = form.part("file", file_part);

        let mut request = self.client.post(self.endpoint.clone()).multipart(form);
        let mut has_authorization = false;

        for (key, value) in &self.headers {
            if key.eq_ignore_ascii_case(header::AUTHORIZATION.as_str()) {
                has_authorization = true;
            }
            request = request.header(key, value);
        }

        if let Some(api_key) = &self.api_key {
            if !has_authorization {
                request = request.bearer_auth(api_key);
            }
        }

        let request_start = Instant::now();
        let response = request
            .send()
            .await
            .context("Failed to send custom transcription request")?;

        let upload_duration = request_start.elapsed();

        if response.status().is_success() {
            let parse_start = Instant::now();
            let payload: OpenAiTranscriptionResponse = response
                .json()
                .await
                .context("Failed to deserialize custom transcription response")?;
            let response_duration = parse_start.elapsed();
            return Ok((
                payload.text.unwrap_or_default(),
                NetworkTimings {
                    upload: upload_duration,
                    response: response_duration,
                },
            ));
        }

        let status = response.status();
        let body = response
            .json::<OpenAiErrorResponse>()
            .await
            .unwrap_or_default();

        let message = body
            .error
            .and_then(|err| err.message)
            .unwrap_or_else(|| format!("Custom transcription failed with status {status}"));

        Err(anyhow::anyhow!(message).context(format!("Custom request failed ({status})")))
    }
}

#[derive(Clone, Copy)]
enum AudioFormat {
    Wav,
    Flac,
}

impl AudioFormat {
    fn from_config(value: &str) -> Result<Self> {
        match value.trim() {
            "" | "wav" => Ok(Self::Wav),
            "flac" => Ok(Self::Flac),
            other => anyhow::bail!(
                "unsupported custom provider audio_format '{other}' (supported: wav, flac)"
            ),
        }
    }

    fn file_name(self) -> &'static str {
        match self {
            Self::Wav => "audio.wav",
            Self::Flac => "audio.flac",
        }
    }
}

fn is_absolute_endpoint(endpoint: &str) -> bool {
    endpoint.starts_with("http://") || endpoint.starts_with("https://")
}

fn resolve_endpoint(base_url: Option<&str>, endpoint: &str) -> Result<Url> {
    if is_absolute_endpoint(endpoint) {
        return Url::parse(endpoint)
            .with_context(|| format!("Invalid custom endpoint: {endpoint}"));
    }

    let base_url = base_url.ok_or_else(|| anyhow::anyhow!("base_url is required"))?;
    let mut base = Url::parse(base_url)
        .with_context(|| format!("Invalid custom provider base_url: {base_url}"))?;
    if !base.path().ends_with('/') {
        let path = format!("{}/", base.path());
        base.set_path(&path);
    }

    base.join(endpoint.trim_start_matches('/'))
        .with_context(|| format!("Invalid custom provider endpoint: {endpoint}"))
}

#[derive(Debug, Clone, Copy)]
struct NetworkTimings {
    upload: Duration,
    response: Duration,
}

#[derive(Debug, Deserialize, Default)]
struct OpenAiTranscriptionResponse {
    text: Option<String>,
}

#[derive(Debug, Deserialize, Default)]
struct OpenAiErrorResponse {
    error: Option<OpenAiErrorDetail>,
}

#[derive(Debug, Deserialize, Default)]
struct OpenAiErrorDetail {
    message: Option<String>,
}
