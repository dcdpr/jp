use crate::{Context, Error, Tool};

mod commit;
// mod utils;

use commit::git_commit;

pub async fn run(ctx: Context, t: Tool) -> std::result::Result<String, Error> {
    match t.name.trim_start_matches("git_") {
        "commit" => git_commit(ctx.root, t.req("message")?).await,

        _ => Err(format!("Unknown tool '{}'", t.name).into()),
    }
}
