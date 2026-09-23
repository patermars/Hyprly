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
        Self {
            model: default_model(),
            max_tokens: default_max_tokens(),
            api_key: String::new(),
        }
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
    #[serde(default = "default_min_utterance_seconds")]
    pub min_utterance_seconds: u32,
    #[serde(default = "default_max_utterance_seconds")]
    pub max_utterance_seconds: u32,
    #[serde(default = "default_transcription_model")]
    pub transcription_model: String,
    #[serde(default = "default_max_concurrent_transcriptions")]
    pub max_concurrent_transcriptions: usize,
    #[serde(default = "default_language")]
    pub language: String,
    #[serde(default = "default_vad_enabled")]
    pub vad_enabled: bool,
    #[serde(default = "default_silence_threshold")]
    pub silence_threshold: f32,
    #[serde(default = "default_endpoint_silence_ms")]
    pub endpoint_silence_ms: u64,
    #[serde(default = "default_debounce_ms")]
    pub debounce_ms: u64,
    #[serde(default = "default_filler_filter")]
    pub filler_filter: bool,
    #[serde(default)]
    pub confidence_threshold: f32,
}

impl Default for AudioConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            source: String::new(),
            chunk_seconds: default_chunk_seconds(),
            min_utterance_seconds: default_min_utterance_seconds(),
            max_utterance_seconds: default_max_utterance_seconds(),
            transcription_model: default_transcription_model(),
            max_concurrent_transcriptions: default_max_concurrent_transcriptions(),
            language: default_language(),
            vad_enabled: default_vad_enabled(),
            silence_threshold: default_silence_threshold(),
            endpoint_silence_ms: default_endpoint_silence_ms(),
            debounce_ms: default_debounce_ms(),
            filler_filter: default_filler_filter(),
            confidence_threshold: 0.0,
        }
    }
}

fn default_model() -> String {
    "openai/gpt-oss-20b".to_string()
}
fn default_max_tokens() -> u32 {
    1024
}
fn default_chunk_seconds() -> u32 {
    4
}
fn default_min_utterance_seconds() -> u32 {
    2
}
fn default_max_utterance_seconds() -> u32 {
    10
}
fn default_transcription_model() -> String {
    "whisper-large-v3-turbo".to_string()
}
fn default_max_concurrent_transcriptions() -> usize {
    2
}
fn default_language() -> String {
    "en".to_string()
}
fn default_vad_enabled() -> bool {
    true
}
fn default_silence_threshold() -> f32 {
    0.02
}
fn default_endpoint_silence_ms() -> u64 {
    900
}
fn default_debounce_ms() -> u64 {
    300
}
fn default_filler_filter() -> bool {
    true
}

impl Config {
    pub fn config_path() -> PathBuf {
        dirs::config_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("hyprly/config.toml")
    }

    pub fn load() -> anyhow::Result<Self> {
        let path = Self::config_path();
        let mut config = if path.exists() {
            toml::from_str(&fs::read_to_string(path)?)?
        } else {
            Self::default()
        };
        if config.api.api_key.is_empty() {
            config.api.api_key = env::var("GROQ_API_KEY")
                .unwrap_or_default()
                .trim()
                .to_string();
        }
        config.audio.chunk_seconds = config.audio.chunk_seconds.clamp(2, 4);
        config.audio.min_utterance_seconds = config.audio.min_utterance_seconds.clamp(1, 4);
        config.audio.max_utterance_seconds = config
            .audio
            .max_utterance_seconds
            .clamp(config.audio.min_utterance_seconds, 15);
        config.audio.max_concurrent_transcriptions =
            config.audio.max_concurrent_transcriptions.clamp(1, 4);
        config.audio.endpoint_silence_ms = config.audio.endpoint_silence_ms.clamp(800, 1_500);
        config.audio.debounce_ms = config.audio.debounce_ms.clamp(150, 700);
        Ok(config)
    }
}
