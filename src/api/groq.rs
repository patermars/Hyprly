use crate::api::types::{ChatMessage, ChatRequest, ChatResponse};
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
