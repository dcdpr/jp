use crate::{Context, Error, Tool};

mod commit;
mod diff;
mod stage;
mod unstage;

// mod utils;

use commit::git_commit;
use diff::git_diff;
use stage::git_stage;
use unstage::git_unstage;

pub async fn run(ctx: Context, t: Tool) -> std::result::Result<String, Error> {
    match t.name.trim_start_matches("git_") {
        "commit" => git_commit(ctx.root, t.req("message")?).await,
        "stage" => git_stage(ctx.root, t.opt("paths")?, t.opt("patches")?).await,
        "unstage" => git_unstage(ctx.root, t.req("paths")?).await,
        "diff" => git_diff(ctx.root, t.req("paths")?, t.opt("cached")?).await,

        _ => Err(format!("Unknown tool '{}'", t.name).into()),
    }
}
