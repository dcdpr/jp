use crate::{
    Context, Tool,
    util::{ToolResult, unknown_tool},
};

mod add_intent;
mod commit;
mod diff;
mod list_patches;
mod stage_patch;
mod stage_raw_patch;
mod unstage;

use add_intent::git_add_intent;
use commit::git_commit;
use diff::git_diff;
use list_patches::git_list_patches;
use stage_patch::git_stage_patch;
use stage_raw_patch::git_stage_raw_patch;
use unstage::git_unstage;

pub async fn run(ctx: Context, t: Tool) -> ToolResult {
    match t.name.trim_start_matches("git_") {
        "add_intent" => git_add_intent(&ctx.root, t.req("paths")?).await,

        "commit" => git_commit(ctx.root, t.req("message")?).await,

        "stage_patch" => {
            git_stage_patch(ctx, &t.answers, t.req("path")?, t.req("patch_ids")?).await
        }

        "stage_raw_patch" => git_stage_raw_patch(ctx, t.req("path")?, t.req("diff")?).await,

        "list_patches" => git_list_patches(&ctx.root, t.req("files")?),

        "unstage" => git_unstage(&ctx.root, t.req("paths")?).await,

        "diff" => git_diff(ctx.root, t.req("paths")?, t.opt("cached")?).await,

        _ => unknown_tool(t),
    }
}
