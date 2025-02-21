use crate::{workspace::Workspace, Config};

#[derive(Debug, Default)]
pub struct Context {
    pub config: Config,
    pub workspace: Option<Workspace>,
}
