use crate::config::{CustomProviderConfig, SubscriptionAuthSource};
use crate::transcription::audio::{EncodedAudio, encode_to_flac, encode_to_wav};
use crate::transcription::postprocess::clean_transcription;
use crate::transcription::{BackendMetrics, TranscriptionResult};
use anyhow::{Context, Result};
use reqwest::{Client, Url, header, multipart};
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
    auth: CustomAuth,
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
        let auth = if config.subscription.is_configured() {
            config
                .subscription
                .resolve("subscription")?
                .ok_or_else(|| {
                    anyhow::anyhow!("subscription auth source did not resolve a token")
                })?;
            CustomAuth::Subscription(config.subscription.clone())
        } else {
            config
                .api_key
                .resolve("api_key")?
                .map(CustomAuth::ApiKey)
                .unwrap_or(CustomAuth::None)
        };

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
            auth,
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

        if !has_authorization {
            if let Some(auth_token) = self.resolve_auth_token()? {
                request = request.bearer_auth(auth_token);
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

    fn resolve_auth_token(&self) -> Result<Option<String>> {
        match &self.auth {
            CustomAuth::None => Ok(None),
            CustomAuth::ApiKey(api_key) => Ok(Some(api_key.clone())),
            CustomAuth::Subscription(source) => source
                .resolve("subscription")?
                .ok_or_else(|| anyhow::anyhow!("subscription auth source did not resolve a token"))
                .map(Some),
        }
    }
}

#[derive(Clone)]
enum CustomAuth {
    None,
    ApiKey(String),
    Subscription(SubscriptionAuthSource),
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{SecretSource, ValueSource};
    use bytes::Bytes;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::{TcpListener, TcpStream};
    use tokio::sync::mpsc;

    #[tokio::test]
    async fn subscription_auth_is_resolved_for_each_request() {
        let root = std::env::temp_dir().join(format!(
            "hyprwhspr-rs-custom-openai-auth-refresh-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&root).expect("create temp dir");
        let auth_path = root.join("auth.json");
        write_auth_token(&auth_path, "first-token");

        let (endpoint, mut auth_headers) = spawn_auth_capture_server().await;
        let config = CustomProviderConfig {
            base_url: ValueSource::default(),
            endpoint,
            model: "gpt-4o-mini-transcribe".to_string(),
            audio_format: "wav".to_string(),
            api_key: SecretSource::default(),
            subscription: SubscriptionAuthSource {
                file: Some(auth_path.to_string_lossy().into_owned()),
                ..Default::default()
            },
            ..Default::default()
        };
        let transcriber = CustomOpenAiTranscriber::new(
            "auth_refresh",
            &config,
            Duration::from_secs(5),
            0,
            String::new(),
        )
        .expect("transcriber");

        let audio = EncodedAudio {
            data: Bytes::from_static(b"not-a-real-wav"),
            content_type: "audio/wav",
        };

        transcriber.send_once(&audio).await.expect("first request");
        assert_eq!(
            auth_headers.recv().await.as_deref(),
            Some("Bearer first-token")
        );

        write_auth_token(&auth_path, "second-token");
        transcriber.send_once(&audio).await.expect("second request");
        assert_eq!(
            auth_headers.recv().await.as_deref(),
            Some("Bearer second-token")
        );

        let _ = std::fs::remove_dir_all(&root);
    }

    fn write_auth_token(path: &std::path::Path, token: &str) {
        std::fs::write(
            path,
            format!(
                r#"{{
                    "tokens": {{
                        "access_token": "{token}"
                    }}
                }}"#
            ),
        )
        .expect("write auth token");
    }

    async fn spawn_auth_capture_server() -> (String, mpsc::Receiver<String>) {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind test server");
        let endpoint = format!(
            "http://{}/v1/audio/transcriptions",
            listener.local_addr().expect("local addr")
        );
        let (tx, rx) = mpsc::channel(2);

        tokio::spawn(async move {
            for _ in 0..2 {
                let (stream, _) = listener.accept().await.expect("accept request");
                let tx = tx.clone();
                tokio::spawn(async move {
                    let auth = read_authorization_header(stream)
                        .await
                        .expect("read request");
                    tx.send(auth).await.expect("send auth header");
                });
            }
        });

        (endpoint, rx)
    }

    async fn read_authorization_header(mut stream: TcpStream) -> Result<String> {
        let mut buffer = Vec::new();
        let mut chunk = [0; 1024];
        let header_end = loop {
            let read = stream.read(&mut chunk).await?;
            if read == 0 {
                anyhow::bail!("request closed before headers");
            }
            buffer.extend_from_slice(&chunk[..read]);
            if let Some(position) = find_header_end(&buffer) {
                break position;
            }
        };

        let headers = String::from_utf8_lossy(&buffer[..header_end]).into_owned();
        let content_length = headers
            .lines()
            .filter_map(|line| line.split_once(':'))
            .find(|(name, _)| name.eq_ignore_ascii_case("content-length"))
            .and_then(|(_, value)| value.trim().parse::<usize>().ok())
            .unwrap_or(0);
        let body_start = header_end + 4;
        while buffer.len().saturating_sub(body_start) < content_length {
            let read = stream.read(&mut chunk).await?;
            if read == 0 {
                break;
            }
            buffer.extend_from_slice(&chunk[..read]);
        }

        let auth = headers
            .lines()
            .filter_map(|line| line.split_once(':'))
            .find(|(name, _)| name.eq_ignore_ascii_case("authorization"))
            .map(|(_, value)| value.trim().to_string())
            .context("missing authorization header")?;

        let body = br#"{"text":""}"#;
        let response = format!(
            "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
            body.len()
        );
        stream.write_all(response.as_bytes()).await?;
        stream.write_all(body).await?;
        stream.shutdown().await?;

        Ok(auth)
    }

    fn find_header_end(buffer: &[u8]) -> Option<usize> {
        buffer.windows(4).position(|window| window == b"\r\n\r\n")
    }
}
