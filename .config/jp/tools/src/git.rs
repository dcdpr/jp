use crate::{Error, Tool, Workspace};

mod commit;
// mod utils;

use commit::git_commit;

pub async fn run(ws: Workspace, t: Tool) -> std::result::Result<String, Error> {
    match t.name.trim_start_matches("git_") {
        "commit" => git_commit(ws.path, t.req("message")?).await,

        _ => Err(format!("Unknown tool '{}'", t.name).into()),
    }
}
