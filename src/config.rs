use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::{env, fs};

const DEFAULT_SERVER_PORT: u16 = 8080;
const DEFAULT_SERVER_ADDRESS: &str = "0.0.0.0";
const DEFAULT_OPENROUTER_API_KEY_ENV: &str = "OPENROUTER_API_KEY";
const DEFAULT_OPENROUTER_APP_NAME: &str = "ClaudeR";

const DEFAULT_CHAT_MODEL: &str = "anthropic/claude-3.5-sonnet";
const DEFAULT_CHAT_MAX_TOKENS: u32 = 8192;
const DEFAULT_CHAT_TEMPERATURE: f64 = 0.0;

const DEFAULT_REASONING_MODEL: &str = "deepseek/deepseek-r1";
const DEFAULT_REASONING_MAX_TOKENS: u32 = 8192;
const DEFAULT_REASONING_TEMPERATURE: f64 = 0.6;
const DEFAULT_REASONING_STOP_WORD: &str = "</think>";

const DEFAULT_CONFIG_PATHS: &[&str] = &[
    "./clauder.toml",
    "./config/clauder.toml",
    "~/.config/clauder.toml",
];

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(default)]
#[derive(Default)]
pub struct Config {
    pub server: ServerConfig,
    pub openrouter: OpenRouterConfig,
    pub llm: LlmConfig,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ServerConfig {
    #[serde(default = "default_port")]
    pub port: u16,
    #[serde(default = "default_address")]
    pub address: String,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            port: default_port(),
            address: default_address(),
        }
    }
}

fn default_port() -> u16 {
    DEFAULT_SERVER_PORT
}

fn default_address() -> String {
    DEFAULT_SERVER_ADDRESS.to_string()
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct OpenRouterConfig {
    #[serde(default = "default_api_key_env")]
    pub api_key_env: String,
    #[serde(default = "default_app_name")]
    pub app_name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub app_referrer: Option<String>,
}

impl Default for OpenRouterConfig {
    fn default() -> Self {
        Self {
            api_key_env: default_api_key_env(),
            app_name: default_app_name(),
            app_referrer: None,
        }
    }
}

fn default_api_key_env() -> String {
    DEFAULT_OPENROUTER_API_KEY_ENV.to_string()
}

fn default_app_name() -> String {
    DEFAULT_OPENROUTER_APP_NAME.to_string()
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct LlmConfig {
    #[serde(default = "ModelConfig::default_chat")]
    pub chat: ModelConfig,
    #[serde(default = "ModelConfig::default_reasoning")]
    pub reasoning: ModelConfig,
}

impl Default for LlmConfig {
    fn default() -> Self {
        Self {
            chat: ModelConfig::default_chat(),
            reasoning: ModelConfig::default_reasoning(),
        }
    }
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(untagged)]
pub enum ModelConfig {
    Chat {
        max_tokens: u32,
        temperature: f64,
        model: String,
    },
    Reasoning {
        max_tokens: u32,
        temperature: f64,
        model: String,
        stop_word: Option<String>,
    },
}

impl ModelConfig {
    pub fn is_reasoning(&self) -> bool {
        matches!(self, Self::Reasoning { .. })
    }

    pub fn max_tokens(&self) -> u32 {
        match self {
            Self::Chat { max_tokens, .. } => *max_tokens,
            Self::Reasoning { max_tokens, .. } => *max_tokens,
        }
    }

    pub fn temperature(&self) -> f64 {
        match self {
            Self::Chat { temperature, .. } => *temperature,
            Self::Reasoning { temperature, .. } => *temperature,
        }
    }

    pub fn model(&self) -> &str {
        match self {
            Self::Chat { model, .. } => model,
            Self::Reasoning { model, .. } => model,
        }
    }

    fn default_chat() -> Self {
        Self::Chat {
            max_tokens: DEFAULT_CHAT_MAX_TOKENS,
            temperature: DEFAULT_CHAT_TEMPERATURE,
            model: DEFAULT_CHAT_MODEL.to_string(),
        }
    }

    fn default_reasoning() -> Self {
        Self::Reasoning {
            max_tokens: DEFAULT_REASONING_MAX_TOKENS,
            temperature: DEFAULT_REASONING_TEMPERATURE,
            model: DEFAULT_REASONING_MODEL.to_string(),
            stop_word: Some(DEFAULT_REASONING_STOP_WORD.to_string()),
        }
    }
}

impl Config {
    pub fn load(config_path: Option<&str>) -> Result<Self> {
        config_path
            .iter()
            .chain(DEFAULT_CONFIG_PATHS.iter())
            .map(|path| path.replace("~", &env::var("HOME").unwrap_or_default()))
            .map(PathBuf::from)
            .find(|p| p.exists())
            .map(Self::load_from_file)
            .unwrap_or_else(|| Ok(Self::default()))
    }

    fn load_from_file(path: impl AsRef<Path>) -> Result<Self> {
        let config_str = fs::read_to_string(&path).context(format!(
            "Failed to read config file from {:?}",
            &path.as_ref()
        ))?;

        let config: Config = toml::from_str(&config_str).context(format!(
            "Failed to parse TOML config from {:?}",
            path.as_ref()
        ))?;

        Ok(config)
    }

    pub fn save(&self, path: impl AsRef<Path>) -> Result<()> {
        let config_str =
            toml::to_string_pretty(self).context("Failed to serialize config to TOML")?;

        fs::write(&path, config_str)
            .context(format!("Failed to write config to {:?}", path.as_ref()))?;

        Ok(())
    }
}

pub fn generate_default_config(path: impl AsRef<Path>) -> Result<()> {
    let config = Config::default();
    config.save(path)
}
