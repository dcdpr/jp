use std::{env, fs, path::PathBuf};

use crossterm::style::Stylize as _;
use duct::cmd;
use jp_conversation::{ModelId, Persona, PersonaId};
use jp_workspace::Workspace;
use path_clean::PathClean as _;

use crate::{Output, DEFAULT_STORAGE_DIR};

#[derive(Debug, clap::Args)]
pub struct Args {
    /// Path to initialize the workspace at. Defaults to the current directory.
    pub path: Option<PathBuf>,
}

impl Args {
    pub fn run(self) -> Output {
        let cwd = std::env::current_dir()?;
        let mut root = self.path.unwrap_or_else(|| PathBuf::from(".")).clean();
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

        let id = PersonaId::try_from("default")?;
        let persona = Persona {
            model: default_model(),
            ..Default::default()
        };

        if let Some(model) = persona.model.as_ref() {
            print!("Using model {}", model.to_string().bold().blue());
            let note =
                "  (to use a different model, update `.jp/personas/default.json`)".to_owned();
            println!("{}\n", note.grey().italic());
        }

        workspace.create_persona_with_id(id, persona)?;

        workspace.persist()?;

        Ok(format!("Initialized workspace at {}", root.to_string_lossy().bold()).into())
    }
}

fn default_model() -> Option<ModelId> {
    env::var("JP_LLM_MODEL_ID")
        .ok()
        .and_then(|v| ModelId::try_from(v).ok())
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
