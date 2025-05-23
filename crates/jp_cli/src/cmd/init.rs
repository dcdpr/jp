use std::{fs, path::PathBuf};

use crossterm::style::Stylize as _;
use jp_conversation::{Persona, PersonaId};
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
        workspace.create_persona_with_id(id, Persona::default())?;

        workspace.persist()?;

        Ok(format!("Initialized workspace at {}", root.to_string_lossy().bold()).into())
    }
}
