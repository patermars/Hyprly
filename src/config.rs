use serde::Deserialize;
use std::{env, fs, path::PathBuf};

#[derive(Deserialize, Clone, Debug, Default)]
pub struct Config {
    #[serde(default)]
    pub api: ApiConfig,
    #[serde(default)]
    pub audio: AudioConfig,
}

#[derive(Deserialize, Clone, Debug)]
pub struct ApiConfig {
    #[serde(default = "default_model")]
    pub model: String,
    #[serde(default = "default_max_tokens")]
    pub max_tokens: u32,
    #[serde(default)]
    pub api_key: String,
}

impl Default for ApiConfig {
    fn default() -> Self {
        Self { model: default_model(), max_tokens: default_max_tokens(), api_key: String::new() }
    }
}

#[derive(Deserialize, Clone, Debug)]
pub struct AudioConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub source: String,
    #[serde(default = "default_chunk_seconds")]
    pub chunk_seconds: u32,
    #[serde(default = "default_transcription_model")]
    pub transcription_model: String,
}

impl Default for AudioConfig {
    fn default() -> Self {
        Self { enabled: false, source: String::new(), chunk_seconds: default_chunk_seconds(), transcription_model: default_transcription_model() }
    }
}

fn default_model() -> String { "openai/gpt-oss-20b".to_string() }
fn default_max_tokens() -> u32 { 1024 }
fn default_chunk_seconds() -> u32 { 8 }
fn default_transcription_model() -> String { "whisper-large-v3-turbo".to_string() }

impl Config {
    pub fn config_path() -> PathBuf {
        dirs::config_dir().unwrap_or_else(|| PathBuf::from("." )).join("hyprly/config.toml")
    }

    pub fn load() -> anyhow::Result<Self> {
        let path = Self::config_path();
        let mut config = if path.exists() {
            toml::from_str(&fs::read_to_string(path)?)?
        } else {
            Self::default()
        };
        if config.api.api_key.is_empty() {
            config.api.api_key = env::var("GROQ_API_KEY").unwrap_or_default().trim().to_string();
        }
        Ok(config)
    }
}
