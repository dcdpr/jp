use crate::{workspace::Workspace, Config};

#[derive(Debug)]
pub struct Context {
    pub config: Config,
    pub workspace: Option<Workspace>,
}
