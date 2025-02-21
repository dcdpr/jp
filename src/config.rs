use std::{
    env, fs,
    path::{Path, PathBuf},
};

use anyhow::{Context as _, Result};
use confique::{toml::FormatOptions, Config as Confique, File, FileFormat, Partial};
use directories::ProjectDirs;
use path_clean::PathClean as _;

use crate::find_root;

// File paths and environment variables
pub const WORKSPACE_CONFIG_FILENAME: &str = ".jp.toml";

const QUALIFIER: &str = "com";
const ORGANIZATION: &str = "jeanmertz";
const APPLICATION: &str = "jp";
const GLOBAL_CONFIG_FILENAME: &str = "config.toml";
const GLOBAL_CONFIG_ENV_VAR: &str = "JP_GLOBAL_CONFIG_FILE";

#[derive(Debug, Clone, Confique)]
pub struct Config {
    /// Inherit from a local ancestor or global configuration file.
    #[config(default = true)]
    pub inherit: bool,

    /// Server configuration for API access.
    #[config(nested)]
    pub server: ServerConfig,

    /// OpenRouter API configuration.
    #[config(nested)]
    pub openrouter: OpenRouterConfig,

    /// LLM model configuration.
    #[config(nested)]
    pub llm: LlmConfig,

    /// Artifact configuration for embedding file contents.
    #[config(nested)]
    pub artifacts: ArtifactsConfig,
}

#[derive(Debug, Clone, Confique)]
pub struct ServerConfig {
    /// Port the server listens on.
    #[config(default = 8080)]
    pub port: u16,

    /// IP address the server binds to.
    #[config(default = "0.0.0.0")]
    pub address: String,
}

#[derive(Debug, Clone, Confique)]
pub struct OpenRouterConfig {
    /// Environment variable that contains the API key.
    #[config(default = "OPENROUTER_API_KEY")]
    pub api_key_env: String,

    /// Application name sent to OpenRouter.
    #[config(default = "JP")]
    pub app_name: String,

    /// Optional HTTP referrer to send with requests.
    pub app_referrer: Option<String>,
}

#[derive(Debug, Clone, Confique)]
pub struct LlmConfig {
    /// Configuration for the main conversation model.
    #[config(nested)]
    pub chat: ChatModelConfig,

    /// Configuration for the reasoning/thinking model.
    #[config(nested)]
    pub reasoning: ReasoningModelConfig,
}

#[derive(Debug, Clone, Confique)]
pub struct ChatModelConfig {
    /// Maximum number of tokens in the response.
    #[config(default = 8192)]
    pub max_tokens: u32,

    /// Temperature parameter (higher means more creative).
    #[config(default = 0.3)]
    pub temperature: f64,

    /// Model identifier to use (see: <https://openrouter.ai/models>)
    #[config(default = "anthropic/claude-3.5-sonnet")]
    pub model: String,

    /// System prompt to use with this model.
    #[config(default = "
You are an expert {lang} software engineer. You are collaborating with a team of developers to
build a {lang} application.
")]
    pub system_prompt: String,

    /// Whether to use the "web search" feature of OpenRouter for the chat
    /// model.
    ///
    /// See: <https://openrouter.ai/docs/features/web-search>
    #[config(default = true)]
    pub web_search: bool,

    #[config(default = "
Your core tasks include:
- Answering general programming questions.
- Explaining how the code works.
- Reviewing the discussed code.
- Generating unit tests for the selected code.
- Proposing fixes for problems in the selected code.
- Scaffolding code for a new workspace.
- Finding relevant code to the user's query.
- Proposing fixes for test failures.
- Answering questions about programming concepts.

You must:
- Follow the user's requirements carefully and to the letter.
- Keep your answers short and impersonal, especially if the user responds with context outside of your tasks.
- Minimize other prose.
- Use Markdown formatting in your answers.
- Include the programming language name at the start of the Markdown code blocks.
- Avoid including line numbers in code blocks.
- Avoid wrapping the whole response in triple backticks.
- Only return code that's relevant to the task at hand. You may not need to return all of the code that the user has shared.
- Use actual line breaks instead of '\\n' in your response to begin new lines.
- Use '\\n' only when you want a literal backslash followed by a character 'n'.

When given a task:
1. Think step-by-step and describe your plan for what to build in pseudocode, written out in great detail, unless asked not to do so.
2. Output the code in a single code block, being careful to only return relevant code.
3. You should always generate short suggestions for the next user message that are relevant to the conversation.
4. You can only give one reply for each conversation message.
")]
    pub instructions: String,
}

impl ChatModelConfig {
    pub fn model(&self) -> ModelConfig {
        ModelConfig::from(self)
    }
}

#[derive(Debug, Clone, Confique)]
pub struct ReasoningModelConfig {
    /// Maximum number of tokens in the response.
    #[config(default = 8192)]
    pub max_tokens: u32,

    /// Temperature parameter (higher means more creative).
    #[config(default = 0.7)]
    pub temperature: f64,

    /// Model identifier to use (see: <https://openrouter.ai/models>)
    #[config(default = "deepseek/deepseek-r1")]
    pub model: String,

    /// Optional stop word used to stop the reasoning model after completion of
    /// its reasoning phase.
    ///
    /// For `deepseek/deepseek-r1`, the stop word is `</think>`.
    #[config(default = "</think>")]
    pub stop_word: String,

    /// System prompt to use with this model.
    #[config(
        default = "You are a helpful expert software engineer. Your colleague has asked you to reason about a software challenge. Your response will inform their final decision making. Process each request thoughtfully and methodically."
    )]
    pub system_prompt: String,

    /// Whether to use the "web search" feature of OpenRouter for the reasoning
    /// model.
    ///
    /// See: <https://openrouter.ai/docs/features/web-search>
    #[config(default = true)]
    pub web_search: bool,

    #[config(default = "
Your core tasks include:
- Answering general programming questions.
- Explaining how the code works.
- Reviewing the discussed code.
- Generating unit tests for the selected code.
- Proposing fixes for problems in the selected code.
- Scaffolding code for a new workspace.
- Finding relevant code to the user's query.
- Proposing fixes for test failures.
- Answering questions about programming concepts.

You must:
- Follow the user's requirements carefully and to the letter.
- Keep your answers short and impersonal, especially if the user responds with context outside of your tasks.
- Minimize other prose.
- Use Markdown formatting in your answers.
- Include the programming language name at the start of the Markdown code blocks.
- Avoid including line numbers in code blocks.
- Avoid wrapping the whole response in triple backticks.
- Only return code that's relevant to the task at hand. You may not need to return all of the code that the user has shared.
- Use actual line breaks instead of '\\n' in your response to begin new lines.
- Use '\\n' only when you want a literal backslash followed by a character 'n'.

When given a task:
1. Think step-by-step and describe your plan for what to build in pseudocode, written out in great detail, unless asked not to do so.
2. Output the code in a single code block, being careful to only return relevant code.
3. You should always generate short suggestions for the next user message that are relevant to the conversation.
4. You can only give one reply for each conversation message.
")]
    pub instructions: String,
}

impl ReasoningModelConfig {
    pub fn model(&self) -> ModelConfig {
        ModelConfig::from(self)
    }
}

#[derive(Debug, Clone, Confique)]
pub struct ArtifactsConfig {
    /// Path to the ignore file with gitignore syntax.
    #[config(default = ".jp/ignore")]
    pub ignorefile: String,
}

impl Config {
    /// Load configuration, respecting the hierarchical inheritance chain
    pub fn load(explicit_config_path: Option<&Path>) -> Result<Self> {
        type PartialConfig = <Config as Confique>::Partial;

        // 1. Try explicit config path if provided
        if let Some(path) = explicit_config_path {
            if path.exists() {
                return File::with_format(path, FileFormat::Toml)
                    .load()
                    .and_then(Self::from_partial)
                    .context(format!("Failed to load config from {:?}", path));
            }

            return Err(anyhow::anyhow!(
                "Specified config file not found: {:?}",
                path
            ));
        }

        // 2. Find the chain of configuration files
        let mut config_partials = Vec::new();
        let mut current_dir = env::current_dir()?;
        let mut inherit = true;

        // Start with local configs by walking up the directory tree
        while inherit && current_dir.parent().is_some() {
            let config_path = current_dir.join(WORKSPACE_CONFIG_FILENAME);

            if config_path.exists() {
                // Load config and check inheritance flag
                let partial = File::with_format(&config_path, FileFormat::Toml)
                    .load::<PartialConfig>()
                    .context(format!("Failed to parse config at {:?}", config_path))?;

                // Check if inheritance should continue
                inherit = partial.inherit.unwrap_or(true);
                config_partials.push(partial);
            }

            // Move up one directory
            if !current_dir.pop() {
                break;
            }
        }

        // 3. Add global config if needed and inheritance is allowed
        if inherit {
            if let Some(global_path) = get_global_config_path(true) {
                let global_partial = File::with_format(global_path, FileFormat::Toml)
                    .load::<PartialConfig>()
                    .context("Failed to load global config")?;

                config_partials.push(global_partial);
            }
        }

        // 4. Merge all partials in reverse order (most general to most specific)
        let mut merged = PartialConfig::default_values();
        for partial in config_partials.into_iter().rev() {
            merged = partial.with_fallback(merged);
        }

        // 5. Convert to final config
        Self::from_partial(merged).context("Failed to create final config")
    }

    /// Generate a documented template file with default values as comments
    pub fn generate_template(with_header: bool) -> String {
        let template = confique::toml::template::<Self>(FormatOptions::default());

        if with_header {
            format!(
                "# Jean's Personal LLM Toolkit Configuration\n\
                 # ------------------------------------------\n\
                 #\n\
                 # This is a configuration file with explanatory comments.\n\
                 # - Commented options (#) use default values\n\
                 # - Uncommented options override defaults\n\
                 \n{}",
                template
            )
        } else {
            template
        }
    }

    /// Save a documented template to file
    pub fn save_template(path: impl AsRef<Path>, with_header: bool) -> Result<()> {
        let template = Self::generate_template(with_header);

        if let Some(parent) = path.as_ref().parent() {
            fs::create_dir_all(parent)
                .context(format!("Failed to create directory at {:?}", parent))?;
        }

        fs::write(&path, template)
            .context(format!("Failed to write config to {:?}", path.as_ref()))?;

        Ok(())
    }
}

// /// Find project config by walking up the directory tree
// fn find_project_config(start_dir: &Path) -> Option<PathBuf> {
//     let mut current_dir = start_dir.to_path_buf();
//
//     loop {
//         let config_path = current_dir.join(WORKSPACE_CONFIG_FILENAME);
//         if config_path.exists() {
//             return Some(config_path);
//         }
//
//         if !current_dir.pop() {
//             return None;
//         }
//     }
// }

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
        .map(|project_root| project_root.join(WORKSPACE_CONFIG_FILENAME))
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

#[derive(Debug, Clone)]
pub enum ModelConfig {
    Chat {
        max_tokens: u32,
        temperature: f64,
        model: String,
        system_prompt: String,
        web_search: bool,
    },
    Reasoning {
        max_tokens: u32,
        temperature: f64,
        model: String,
        stop_word: Option<String>,
        system_prompt: String,
        web_search: bool,
    },
}

impl From<&ChatModelConfig> for ModelConfig {
    fn from(config: &ChatModelConfig) -> Self {
        Self::Chat {
            max_tokens: config.max_tokens,
            temperature: config.temperature,
            model: config.model.clone(),
            system_prompt: config.system_prompt.clone(),
            web_search: config.web_search,
        }
    }
}

impl From<&ReasoningModelConfig> for ModelConfig {
    fn from(config: &ReasoningModelConfig) -> Self {
        Self::Reasoning {
            max_tokens: config.max_tokens,
            temperature: config.temperature,
            model: config.model.clone(),
            stop_word: Some(config.stop_word.clone()),
            system_prompt: config.system_prompt.clone(),
            web_search: config.web_search,
        }
    }
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

    pub fn system_prompt(&self) -> &str {
        match self {
            Self::Chat { system_prompt, .. } => system_prompt,
            Self::Reasoning { system_prompt, .. } => system_prompt,
        }
    }

    pub fn system_prompt_mut(&mut self) -> &mut String {
        match self {
            Self::Chat { system_prompt, .. } => system_prompt,
            Self::Reasoning { system_prompt, .. } => system_prompt,
        }
    }

    pub fn web_search(&self) -> bool {
        match self {
            Self::Chat { web_search, .. } => *web_search,
            Self::Reasoning { web_search, .. } => *web_search,
        }
    }
}
