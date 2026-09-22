use serde::Deserialize;
use std::env;
use std::fs;
use std::path::PathBuf;

#[derive(Deserialize, Clone, Debug)]
pub struct Config {
    #[serde(default = "ApiConfig::default")]
    pub api: ApiConfig,
    #[serde(default = "CaptureConfig::default")]
    pub capture: CaptureConfig,
    #[serde(default = "UiConfig::default")]
    pub ui: UiConfig,
    #[serde(default = "KeybindConfig::default")]
    pub keybind: KeybindConfig,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            api: ApiConfig::default(),
            capture: CaptureConfig::default(),
            ui: UiConfig::default(),
            keybind: KeybindConfig::default(),
        }
    }
}

#[derive(Deserialize, Clone, Debug)]
pub struct ApiConfig {
    #[serde(default = "default_provider")]
    pub provider: String,
    #[serde(default)]
    pub api_key: String,
    #[serde(default = "default_model")]
    pub model: String,
    #[serde(default = "default_true")]
    pub stream: bool,
    #[serde(default = "default_max_tokens")]
    pub max_tokens: u32,
}

impl Default for ApiConfig {
    fn default() -> Self {
        Self {
            provider: default_provider(),
            api_key: String::new(),
            model: default_model(),
            stream: default_true(),
            max_tokens: default_max_tokens(),
        }
    }
}

#[derive(Deserialize, Clone, Debug)]
pub struct CaptureConfig {
    #[serde(default = "CaptureMode::default")]
    pub mode: CaptureMode,
    #[serde(default = "default_true")]
    pub ocr: bool,
    #[serde(default = "default_true")]
    pub include_clipboard: bool,
}

impl Default for CaptureConfig {
    fn default() -> Self {
        Self {
            mode: CaptureMode::default(),
            ocr: default_true(),
            include_clipboard: default_true(),
        }
    }
}

#[derive(Deserialize, Clone, Debug, Default)]
#[serde(rename_all = "snake_case")]
pub enum CaptureMode {
    Fullscreen,
    #[default]
    ActiveWindow,
    Region,
    ClipboardOnly,
}

#[derive(Deserialize, Clone, Debug)]
pub struct UiConfig {
    #[serde(default = "default_position")]
    pub position: String,
    #[serde(default = "default_width")]
    pub width: i32,
    #[serde(default = "default_opacity")]
    pub opacity: f64,
    #[serde(default = "default_font")]
    pub font: String,
    #[serde(default = "default_theme")]
    pub theme: String,
}

impl Default for UiConfig {
    fn default() -> Self {
        Self {
            position: default_position(),
            width: default_width(),
            opacity: default_opacity(),
            font: default_font(),
            theme: default_theme(),
        }
    }
}

#[derive(Deserialize, Clone, Debug)]
pub struct KeybindConfig {
    #[serde(default = "default_true")]
    pub trigger_socket: bool,
}

impl Default for KeybindConfig {
    fn default() -> Self {
        Self {
            trigger_socket: default_true(),
        }
    }
}

fn default_provider() -> String {
    "groq".to_string()
}

fn default_model() -> String {
    "openai/gpt-oss-20b".to_string()
}

fn default_true() -> bool {
    true
}

fn default_max_tokens() -> u32 {
    1024
}

fn default_position() -> String {
    "center".to_string()
}

fn default_width() -> i32 {
    720
}

fn default_opacity() -> f64 {
    0.92
}

fn default_font() -> String {
    "JetBrains Mono 11".to_string()
}

fn default_theme() -> String {
    "dark".to_string()
}

impl Config {
    pub fn config_path() -> PathBuf {
        dirs::config_dir().unwrap().join("hyprly/config.toml")
    }

    pub fn load() -> anyhow::Result<Self> {
        let path = Self::config_path();
        let mut config = if path.exists() {
            let contents = fs::read_to_string(&path)?;
            toml::from_str(&contents)?
        } else {
            Config::default()
        };

        if config.api.api_key.is_empty() {
            if let Ok(key) = env::var("GROQ_API_KEY") {
                config.api.api_key = key.trim().to_string();
            }
        } else {
            config.api.api_key = config.api.api_key.trim().to_string();
        }

        Ok(config)
    }
}
