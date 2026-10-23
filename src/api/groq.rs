use crate::api::types::{ChatMessage, ChatRequest, ChatResponse, ResponseFormat};
use crate::config::ApiConfig;
use anyhow::{bail, Result};
use base64::{engine::general_purpose::STANDARD, Engine as _};
use futures_util::StreamExt;
use reqwest::Client;
use tokio::sync::mpsc;

const GROQ_API_URL: &str = "https://api.groq.com/openai/v1/chat/completions";

pub struct GroqClient {
    client: Client,
    api_keys: Vec<String>,
    model: String,
    max_tokens: u32,
}

#[derive(Debug, Clone)]
pub struct TranscriptionResult {
    pub text: String,
    pub confidence: Option<f32>,
}

impl GroqClient {
    pub fn new(config: &ApiConfig) -> Self {
        Self {
            client: Client::builder()
                .timeout(std::time::Duration::from_secs(30))
                .build()
                .expect("valid HTTP client configuration"),
            api_keys: if config.api_keys.is_empty() {
                vec![config.api_key.clone()]
            } else {
                config.api_keys.clone()
            },
            model: config.model.clone(),
            max_tokens: config.max_tokens,
        }
    }

    async fn send_with_api_keys(
        &self,
        mut build_request: impl FnMut(&str) -> reqwest::RequestBuilder,
        failure_label: &str,
    ) -> Result<reqwest::Response> {
        let mut last_error = None;
        for key in &self.api_keys {
            if key.trim().is_empty() {
                continue;
            }
            match self
                .send_request(|| build_request(key), failure_label)
                .await
            {
                Ok(response) => return Ok(response),
                Err(error) => last_error = Some(error),
            }
        }
        Err(last_error.unwrap_or_else(|| anyhow::anyhow!("No Groq API keys configured")))
    }

    async fn send_request(
        &self,
        build_request: impl FnOnce() -> reqwest::RequestBuilder,
        failure_label: &str,
    ) -> Result<reqwest::Response> {
        let response = build_request().send().await?;
        if response.status().is_success() {
            return Ok(response);
        }

        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        Err(anyhow::anyhow!(
            "{failure_label} failed with status {status}: {body}"
        ))
    }

    pub async fn transcribe(
        &self,
        wav: &[u8],
        model: &str,
        language: &str,
    ) -> Result<TranscriptionResult> {
        let response = self
            .send_with_api_keys(
                |key| {
                    let file =
                        reqwest::multipart::Part::bytes(wav.to_vec()).file_name("meeting.wav");
                    let mut form = reqwest::multipart::Form::new()
                        .text("model", model.to_string())
                        .text("response_format", "verbose_json")
                        .part("file", file);
                    if !language.trim().is_empty() {
                        form = form.text("language", language.trim().to_string());
                    }
                    self.client
                        .post("https://api.groq.com/openai/v1/audio/transcriptions")
                        .bearer_auth(key)
                        .multipart(form)
                },
                "Transcription request",
            )
            .await?;
        let body = response.text().await?;
        let parsed: serde_json::Value = serde_json::from_str(&body)?;
        let text = parsed
            .get("text")
            .and_then(|text| text.as_str())
            .unwrap_or_default()
            .to_string();

        // Groq exposes segment avg_logprob for verbose transcription
        // responses. Convert the mean log probability to a bounded, usable
        // confidence score while remaining compatible with responses that do
        // not include segment metadata.
        let confidence = parsed
            .get("confidence")
            .and_then(|value| value.as_f64())
            .map(|value| value as f32)
            .or_else(|| {
                let segments = parsed.get("segments")?.as_array()?;
                let scores: Vec<f64> = segments
                    .iter()
                    .filter_map(|segment| segment.get("avg_logprob")?.as_f64())
                    .collect();
                if scores.is_empty() {
                    None
                } else {
                    let average = scores.iter().sum::<f64>() / scores.len() as f64;
                    Some(average.exp().clamp(0.0, 1.0) as f32)
                }
            });

        Ok(TranscriptionResult { text, confidence })
    }

    pub async fn analyze_image(
        &self,
        model: &str,
        prompt: &str,
        image: &[u8],
        mime_type: &str,
    ) -> Result<String> {
        const MAX_IMAGE_BYTES: usize = 20 * 1024 * 1024;
        if image.len() > MAX_IMAGE_BYTES {
            bail!("Screenshot exceeds Groq's 20 MB image limit");
        }

        #[derive(serde::Serialize)]
        struct ImageRequest {
            model: String,
            messages: Vec<ImageMessage>,
            stream: bool,
            max_completion_tokens: u32,
        }
        #[derive(serde::Serialize)]
        struct ImageMessage {
            role: String,
            content: Vec<ImageContent>,
        }
        #[derive(serde::Serialize)]
        #[serde(untagged)]
        enum ImageContent {
            Text {
                #[serde(rename = "type")]
                kind: String,
                text: String,
            },
            ImageUrl {
                #[serde(rename = "type")]
                kind: String,
                image_url: ImageUrlValue,
            },
        }
        #[derive(serde::Serialize)]
        struct ImageUrlValue {
            url: String,
        }

        let encoded_image = STANDARD.encode(image);
        let request = ImageRequest {
            model: model.to_string(),
            messages: vec![ImageMessage {
                role: "user".to_string(),
                content: vec![
                    ImageContent::Text {
                        kind: "text".to_string(),
                        text: prompt.to_string(),
                    },
                    ImageContent::ImageUrl {
                        kind: "image_url".to_string(),
                        image_url: ImageUrlValue {
                            url: format!("data:{mime_type};base64,{encoded_image}"),
                        },
                    },
                ],
            }],
            stream: false,
            max_completion_tokens: self.max_tokens,
        };

        let mut last_error = None;
        for key in &self.api_keys {
            if key.trim().is_empty() {
                continue;
            }
            match self
                .send_request(
                    || {
                        self.client
                            .post(GROQ_API_URL)
                            .bearer_auth(key)
                            .json(&request)
                    },
                    "Groq vision request",
                )
                .await
            {
                Ok(response) => {
                    let body: serde_json::Value = response.json().await?;
                    if let Some(answer) = body
                        .pointer("/choices/0/message/content")
                        .and_then(serde_json::Value::as_str)
                    {
                        return Ok(answer.to_string());
                    }
                    last_error = Some(anyhow::anyhow!("Groq returned an empty vision response"));
                }
                Err(error) => last_error = Some(error),
            }
        }
        Err(last_error.unwrap_or_else(|| anyhow::anyhow!("No Groq API keys configured")))
    }

    pub async fn stream_chat(
        &self,
        messages: Vec<ChatMessage>,
        sender: mpsc::UnboundedSender<String>,
    ) -> Result<()> {
        let req_body = ChatRequest {
            model: self.model.clone(),
            messages,
            stream: true,
            max_tokens: Some(self.max_tokens),
            response_format: Some(ResponseFormat {
                format_type: "json_object".to_string(),
            }),
        };

        let response = self
            .send_with_api_keys(
                |key| {
                    self.client
                        .post(GROQ_API_URL)
                        .bearer_auth(key)
                        .json(&req_body)
                },
                "API request",
            )
            .await?;

        let mut stream = response.bytes_stream();
        let mut pending = Vec::new();

        while let Some(chunk) = stream.next().await {
            pending.extend_from_slice(&chunk?);
            while let Some(newline) = pending.iter().position(|byte| *byte == b'\n') {
                let mut line: Vec<_> = pending.drain(..=newline).collect();
                line.pop();
                if line.last() == Some(&b'\r') {
                    line.pop();
                }
                if let Some(content) = parse_sse_line(&line) {
                    if sender.send(content).is_err() {
                        return Ok(());
                    }
                }
            }
        }
        if !pending.is_empty() {
            if let Some(content) = parse_sse_line(&pending) {
                let _ = sender.send(content);
            }
        }

        Ok(())
    }
}

fn parse_sse_line(line: &[u8]) -> Option<String> {
    let line = std::str::from_utf8(line).ok()?;
    let data = line.strip_prefix("data:")?.trim_start();
    if data == "[DONE]" {
        return None;
    }
    serde_json::from_str::<ChatResponse>(data)
        .ok()?
        .choices
        .into_iter()
        .next()?
        .delta?
        .content
}

#[cfg(test)]
mod tests {
    use super::{parse_sse_line, GroqClient};
    use crate::config::ApiConfig;

    #[test]
    fn parses_content_from_an_sse_data_line() {
        assert_eq!(
            parse_sse_line(br#"data: {"choices":[{"delta":{"content":"Hello"}}]}"#),
            Some("Hello".to_string())
        );
        assert_eq!(parse_sse_line(b"data: [DONE]"), None);
        assert_eq!(parse_sse_line(b"event: ping"), None);
    }

    #[tokio::test]
    async fn skips_empty_api_keys_without_sending_a_request() {
        let client = GroqClient::new(&ApiConfig::default());
        let error = client
            .send_with_api_keys(
                |_| panic!("empty keys must not send requests"),
                "API request",
            )
            .await
            .expect_err("missing keys should return an error");

        assert_eq!(error.to_string(), "No Groq API keys configured");
    }
}
