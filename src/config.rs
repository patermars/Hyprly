use serde::{Deserialize, Serialize};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::{
    env, fs,
    io::Write,
    path::{Path, PathBuf},
};

pub(crate) const MAX_API_KEYS: usize = 3;
pub(crate) const MIN_MOUSE_BUTTON: u16 = 0x110;
pub(crate) const MAX_MOUSE_BUTTON: u16 = 0x11f;
pub(crate) const MAX_GESTURE_CLICKS: u8 = 10;

#[derive(Deserialize, Serialize, Clone, Debug, Default)]
pub struct Config {
    #[serde(default)]
    pub api: ApiConfig,
    #[serde(default)]
    pub audio: AudioConfig,
    #[serde(default)]
    pub context: ContextConfig,
    #[serde(default)]
    pub screen: ScreenConfig,
}

#[derive(Deserialize, Serialize, Clone, Debug)]
pub struct ScreenConfig {
    #[serde(default = "default_vision_model")]
    pub vision_model: String,
    #[serde(default)]
    pub mouse_gesture: Option<MouseGesture>,
    #[serde(default = "default_answer_target")]
    pub answer_target: String,
    #[serde(default)]
    pub overlay_window: OverlayWindowConfig,
}

#[derive(Deserialize, Serialize, Clone, Copy, Debug)]
pub struct OverlayWindowConfig {
    #[serde(default = "default_overlay_min_width")]
    pub min_width: u32,
    #[serde(default = "default_overlay_width")]
    pub width: u32,
    #[serde(default = "default_overlay_max_width")]
    pub max_width: u32,
    #[serde(default = "default_overlay_min_height")]
    pub min_height: u32,
    #[serde(default = "default_overlay_height")]
    pub height: u32,
    #[serde(default = "default_overlay_max_height")]
    pub max_height: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub position_x: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub position_y: Option<i32>,
}

impl Default for OverlayWindowConfig {
    fn default() -> Self {
        Self {
            min_width: default_overlay_min_width(),
            width: default_overlay_width(),
            max_width: default_overlay_max_width(),
            min_height: default_overlay_min_height(),
            height: default_overlay_height(),
            max_height: default_overlay_max_height(),
            position_x: None,
            position_y: None,
        }
    }
}

fn default_overlay_min_width() -> u32 {
    320
}

fn default_overlay_width() -> u32 {
    643
}

fn default_overlay_max_width() -> u32 {
    1200
}

fn default_overlay_min_height() -> u32 {
    120
}

fn default_overlay_height() -> u32 {
    189
}

fn default_overlay_max_height() -> u32 {
    378
}

fn default_answer_target() -> String {
    "webpage".to_string()
}

impl Default for ScreenConfig {
    fn default() -> Self {
        Self {
            vision_model: default_vision_model(),
            mouse_gesture: None,
            answer_target: default_answer_target(),
            overlay_window: OverlayWindowConfig::default(),
        }
    }
}

#[derive(Deserialize, Serialize, Clone, Debug)]
pub struct MouseGesture {
    pub button: u16,
    pub clicks: u8,
}

#[derive(Deserialize, Serialize, Clone, Debug)]
pub struct ContextConfig {
    #[serde(default)]
    pub files: Vec<String>,
    #[serde(default = "default_context_max_chars")]
    pub max_chars: usize,
}

impl Default for ContextConfig {
    fn default() -> Self {
        Self {
            files: Vec::new(),
            max_chars: default_context_max_chars(),
        }
    }
}

fn default_context_max_chars() -> usize {
    24_000
}

#[derive(Deserialize, Serialize, Clone, Debug)]
pub struct ApiConfig {
    #[serde(default = "default_model")]
    pub model: String,
    #[serde(default = "default_max_tokens")]
    pub max_tokens: u32,
    #[serde(default)]
    pub api_key: String,
    #[serde(default)]
    pub api_keys: Vec<String>,
}

impl Default for ApiConfig {
    fn default() -> Self {
        Self {
            model: default_model(),
            max_tokens: default_max_tokens(),
            api_key: String::new(),
            api_keys: Vec::new(),
        }
    }
}

#[derive(Deserialize, Serialize, Clone, Debug)]
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
fn default_vision_model() -> String {
    "qwen/qwen3.8-27b".to_string()
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

    pub fn save_config_text(contents: &str) -> anyhow::Result<()> {
        // Reject malformed or structurally invalid configuration before replacing the file.
        let _: Self = toml::from_str(contents)?;
        let path = Self::config_path();
        Self::write_config(&path, contents)
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
        if config.api.api_keys.is_empty() && !config.api.api_key.trim().is_empty() {
            config
                .api
                .api_keys
                .push(config.api.api_key.trim().to_string());
        }
        config.api.api_keys.retain(|key| !key.trim().is_empty());
        config.api.api_keys.truncate(MAX_API_KEYS);
        if let Some(first) = config.api.api_keys.first() {
            config.api.api_key = first.clone();
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
        config.context.max_chars = config.context.max_chars.clamp(1_000, 100_000);
        if let Some(gesture) = &mut config.screen.mouse_gesture {
            gesture.button = gesture.button.clamp(MIN_MOUSE_BUTTON, MAX_MOUSE_BUTTON);
            gesture.clicks = gesture.clicks.clamp(1, MAX_GESTURE_CLICKS);
        }
        Ok(config)
    }

    pub fn save_mouse_gesture(gesture: Option<&MouseGesture>) -> anyhow::Result<()> {
        Self::update_config_document(|document| {
            let screen = table_section_mut(document, "screen")?;
            if let Some(gesture) = gesture {
                let mut table = toml_edit::Table::new();
                table["button"] = toml_edit::value(gesture.button as i64);
                table["clicks"] = toml_edit::value(gesture.clicks as i64);
                screen["mouse_gesture"] = toml_edit::Item::Table(table);
            } else {
                screen.remove("mouse_gesture");
            }
            Ok(())
        })
    }

    pub fn save_answer_target(target: &str) -> anyhow::Result<()> {
        let value = match target {
            "floating" | "both" => target,
            _ => "webpage",
        };
        Self::update_config_document(|document| {
            let screen = table_section_mut(document, "screen")?;
            screen["answer_target"] = toml_edit::value(value);
            Ok(())
        })
    }

    pub fn save_api_keys(api_keys: &[String]) -> anyhow::Result<()> {
        Self::update_config_document(|document| {
            let api = table_section_mut(document, "api")?;
            let keys: Vec<String> = api_keys
                .iter()
                .map(|key| key.trim().to_string())
                .filter(|key| !key.is_empty())
                .take(MAX_API_KEYS)
                .collect();
            api["api_key"] = toml_edit::value(keys.first().cloned().unwrap_or_default());
            api["api_keys"] = toml_edit::value(toml_edit::Array::from_iter(keys.iter().cloned()));

            let audio = table_section_mut(document, "audio")?;
            if !audio.contains_key("enabled") {
                audio["enabled"] = toml_edit::value(true);
            }
            Ok(())
        })
    }

    fn update_config_document(
        update: impl FnOnce(&mut toml_edit::DocumentMut) -> anyhow::Result<()>,
    ) -> anyhow::Result<()> {
        let path = Self::config_path();
        let mut document = read_config_document(&path)?;
        update(&mut document)?;
        Self::write_config(&path, &document.to_string())
    }

    fn write_config(path: &Path, contents: &str) -> anyhow::Result<()> {
        let parent = path.parent().unwrap_or_else(|| std::path::Path::new("."));
        fs::create_dir_all(parent)?;
        fs::set_permissions(parent, fs::Permissions::from_mode(0o700))?;
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(path)?;
        file.write_all(contents.as_bytes())?;
        file.sync_all()?;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
        Ok(())
    }
}

fn read_config_document(path: &Path) -> anyhow::Result<toml_edit::DocumentMut> {
    if path.exists() {
        Ok(fs::read_to_string(path)?.parse()?)
    } else {
        Ok(toml_edit::DocumentMut::new())
    }
}

fn table_section_mut<'a>(
    document: &'a mut toml_edit::DocumentMut,
    section: &str,
) -> anyhow::Result<&'a mut toml_edit::Table> {
    if !document.contains_key(section) {
        document[section] = toml_edit::Item::Table(toml_edit::Table::new());
    }
    document[section]
        .as_table_mut()
        .ok_or_else(|| anyhow::anyhow!("The [{section}] config section must be a TOML table"))
}
