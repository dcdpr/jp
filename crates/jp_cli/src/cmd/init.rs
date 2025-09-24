use std::{env, fs, path::PathBuf, str::FromStr as _};

use crossterm::style::Stylize as _;
use duct::cmd;
use jp_config::{
    conversation::tool::RunMode,
    model::id::{ModelIdConfig, PartialModelIdConfig},
    PartialAppConfig,
};
use jp_workspace::Workspace;
use path_clean::PathClean as _;

use crate::{ctx::IntoPartialAppConfig, Output, DEFAULT_STORAGE_DIR};

#[derive(Debug, clap::Args)]
pub(crate) struct Init {
    /// Path to initialize the workspace at. Defaults to the current directory.
    path: Option<PathBuf>,
}

impl Init {
    pub(crate) fn run(&self) -> Output {
        let cwd = std::env::current_dir()?;
        let mut root = self
            .path
            .clone()
            .unwrap_or_else(|| PathBuf::from("."))
            .clean();

        if !root.is_absolute() {
            root = cwd.join(root);
        }

        fs::create_dir_all(&root)?;

        let storage = root.join(DEFAULT_STORAGE_DIR);
        let id = jp_workspace::Id::new();
        jp_id::global::set(id.to_string());

        let mut workspace =
            Workspace::new_with_id(root.clone(), id.clone()).persisted_at(&storage)?;

        id.store(&storage)?;

        workspace = workspace.with_local_storage()?;

        let mut config = default_config();
        if let Some(id) = default_model() {
            print!("Using model {}", id.to_string().bold().blue());
            let note = "  (to use a different model, update `.jp/config.toml`)".to_owned();
            println!("{}\n", note.grey().italic());

            config.assistant.model.id = PartialModelIdConfig {
                provider: Some(id.provider),
                name: Some(id.name),
            };
        }

        let data = toml::to_string_pretty(&config)?;
        fs::write(storage.join("config.toml"), data)?;
        fs::create_dir_all(storage.join("config.d"))?;

        workspace.persist()?;

        let loc = if root == cwd {
            "current directory".to_owned()
        } else {
            root.to_string_lossy().bold().to_string()
        };

        Ok(format!("Initialized workspace at {loc}").into())
    }
}

fn default_config() -> jp_config::PartialAppConfig {
    let mut cfg = jp_config::PartialAppConfig::default();
    cfg.extends
        .get_or_insert_default()
        .push("config.d/**/*".into());

    // This is a required field without a default value (that is, the
    // `ToolsDefaultsConfig` type does not set a default value for `run`).
    //
    // By setting it explicitly, we ensure that the default generated config
    // file has this value set, which exposes it to the user. This is desired,
    // as this is an important security feature, which we don't want users to
    // have to rely on a default value that might change in the future.
    cfg.conversation.tools.defaults.run = Some(RunMode::Ask);

    cfg
}

fn default_model() -> Option<ModelIdConfig> {
    env::var("JP_CFG_ASSISTANT_MODEL_ID")
        .ok()
        .and_then(|v| ModelIdConfig::from_str(&v).ok())
        .or_else(|| {
            let models = cmd!("ollama", "list")
                .pipe(cmd!("cut", "-d", " ", "-f1"))
                .pipe(cmd!("tail", "-n+2"))
                .read()
                .unwrap_or_default();

            let models = models.lines().map(str::trim).collect::<Vec<_>>();
            let model = if let Some(model) = models.iter().find(|m| m.starts_with("llama")) {
                model
            } else if let Some(model) = models.iter().find(|m| m.starts_with("gemma")) {
                model
            } else if let Some(model) = models.iter().find(|m| m.starts_with("qwen")) {
                model
            } else {
                return None;
            };

            format!("ollama/{model}").parse().ok()
        })
        // TODO: Use `Config` env vars here.
        .or_else(|| {
            env::var("ANTHROPIC_API_KEY")
                .is_ok()
                .then(|| "anthropic/claude-sonnet-4-0".parse().ok())
                .flatten()
        })
        .or_else(|| {
            env::var("OPENAI_API_KEY")
                .is_ok()
                .then(|| "openai/o4-mini".parse().ok())
                .flatten()
        })
        .or_else(|| {
            env::var("GEMINI_API_KEY")
                .is_ok()
                .then(|| "google/gemini-2.5-flash-preview-05-20".parse().ok())
                .flatten()
        })
}

impl IntoPartialAppConfig for Init {
    fn apply_cli_config(
        &self,
        _workspace: Option<&Workspace>,
        partial: PartialAppConfig,
    ) -> std::result::Result<PartialAppConfig, Box<dyn std::error::Error + Send + Sync>> {
        Ok(partial)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = default_config();

        insta::assert_toml_snapshot!(config);
    }
}
