use crate::api::types::{ChatMessage, ChatRequest, ChatResponse, ResponseFormat};
use crate::config::ApiConfig;
use anyhow::{bail, Result};
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

    pub async fn transcribe(
        &self,
        path: &str,
        model: &str,
        language: &str,
    ) -> Result<TranscriptionResult> {
        let mut last_error = None;
        let mut response = None;
        for key in &self.api_keys {
            let file = reqwest::multipart::Part::file(path)
                .await?
                .file_name("meeting.wav");
            let mut form = reqwest::multipart::Form::new()
                .text("model", model.to_string())
                .text("response_format", "verbose_json")
                .part("file", file);
            if !language.trim().is_empty() {
                form = form.text("language", language.trim().to_string());
            }
            match self
                .client
                .post("https://api.groq.com/openai/v1/audio/transcriptions")
                .bearer_auth(key)
                .multipart(form)
                .send()
                .await
            {
                Ok(result) if result.status().is_success() => {
                    response = Some(result);
                    break;
                }
                Ok(result) => {
                    let status = result.status();
                    let body = result.text().await.unwrap_or_default();
                    last_error = Some(anyhow::anyhow!(
                        "Transcription request failed with status {}: {}",
                        status,
                        body
                    ));
                }
                Err(error) => last_error = Some(error.into()),
            }
        }
        let response = match response {
            Some(response) => response,
            None => {
                return Err(
                    last_error.unwrap_or_else(|| anyhow::anyhow!("No Groq API keys configured"))
                )
            }
        };
        let status = response.status();
        let body = response.text().await?;
        if !status.is_success() {
            bail!(
                "Transcription request failed with status {}: {}",
                status,
                body
            );
        }
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

        let mut last_error = None;
        let mut response = None;
        for key in &self.api_keys {
            match self
                .client
                .post(GROQ_API_URL)
                .bearer_auth(key)
                .json(&req_body)
                .send()
                .await
            {
                Ok(result) if result.status().is_success() => {
                    response = Some(result);
                    break;
                }
                Ok(result) => {
                    let status = result.status();
                    let body = result.text().await.unwrap_or_default();
                    last_error = Some(anyhow::anyhow!(
                        "API request failed with status {}: {}",
                        status,
                        body
                    ));
                }
                Err(error) => last_error = Some(error.into()),
            }
        }
        let response = match response {
            Some(response) => response,
            None => {
                return Err(
                    last_error.unwrap_or_else(|| anyhow::anyhow!("No Groq API keys configured"))
                )
            }
        };

        let mut stream = response.bytes_stream();

        while let Some(chunk) = stream.next().await {
            let chunk = chunk?;
            let text = String::from_utf8_lossy(&chunk);

            for line in text.lines() {
                if line.starts_with("data: ") {
                    let data = &line[6..];
                    if data == "[DONE]" {
                        continue;
                    }

                    if let Ok(resp) = serde_json::from_str::<ChatResponse>(data) {
                        if let Some(choice) = resp.choices.first() {
                            if let Some(delta) = &choice.delta {
                                if let Some(content) = &delta.content {
                                    if sender.send(content.clone()).is_err() {
                                        return Ok(());
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }

        Ok(())
    }
}
