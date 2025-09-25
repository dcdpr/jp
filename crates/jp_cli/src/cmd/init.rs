use std::{env, fs, path::PathBuf, str::FromStr as _};

use crossterm::style::Stylize as _;
use duct::cmd;
use jp_config::{
    conversation::tool::RunMode,
    model::id::{ModelIdConfig, Name, PartialModelIdConfig, ProviderId},
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

#[expect(clippy::too_many_lines)]
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

    if has_anthropic() {
        cfg.providers.llm.aliases.extend([
            ("anthropic".to_owned(), PartialModelIdConfig {
                provider: Some(ProviderId::Anthropic),
                name: Some(Name("claude-sonnet-4-0".into())),
            }),
            ("claude".to_owned(), PartialModelIdConfig {
                provider: Some(ProviderId::Anthropic),
                name: Some(Name("claude-sonnet-4-0".into())),
            }),
            ("sonnet".to_owned(), PartialModelIdConfig {
                provider: Some(ProviderId::Anthropic),
                name: Some(Name("claude-sonnet-4-0".into())),
            }),
            ("opus".to_owned(), PartialModelIdConfig {
                provider: Some(ProviderId::Anthropic),
                name: Some(Name("claude-opus-4-1".into())),
            }),
            ("haiku".to_owned(), PartialModelIdConfig {
                provider: Some(ProviderId::Anthropic),
                name: Some(Name("claude-3-5-haiku-latest".into())),
            }),
        ]);
    }

    if has_openai() {
        cfg.providers.llm.aliases.extend([
            ("openai".to_owned(), PartialModelIdConfig {
                provider: Some(ProviderId::Openai),
                name: Some(Name("gpt-5".into())),
            }),
            ("chatgpt".to_owned(), PartialModelIdConfig {
                provider: Some(ProviderId::Openai),
                name: Some(Name("gpt-5".into())),
            }),
            ("gpt".to_owned(), PartialModelIdConfig {
                provider: Some(ProviderId::Openai),
                name: Some(Name("gpt-5".into())),
            }),
            ("gpt5".to_owned(), PartialModelIdConfig {
                provider: Some(ProviderId::Openai),
                name: Some(Name("gpt-5".into())),
            }),
            ("gpt5-mini".to_owned(), PartialModelIdConfig {
                provider: Some(ProviderId::Openai),
                name: Some(Name("gpt-5-mini".into())),
            }),
            ("gpt-mini".to_owned(), PartialModelIdConfig {
                provider: Some(ProviderId::Openai),
                name: Some(Name("gpt-5-mini".into())),
            }),
            ("gpt5-nano".to_owned(), PartialModelIdConfig {
                provider: Some(ProviderId::Openai),
                name: Some(Name("gpt-5-nano".into())),
            }),
            ("gpt-nano".to_owned(), PartialModelIdConfig {
                provider: Some(ProviderId::Openai),
                name: Some(Name("gpt-5-nano".into())),
            }),
            ("o3-research".to_owned(), PartialModelIdConfig {
                provider: Some(ProviderId::Openai),
                name: Some(Name("o3-deep-research".into())),
            }),
            ("o4-mini-research".to_owned(), PartialModelIdConfig {
                provider: Some(ProviderId::Openai),
                name: Some(Name("o4-mini-deep-research".into())),
            }),
            ("codex".to_owned(), PartialModelIdConfig {
                provider: Some(ProviderId::Openai),
                name: Some(Name("gpt-5-codex".into())),
            }),
            ("gpt-5-codex".to_owned(), PartialModelIdConfig {
                provider: Some(ProviderId::Openai),
                name: Some(Name("gpt-5-codex".into())),
            }),
            ("codex-mini".to_owned(), PartialModelIdConfig {
                provider: Some(ProviderId::Openai),
                name: Some(Name("codex-mini-latest".into())),
            }),
        ]);
    }

    if has_google() {
        cfg.providers.llm.aliases.extend([
            ("google".to_owned(), PartialModelIdConfig {
                provider: Some(ProviderId::Google),
                name: Some(Name("gemini-2.5-pro".into())),
            }),
            ("gemini".to_owned(), PartialModelIdConfig {
                provider: Some(ProviderId::Google),
                name: Some(Name("gemini-2.5-pro".into())),
            }),
            ("gemini-pro".to_owned(), PartialModelIdConfig {
                provider: Some(ProviderId::Google),
                name: Some(Name("gemini-2.5-pro".into())),
            }),
            ("gemini-flash".to_owned(), PartialModelIdConfig {
                provider: Some(ProviderId::Google),
                name: Some(Name("gemini-2.5-flash".into())),
            }),
            ("gemini-lite".to_owned(), PartialModelIdConfig {
                provider: Some(ProviderId::Google),
                name: Some(Name("gemini-2.5-flash-lite".into())),
            }),
        ]);
    }

    cfg
}

fn has_anthropic() -> bool {
    env::var("ANTHROPIC_API_KEY").is_ok()
}

fn has_openai() -> bool {
    env::var("OPENAI_API_KEY").is_ok()
}

fn has_google() -> bool {
    env::var("GOOGLE_API_KEY").is_ok()
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
    use serial_test::serial;

    use super::*;

    pub(crate) struct EnvVarGuard {
        name: String,
        original_value: Option<String>,
    }

    impl EnvVarGuard {
        pub fn set(name: &str, value: &str) -> Self {
            let name = name.to_string();
            let original_value = std::env::var(&name).ok();
            unsafe { std::env::set_var(&name, value) };
            Self {
                name,
                original_value,
            }
        }
    }

    impl Drop for EnvVarGuard {
        fn drop(&mut self) {
            if let Some(ref original) = self.original_value {
                unsafe { std::env::set_var(&self.name, original) };
            } else {
                unsafe { std::env::remove_var(&self.name) };
            }
        }
    }

    #[test]
    #[serial(env_vars)]
    fn test_default_config() {
        let _env1 = EnvVarGuard::set("ANTHROPIC_API_KEY", "foo");
        let _env2 = EnvVarGuard::set("OPENAI_API_KEY", "bar");
        let _env3 = EnvVarGuard::set("GOOGLE_API_KEY", "baz");

        let config = default_config();

        insta::assert_toml_snapshot!(config);
    }
}
