use crate::api::types::{ChatMessage, ChatRequest, ChatResponse, ResponseFormat};
use crate::config::ApiConfig;
use anyhow::{bail, Result};
use futures_util::StreamExt;
use reqwest::Client;
use tokio::sync::mpsc;

const GROQ_API_URL: &str = "https://api.groq.com/openai/v1/chat/completions";

pub struct GroqClient {
    client: Client,
    api_key: String,
    model: String,
    max_tokens: u32,
}

impl GroqClient {
    pub fn new(config: &ApiConfig) -> Self {
        Self {
            client: Client::new(),
            api_key: config.api_key.clone(),
            model: config.model.clone(),
            max_tokens: config.max_tokens,
        }
    }

    pub async fn transcribe(&self, path: &str, model: &str, language: &str) -> Result<String> {
        let file = reqwest::multipart::Part::file(path)
            .await?
            .file_name("meeting.wav");
        let mut form = reqwest::multipart::Form::new()
            .text("model", model.to_string())
            .text("response_format", "json")
            .part("file", file);
        if !language.trim().is_empty() {
            form = form.text("language", language.trim().to_string());
        }
        let response = self
            .client
            .post("https://api.groq.com/openai/v1/audio/transcriptions")
            .bearer_auth(&self.api_key)
            .multipart(form)
            .send()
            .await?;
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
        Ok(parsed
            .get("text")
            .and_then(|text| text.as_str())
            .unwrap_or_default()
            .to_string())
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
            .client
            .post(GROQ_API_URL)
            .bearer_auth(&self.api_key)
            .json(&req_body)
            .send()
            .await?;

        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            bail!("API request failed with status {}: {}", status, body);
        }

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
