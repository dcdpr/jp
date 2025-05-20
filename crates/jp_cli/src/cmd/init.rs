use std::{collections::HashMap, fs, path::PathBuf};

use crossterm::style::Stylize as _;
use jp_conversation::{
    model::{ProviderId, Reasoning},
    Model, ModelId, Persona, PersonaId,
};
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

        for (id, model) in default_models() {
            let id = ModelId::try_from((model.provider, id))?;
            workspace.create_model_with_id(id, model)?;
        }

        let id = PersonaId::try_from("default")?;
        workspace.create_persona_with_id(id, Persona::default())?;

        workspace.persist()?;

        Ok(format!("Initialized workspace at {}", root.to_string_lossy().bold()).into())
    }
}

fn default_models() -> Vec<(&'static str, Model)> {
    vec![
        ("claude-3.7-sonnet", Model {
            provider: ProviderId::Openrouter,
            slug: "anthropic/claude-3.7-sonnet".to_string(),
            max_tokens: None,
            reasoning: Some(Reasoning::default()),
            temperature: Some(1.0),
            stop_words: vec![],
            additional_parameters: HashMap::new(),
        }),
        ("claude-3.5-haiku", Model {
            provider: ProviderId::Openrouter,
            slug: "anthropic/claude-3.5-haiku".to_string(),
            max_tokens: None,
            reasoning: None,
            temperature: Some(1.0),
            stop_words: vec![],
            additional_parameters: HashMap::new(),
        }),
        ("chatgpt-o3-mini-high", Model {
            provider: ProviderId::Openrouter,
            slug: "openai/o3-mini-high".to_string(),
            max_tokens: None,
            reasoning: None,
            temperature: Some(1.0),
            stop_words: vec![],
            additional_parameters: HashMap::new(),
        }),
        ("chatgpt-o1", Model {
            provider: ProviderId::Openrouter,
            slug: "openai/chatgpt-4o-latest".to_string(),
            max_tokens: None,
            reasoning: Some(Reasoning::default()),
            temperature: Some(1.0),
            stop_words: vec![],
            additional_parameters: HashMap::new(),
        }),
        ("chatgpt-4o-latest", Model {
            provider: ProviderId::Openrouter,
            slug: "openai/chatgpt-4o-latest".to_string(),
            max_tokens: None,
            reasoning: None,
            temperature: Some(1.0),
            stop_words: vec![],
            additional_parameters: HashMap::new(),
        }),
        ("grok", Model {
            provider: ProviderId::Openrouter,
            slug: "x-ai/grok-beta".to_string(),
            max_tokens: None,
            reasoning: None,
            temperature: Some(1.0),
            stop_words: vec![],
            additional_parameters: HashMap::new(),
        }),
        ("deepseek-r1", Model {
            provider: ProviderId::Openrouter,
            slug: "deepseek/deepseek-r1".to_string(),
            max_tokens: None,
            reasoning: Some(Reasoning::default()),
            temperature: Some(1.0),
            stop_words: vec![],
            additional_parameters: HashMap::new(),
        }),
        ("google-gemini-2.5-pro", Model {
            provider: ProviderId::Openrouter,
            slug: "google/gemini-2.5-pro-preview-03-25".to_string(),
            max_tokens: Some(1_000_000),
            reasoning: Some(Reasoning::default()),
            temperature: Some(1.0),
            stop_words: vec![],
            additional_parameters: HashMap::new(),
        }),
    ]
}
