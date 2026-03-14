use std::{env, fs, io, str::FromStr as _};

use camino::{FromPathBufError, Utf8PathBuf};
use clean_path::Clean as _;
use crossterm::style::Stylize as _;
use duct::cmd;
use inquire::{Select, Text};
use jp_config::{
    AppConfig, PartialAppConfig,
    conversation::tool::RunMode,
    model::id::{ModelIdConfig, Name, ProviderId},
};
use jp_printer::Printer;
use jp_workspace::Workspace;
use schematic::{ConfigEnum as _, SchemaBuilder, Schematic as _};

use crate::{DEFAULT_STORAGE_DIR, cmd::Output, ctx::IntoPartialAppConfig};

#[derive(Debug, clap::Args)]
pub(crate) struct Init {
    /// Path to initialize the workspace at. Defaults to the current directory.
    path: Option<Utf8PathBuf>,
}

impl Init {
    #[expect(unused_assignments)]
    pub(crate) fn run(&self, printer: &Printer) -> Output {
        let cwd: Utf8PathBuf = std::env::current_dir()?
            .try_into()
            .map_err(FromPathBufError::into_io_error)?;

        let mut root: Utf8PathBuf = self
            .path
            .clone()
            .unwrap_or_else(|| Utf8PathBuf::from("."))
            .into_std_path_buf()
            .clean()
            .try_into()
            .map_err(FromPathBufError::into_io_error)?;

        if !root.is_absolute() {
            root = cwd.join(root);
        }

        fs::create_dir_all(&root)?;

        let storage = root.join(DEFAULT_STORAGE_DIR);
        let id = jp_workspace::Id::new();

        let mut workspace =
            Workspace::new_with_id(root.clone(), id.clone()).persisted_at(&storage)?;

        id.store(&storage)?;

        workspace = workspace.with_local_storage()?;

        // Interactive configuration
        let _run_mode = Self::ask_run_mode(&mut printer.out_writer(), true)?;
        let (_provider, _name) = Self::ask_model(&mut printer.out_writer())?;

        let builder = SchemaBuilder::default();
        let schema = AppConfig::build_schema(builder);
        std::fs::write("schema.json", serde_json::to_string_pretty(&schema)?)?;

        // // Generate Config
        // let mut generator = SchemaGenerator::default();
        // generator.add::<AppConfig>();
        //
        // // 1. conversation.tools.defaults.run
        // let run_variants = RunMode::variants();
        // let run_idx = run_variants
        //     .iter()
        //     .position(|v| *v == run_mode)
        //     .unwrap_or(0);
        //
        // let run_schema = Schema::new(SchemaType::Enum(Box::new(EnumType {
        //     values: run_variants
        //         .iter()
        //         .map(|v| LiteralValue::String(v.to_string()))
        //         .collect(),
        //     default_index: Some(run_idx),
        //     ..Default::default()
        // })));
        //
        // // 2. assistant.model.id
        // // We override the leaf nodes for provider and name.
        // let provider_schema = Schema::new(SchemaType::String(Box::new(StringType {
        //     default: Some(LiteralValue::String(provider.to_string())),
        //     ..Default::default()
        // })));
        //
        // let name_schema = Schema::new(SchemaType::String(Box::new(StringType {
        //     default: Some(LiteralValue::String(name.to_string())),
        //     ..Default::default()
        // })));
        //
        // let options = TemplateOptions {
        //     header: indoc::formatdoc! {"
        //         # This is a TOML config file.
        //
        //     "},
        //     custom_values: HashMap::from([
        //         ("conversation.tools.defaults.run".to_owned(), run_schema),
        //         ("assistant.model.id.provider".to_owned(), provider_schema),
        //         ("assistant.model.id.name".to_owned(), name_schema),
        //     ]),
        //     hide_fields: vec![
        //         "inherit",
        //         "extends",
        //         "assistant.instructions",
        //         // "assistant.name",
        //         // "conversation.attachments",
        //     ]
        //     .into_iter()
        //     .map(Into::into)
        //     .collect(),
        //
        //     print_enum_values: false,
        //
        //     env_vars: AppConfig::envs().into_iter().collect(),
        //     ..Default::default()
        // };
        // let renderer = TomlTemplateRenderer::new(options);
        //
        // let config_path = storage.join("test.toml");
        // generator
        //     .generate(&config_path, renderer)
        //     .map_err(io::Error::other)?;
        //
        // // Create config.d directory as well
        // // fs::create_dir_all(storage.join("config.d"))?;

        let loc = if root == cwd {
            "current directory".to_owned()
        } else {
            root.to_string().bold().to_string()
        };

        printer.println(format!("Initialized workspace at {loc}"));
        Ok(())
    }

    fn ask_run_mode(
        writer: &mut dyn io::Write,
        help: bool,
    ) -> Result<RunMode, Box<dyn std::error::Error + Send + Sync>> {
        let mut options = vec![
            format!("Yes {}", "(safest option)".green()),
            format!("No  {}", "(potentially dangerous)".red()),
        ];

        if help {
            options.push("Help…".to_owned());
        }

        let answer = Select::new("Confirm before running tools?", options)
            .with_help_message(
                "You can always configure individual tools you deem safe to run without \
                 confirmation.",
            )
            .with_starting_cursor(0)
            .prompt_with_writer(writer)?;

        if answer == "Help…" {
            let _err = indoc::writedoc!(
                writer,
                r"

                    # Recommended Configuration

                    Yes (confirm before running tools)

                    # Summary

                    The assistant runs tools on your local machine, these
                    can perform destructive actions and should therefore
                    be run with a human-in-the-loop confirmation.

                    # Details

                    When using JP, the assistant needs to run tools on
                    your local machine to perform certain tasks such as
                    modifying files, running CLI tools, etc.

                    Most of these tools are safe to run, but some can
                    be potentially dangerous, depending on the
                    arguments provided to them.

                    While all of JP's built-in tools are confined to the
                    workspace root, externally supplied tools cannot be
                    restricted in the same way, and can potentially run
                    any command on your system.

                    For example, a potentially external tool `rm` could
                    take an argument `file`, which could be an absolute
                    path to a file outside of your workspace root,
                    deleting files from your system that you don't want
                    to delete.

                    To avoid this, you should configure the assistant to
                    run these tools with a human-in-the-loop confirmation.
                    This will ensure that the assistant only runs tools
                    that you explicitly allow it to run.

                    You can also configure the assistant to run tools
                    automatically, which means it will run tools without
                    asking you first.

                    The answer to this question will be used as the default
                    for all tools that are run by the assistant, but each
                    tool can also be configured to run with a different
                    mode, by editing your config file after the workspace
                    is initialized.

                "
            );
            writer.flush()?;

            return Self::ask_run_mode(writer, false);
        }

        Ok(if answer.starts_with("Yes") {
            RunMode::Unattended
        } else {
            RunMode::Ask
        })
    }

    fn ask_model(
        writer: &mut dyn io::Write,
    ) -> Result<(ProviderId, Name), Box<dyn std::error::Error + Send + Sync>> {
        let models = Self::detect_models();

        let mut options: Vec<String> = models.iter().map(ToString::to_string).collect();
        options.push("Other (enter manually)".to_string());

        let ans = Select::new("Select an AI model to use:", options.clone())
            .with_help_message("We detected these models based on your environment.")
            .prompt_with_writer(writer)?;

        if ans == "Other (enter manually)" {
            let providers = ProviderId::variants();
            let provider_strs: Vec<String> = providers.iter().map(ToString::to_string).collect();

            let provider_str =
                Select::new("Select a provider:", provider_strs).prompt_with_writer(writer)?;

            let provider =
                ProviderId::from_str(&provider_str).map_err(|e| io::Error::other(e.to_string()))?;

            let name = Text::new("Enter the model name:")
                .with_placeholder("e.g. gpt-4o")
                .prompt_with_writer(writer)?;

            Ok((provider, Name(name)))
        } else {
            let m = models.iter().find(|m| m.to_string() == ans).unwrap();
            Ok((m.provider, m.name.clone()))
        }
    }

    fn detect_models() -> Vec<ModelIdConfig> {
        let mut models = Vec::new();

        if has_anthropic()
            && let Some(m) = default_model_id_for(ProviderId::Anthropic)
        {
            models.push(m);
        }
        if has_openai()
            && let Some(m) = default_model_id_for(ProviderId::Openai)
        {
            models.push(m);
        }
        if has_google()
            && let Some(m) = default_model_id_for(ProviderId::Google)
        {
            models.push(m);
        }

        if let Ok(output) = cmd!("ollama", "list").read() {
            for line in output.lines().skip(1) {
                let Some(name) = line.split_whitespace().next() else {
                    continue;
                };

                if name.is_empty() {
                    continue;
                }

                let name = name.split(':').next().unwrap_or(name);
                models.push(ModelIdConfig {
                    provider: ProviderId::Ollama,
                    name: Name(name.to_owned()),
                });
            }
        }

        models.sort();
        models.dedup();
        models
    }
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

fn default_model_id_for(provider: ProviderId) -> Option<ModelIdConfig> {
    let name = match provider {
        ProviderId::Anthropic => Name("claude-sonnet-4-5".into()),
        ProviderId::Google => Name("gemini-3-pro-preview".into()),
        ProviderId::Openai => Name("gpt-5.2".into()),
        _ => return None,
    };

    Some(ModelIdConfig { provider, name })
}

impl IntoPartialAppConfig for Init {
    fn apply_cli_config(
        &self,
        _workspace: Option<&Workspace>,
        partial: PartialAppConfig,
        _: Option<&PartialAppConfig>,
    ) -> std::result::Result<PartialAppConfig, Box<dyn std::error::Error + Send + Sync>> {
        Ok(partial)
    }
}

#[cfg(test)]
#[path = "init_tests.rs"]
mod tests;
