use std::{
    env, fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result};
use directories::ProjectDirs;
use documented::DocumentedFields;
use path_clean::PathClean as _;
use serde::{Deserialize, Serialize};

use crate::{find_root, workspace::WORKSPACE_CONFIG};

// Default values
const DEFAULT_SERVER_PORT: u16 = 8080;
const DEFAULT_SERVER_ADDRESS: &str = "0.0.0.0";
const DEFAULT_OPENROUTER_API_KEY_ENV: &str = "OPENROUTER_API_KEY";
const DEFAULT_OPENROUTER_APP_NAME: &str = "JP";

const DEFAULT_CHAT_MODEL: &str = "anthropic/claude-3.5-sonnet";
const DEFAULT_CHAT_MAX_TOKENS: u32 = 8192;
const DEFAULT_CHAT_TEMPERATURE: f64 = 0.0;

const DEFAULT_REASONING_MODEL: &str = "deepseek/deepseek-r1";
const DEFAULT_REASONING_MAX_TOKENS: u32 = 8192;
const DEFAULT_REASONING_TEMPERATURE: f64 = 0.6;
const DEFAULT_REASONING_STOP_WORD: &str = "</think>";

// File paths and environment variables
const PROJECT_CONFIG_FILENAME: &str = ".jp.toml";
const QUALIFIER: &str = "com";
const ORGANIZATION: &str = "jeanmertz";
const APPLICATION: &str = "jp";
const GLOBAL_CONFIG_FILENAME: &str = "config.toml";
const GLOBAL_CONFIG_ENV_VAR: &str = "JP_GLOBAL_CONFIG_FILE";

// File representation - uses Option<T> for all fields to allow partial specification
#[derive(Debug, Serialize, Deserialize, Clone, Default, DocumentedFields)]
#[serde(default)]
pub(crate) struct ConfigFile {
    /// Inherit from a local ancestor or global configuration file.
    #[serde(skip_serializing_if = "Option::is_none")]
    inherit: Option<bool>,

    /// Server configuration for API access.
    #[serde(skip_serializing_if = "Option::is_none")]
    server: Option<ServerConfigFile>,

    /// OpenRouter API configuration.
    #[serde(skip_serializing_if = "Option::is_none")]
    openrouter: Option<OpenRouterConfigFile>,

    /// LLM model configuration.
    #[serde(skip_serializing_if = "Option::is_none")]
    llm: Option<LlmConfigFile>,
}

#[derive(Debug, Serialize, Deserialize, Clone, Default, DocumentedFields)]
#[serde(default)]
struct ServerConfigFile {
    /// Port the server listens on.
    #[serde(skip_serializing_if = "Option::is_none")]
    port: Option<u16>,

    /// IP address the server binds to.
    #[serde(skip_serializing_if = "Option::is_none")]
    address: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone, Default, DocumentedFields)]
#[serde(default)]
struct OpenRouterConfigFile {
    /// Environment variable that contains the API key.
    #[serde(skip_serializing_if = "Option::is_none")]
    api_key_env: Option<String>,

    /// Application name sent to OpenRouter.
    #[serde(skip_serializing_if = "Option::is_none")]
    app_name: Option<String>,

    /// Optional HTTP referrer t o send with requests.
    #[serde(skip_serializing_if = "Option::is_none")]
    app_referrer: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone, Default, DocumentedFields)]
#[serde(default)]
struct LlmConfigFile {
    /// Configuration for the main conversation model.
    #[serde(skip_serializing_if = "Option::is_none")]
    chat: Option<ModelConfigFile>,

    /// Configuration for the reasoning/thinking model.
    #[serde(skip_serializing_if = "Option::is_none")]
    reasoning: Option<ModelConfigFile>,
}

#[derive(Debug, Serialize, Deserialize, Clone, Default, DocumentedFields)]
#[serde(default)]
struct ModelConfigFile {
    /// Maximum number of tokens in the response.
    #[serde(skip_serializing_if = "Option::is_none")]
    max_tokens: Option<u32>,

    /// Temperature parameter (higher means more creative).
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f64>,

    /// Model identifier to use (see: <https://openrouter.ai/models>)
    #[serde(skip_serializing_if = "Option::is_none")]
    model: Option<String>,

    /// Optional stop word used to stop the reasoning model after completion of
    /// its reasoning phase.
    ///
    /// For `deepseek/deepseek-r1`, the stop word is `</think>`.
    #[serde(skip_serializing_if = "Option::is_none")]
    stop_word: Option<Option<String>>,
}

// Runtime configuration - fully resolved values
#[derive(Debug, Clone)]
pub struct Config {
    pub inherit: bool,
    pub server: ServerConfig,
    pub openrouter: OpenRouterConfig,
    pub llm: LlmConfig,
}

#[derive(Debug, Clone)]
pub struct ServerConfig {
    pub port: u16,
    pub address: String,
}

#[derive(Debug, Clone)]
pub struct OpenRouterConfig {
    pub api_key_env: String,
    pub app_name: String,
    pub app_referrer: Option<String>,
}

#[derive(Debug, Clone)]
pub struct LlmConfig {
    pub chat: ModelConfig,
    pub reasoning: ModelConfig,
}

#[derive(Debug, Clone)]
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

    pub fn stop_word(&self) -> Option<&str> {
        match self {
            Self::Chat { .. } => None,
            Self::Reasoning { stop_word, .. } => stop_word.as_deref(),
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

impl Default for Config {
    fn default() -> Self {
        Self {
            inherit: true,
            server: ServerConfig {
                port: DEFAULT_SERVER_PORT,
                address: DEFAULT_SERVER_ADDRESS.to_string(),
            },
            openrouter: OpenRouterConfig {
                api_key_env: DEFAULT_OPENROUTER_API_KEY_ENV.to_string(),
                app_name: DEFAULT_OPENROUTER_APP_NAME.to_string(),
                app_referrer: None,
            },
            llm: LlmConfig {
                chat: ModelConfig::default_chat(),
                reasoning: ModelConfig::default_reasoning(),
            },
        }
    }
}

impl ConfigFile {
    pub(crate) fn set_inherit(&mut self, inherit: bool) {
        self.inherit = Some(inherit);
    }

    /// Load configuration file from path
    fn load(path: impl AsRef<Path>) -> Result<Self> {
        let config_str = fs::read_to_string(&path).context(format!(
            "Failed to read config file from {:?}",
            &path.as_ref()
        ))?;

        let config: Self = toml::from_str(&config_str).context(format!(
            "Failed to parse TOML config from {:?}",
            path.as_ref()
        ))?;

        Ok(config)
    }

    /// Save configuration file to path
    pub(crate) fn save(self, path: impl AsRef<Path>, documented: bool) -> Result<()> {
        // Ensure parent directory exists
        if let Some(parent) = path.as_ref().parent() {
            fs::create_dir_all(parent)
                .context(format!("Failed to create directory at {:?}", parent))?;
        }

        let config_str = if documented {
            self.into_documented_string()
        } else {
            toml::to_string_pretty(&self).context("Failed to serialize config to TOML")?
        };

        fs::write(&path, config_str)
            .context(format!("Failed to write config to {:?}", path.as_ref()))?;

        Ok(())
    }

    /// Merge with another ConfigFile, taking values from other when specified
    fn merge(self, other: Self) -> Self {
        Self {
            inherit: other.inherit.or(self.inherit),
            server: match (self.server, other.server) {
                (_, Some(other_server)) => Some(other_server),
                (self_server, None) => self_server,
            },
            openrouter: match (self.openrouter, other.openrouter) {
                (Some(self_or), Some(other_or)) => Some(self_or.merge(other_or)),
                (_, Some(other_or)) => Some(other_or),
                (self_or, None) => self_or,
            },
            llm: match (self.llm, other.llm) {
                (Some(self_llm), Some(other_llm)) => Some(self_llm.merge(other_llm)),
                (_, Some(other_llm)) => Some(other_llm),
                (self_llm, None) => self_llm,
            },
        }
    }

    /// Convert to runtime Config with all values resolved
    fn build(self) -> Config {
        // Start with defaults
        let mut config = Config {
            inherit: self.inherit.unwrap_or(true),
            ..Config::default()
        };

        // Apply server config if present
        if let Some(server_file) = self.server {
            if let Some(port) = server_file.port {
                config.server.port = port;
            }
            if let Some(address) = server_file.address {
                config.server.address = address;
            }
        }

        // Apply OpenRouter config if present
        if let Some(or_file) = self.openrouter {
            if let Some(api_key_env) = or_file.api_key_env {
                config.openrouter.api_key_env = api_key_env;
            }
            if let Some(app_name) = or_file.app_name {
                config.openrouter.app_name = app_name;
            }
            if let Some(app_referrer) = or_file.app_referrer {
                config.openrouter.app_referrer = Some(app_referrer);
            }
        }

        // Apply LLM config if present
        if let Some(llm_file) = self.llm {
            // Apply chat config if present
            if let Some(chat_file) = llm_file.chat {
                // Start with default chat config
                let mut chat_config = match config.llm.chat {
                    ModelConfig::Chat {
                        max_tokens,
                        temperature,
                        model,
                    } => (max_tokens, temperature, model),
                    _ => unreachable!(), // Default is always Chat
                };

                // Apply overrides
                if let Some(max_tokens) = chat_file.max_tokens {
                    chat_config.0 = max_tokens;
                }
                if let Some(temperature) = chat_file.temperature {
                    chat_config.1 = temperature;
                }
                if let Some(model) = chat_file.model {
                    chat_config.2 = model;
                }

                // Update config
                config.llm.chat = ModelConfig::Chat {
                    max_tokens: chat_config.0,
                    temperature: chat_config.1,
                    model: chat_config.2,
                };
            }

            // Apply reasoning config if present
            if let Some(reasoning_file) = llm_file.reasoning {
                // Start with default reasoning config
                let mut reasoning_config = match &config.llm.reasoning {
                    ModelConfig::Reasoning {
                        max_tokens,
                        temperature,
                        model,
                        stop_word,
                    } => (*max_tokens, *temperature, model.clone(), stop_word.clone()),
                    _ => unreachable!(), // Default is always Reasoning
                };

                // Apply overrides
                if let Some(max_tokens) = reasoning_file.max_tokens {
                    reasoning_config.0 = max_tokens;
                }
                if let Some(temperature) = reasoning_file.temperature {
                    reasoning_config.1 = temperature;
                }
                if let Some(model) = reasoning_file.model {
                    reasoning_config.2 = model;
                }
                if let Some(stop_word) = reasoning_file.stop_word {
                    reasoning_config.3 = stop_word; // Option<Option<String>> -> Option<String>
                }

                // Update config
                config.llm.reasoning = ModelConfig::Reasoning {
                    max_tokens: reasoning_config.0,
                    temperature: reasoning_config.1,
                    model: reasoning_config.2,
                    stop_word: reasoning_config.3,
                };
            }
        }

        config
    }

    pub fn to_documented_toml(
        config_to_write: &Config,
        with_header: bool,
    ) -> Result<String, toml::ser::Error> {
        let config_file = Self::from(config_to_write);
        let default_file = Self::from(&Config::default());

        // Start with header if requested
        let mut result = String::new();
        if with_header {
            result.push_str("# Jean-Pierre, a programmer's LLM toolkit.\n");
            result.push_str("# ----------------------------------------\n");
            result.push_str("#\n");
            result.push_str("# This is a configuration file with explanatory comments.\n");
            result.push_str("# - Commented options (#) use default values\n");
            result.push_str("# - Uncommented options override defaults\n\n");
            result.push_str("");
        }

        // Convert to TOML tables
        let config_table = config_file.to_toml_table()?;
        let default_table = default_file.to_toml_table()?;

        // Process root section
        Self::document_value::<ConfigFile>(&mut result, "", config_table, default_table)?;

        Ok(result)
    }

    /// Convert ConfigFile to TOML Value table
    fn to_toml_table(&self) -> Result<toml::value::Table, toml::ser::Error> {
        let serialized = toml::to_string(self)?;
        let value: toml::Value =
            toml::from_str(&serialized).expect("Failed to parse serialized config");

        if let toml::Value::Table(table) = value {
            Ok(table)
        } else {
            panic!("Expected table from serialized config");
        }
    }

    /// Recursively document values, adding fields and their documentation
    fn document_value<D: DocumentedFields>(
        result: &mut String,
        path: &str,
        config: toml::value::Table,
        default: toml::value::Table,
    ) -> Result<(), toml::ser::Error> {
        // Process each key in the table
        for (key, value) in config {
            let field_path = if path.is_empty() {
                key.clone()
            } else {
                format!("{}.{}", path, key)
            };

            if let toml::Value::Table(table) = value {
                // For nested tables, print section header if not at root
                commented_field_docs::<D>(result, &key);
                result.push_str(&format!("[{}]\n", field_path));
                if path.is_empty() {
                    result.push('\n');
                }

                // Get default table for comparison
                let default_subtable = if let Some(toml::Value::Table(t)) = default.get(&key) {
                    t.clone()
                } else {
                    toml::value::Table::new()
                };

                // Recursively process the subsection with the appropriate documentation type
                match field_path.as_str() {
                    "server" => Self::document_value::<ServerConfigFile>(
                        result,
                        &field_path,
                        table,
                        default_subtable,
                    )?,
                    "openrouter" => Self::document_value::<OpenRouterConfigFile>(
                        result,
                        &field_path,
                        table,
                        default_subtable,
                    )?,
                    "llm" => Self::document_value::<LlmConfigFile>(
                        result,
                        &field_path,
                        table,
                        default_subtable,
                    )?,
                    "llm.chat" | "llm.reasoning" => Self::document_value::<ModelConfigFile>(
                        result,
                        &field_path,
                        table,
                        default_subtable,
                    )?,
                    _ => Self::document_value::<D>(result, &field_path, table, default_subtable)?,
                };
            } else {
                // For scalar values, add documentation and the value itself
                commented_field_docs::<D>(result, &key);
                let default_value = default.get(&key);
                let is_default = match default_value {
                    Some(default_val) => &value == default_val,
                    None => false,
                };

                if is_default {
                    result.push_str("# ");
                }
                result.push_str(&format!("{} = {}\n\n", key, format_toml_value(&value)));
            }
        }

        Ok(())
    }

    /// Generate a configuration string based on the current ConfigFile
    pub fn into_documented_string(self) -> String {
        let config = self.build();
        Self::to_documented_toml(&config, true)
            .unwrap_or_else(|e| format!("# Error generating config: {}", e))
    }
}

impl From<&Config> for ConfigFile {
    fn from(config: &Config) -> Self {
        Self {
            inherit: Some(config.inherit),
            server: Some(ServerConfigFile {
                port: Some(config.server.port),
                address: Some(config.server.address.clone()),
            }),
            openrouter: Some(OpenRouterConfigFile {
                api_key_env: Some(config.openrouter.api_key_env.clone()),
                app_name: Some(config.openrouter.app_name.clone()),
                app_referrer: config.openrouter.app_referrer.clone(),
            }),
            llm: Some(LlmConfigFile {
                chat: Some(ModelConfigFile {
                    model: Some(config.llm.chat.model().to_owned()),
                    max_tokens: Some(config.llm.chat.max_tokens()),
                    temperature: Some(config.llm.chat.temperature()),
                    stop_word: None,
                }),
                reasoning: Some(ModelConfigFile {
                    model: Some(config.llm.reasoning.model().to_owned()),
                    max_tokens: Some(config.llm.reasoning.max_tokens()),
                    temperature: Some(config.llm.reasoning.temperature()),
                    stop_word: Some(config.llm.reasoning.stop_word().map(|s| s.to_owned())),
                }),
            }),
        }
    }
}

impl OpenRouterConfigFile {
    fn merge(self, other: Self) -> Self {
        Self {
            api_key_env: other.api_key_env.or(self.api_key_env),
            app_name: other.app_name.or(self.app_name),
            app_referrer: other.app_referrer.or(self.app_referrer),
        }
    }
}

impl LlmConfigFile {
    fn merge(self, other: Self) -> Self {
        Self {
            chat: match (self.chat, other.chat) {
                (Some(self_chat), Some(other_chat)) => Some(self_chat.merge(other_chat)),
                (_, Some(other_chat)) => Some(other_chat),
                (self_chat, None) => self_chat,
            },
            reasoning: match (self.reasoning, other.reasoning) {
                (Some(self_reasoning), Some(other_reasoning)) => {
                    Some(self_reasoning.merge(other_reasoning))
                }
                (_, Some(other_reasoning)) => Some(other_reasoning),
                (self_reasoning, None) => self_reasoning,
            },
        }
    }
}

impl ModelConfigFile {
    fn merge(self, other: Self) -> Self {
        Self {
            max_tokens: other.max_tokens.or(self.max_tokens),
            temperature: other.temperature.or(self.temperature),
            model: other.model.or(self.model),
            stop_word: other.stop_word.or(self.stop_word),
        }
    }
}

impl Config {
    /// Load configuration, respecting the hierarchical inheritance chain
    pub fn load(explicit_config_path: Option<&Path>) -> Result<Self> {
        // 1. Try explicit config path if provided
        if let Some(path) = explicit_config_path {
            return Self::load_from_explicit_path(path);
        }

        // 2. Load config files up the directory tree, possibly including global config
        let config_chain = Self::collect_config_chain()?;

        // 3. Merge the configs in the right order and build the final config
        Self::merge_config_chain(config_chain)
    }

    /// Load config from an explicitly specified path
    fn load_from_explicit_path(path: &Path) -> Result<Self> {
        if path.exists() {
            let config_file = ConfigFile::load(path)?;
            return Ok(config_file.build());
        }
        Err(anyhow::anyhow!(
            "Specified config file not found: {:?}",
            path
        ))
    }

    /// Collect configuration files by walking up the directory tree
    fn collect_config_chain() -> Result<Vec<ConfigFile>> {
        let current_dir = env::current_dir()?;
        let mut config_chain = Vec::new();
        let mut should_load_global = true;

        // First collect all local config files by walking up the directory tree
        Self::collect_local_configs(&current_dir, &mut config_chain, &mut should_load_global)?;

        // Then try to load global config if needed
        if should_load_global {
            Self::add_global_config_if_exists(&mut config_chain)?;
        }

        Ok(config_chain)
    }

    /// Collect local configuration files by walking up the directory tree
    fn collect_local_configs(
        start_dir: &Path,
        config_chain: &mut Vec<ConfigFile>,
        should_load_global: &mut bool,
    ) -> Result<()> {
        let mut current_path = start_dir.to_path_buf();

        loop {
            let Some(mut config_path) = find_project_config(&current_path) else {
                break;
            };

            let config = ConfigFile::load(&config_path)?;
            let should_inherit = config.inherit.unwrap_or(true);
            config_chain.push(config);

            // Check if inheritance should stop here
            if should_inherit {
                *should_load_global = false;
                break;
            }

            // Move up one directory
            if !config_path.pop() {
                break; // We've reached the root
            }

            current_path = config_path;
        }

        Ok(())
    }

    /// Add global configuration if it exists and should be loaded
    fn add_global_config_if_exists(config_chain: &mut Vec<ConfigFile>) -> Result<()> {
        let Some(global_config_path) = get_global_config_path(true) else {
            return Ok(());
        };

        config_chain.push(ConfigFile::load(&global_config_path)?);

        Ok(())
    }

    /// Merge configuration chain from most general to most specific
    fn merge_config_chain(config_chain: Vec<ConfigFile>) -> Result<Self> {
        // Start with default config
        let mut final_config = ConfigFile::default();

        // Read the config chain in reverse (from most general to most specific)
        for config in config_chain.into_iter().rev() {
            final_config = final_config.merge(config);
        }

        // Convert to runtime config with all values resolved
        Ok(final_config.build())
    }
}

/// Find project config by walking up the directory tree
fn find_project_config(start_dir: &Path) -> Option<PathBuf> {
    let mut current_dir = start_dir.to_path_buf();

    loop {
        let config_path = current_dir.join(PROJECT_CONFIG_FILENAME);
        if config_path.exists() {
            return Some(config_path);
        }

        if !current_dir.pop() {
            return None;
        }
    }
}

/// Get the path to the global config file.
///
/// If `must_exist` is true, the function will return `None` if the global
/// config path does not exist or is inaccessible.
pub(crate) fn get_global_config_path(must_exist: bool) -> Option<PathBuf> {
    env::var(GLOBAL_CONFIG_ENV_VAR)
        .ok()
        .and_then(|path| expand_tilde(&path))
        .map(|path| path.clean())
        .or_else(|| {
            ProjectDirs::from(QUALIFIER, ORGANIZATION, APPLICATION)
                .map(|p| p.config_dir().join(GLOBAL_CONFIG_FILENAME))
        })
        .filter(|path| !must_exist || (path.is_file() && path.exists()))
}

/// Get the path to the local config file starting from the current directory.
///
/// This function requires that the config file exists in the current directory
/// or a parent directory.
pub(crate) fn get_local_config_path() -> Option<PathBuf> {
    env::current_dir()
        .ok()
        .and_then(|current_dir| find_root(&current_dir))
        .map(|project_root| project_root.join(WORKSPACE_CONFIG))
}

/// Expand tilde in path to home directory
///
/// If no tilde is found, returns `Some` with the original path. If a tilde is
/// found, but no home directory is set, returns `None`.
fn expand_tilde(path: &str) -> Option<PathBuf> {
    if path.starts_with("~/") {
        return env::var("HOME")
            .ok()
            .map(|home| PathBuf::from(path.replacen("~", &home, 1)));
    }

    Some(PathBuf::from(path))
}

/// Format a TOML value to a string with proper quoting
fn format_toml_value(value: &toml::Value) -> String {
    match value {
        toml::Value::String(s) => format!("\"{}\"", s),
        toml::Value::Float(f) => {
            // Ensure floats have a decimal point
            if f.fract() == 0.0 {
                format!("{}.0", f)
            } else {
                f.to_string()
            }
        }
        _ => value.to_string(),
    }
}

fn commented_field_docs<D: DocumentedFields>(buf: &mut String, key: &str) {
    if let Ok(doc) = D::get_field_docs(key) {
        for line in doc.lines() {
            buf.push_str(&format!("# {}\n", line));
        }
    }
}
