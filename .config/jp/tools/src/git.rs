use serde_json::{Map, Value};

use crate::{
    Context, Tool,
    util::{ToolResult, unknown_tool},
};

mod add_intent;
mod apply;
mod commit;
mod diff;
mod list_patches;
mod stage_patch;
mod stage_patch_lines;
mod unstage;

use add_intent::git_add_intent;
use commit::git_commit;
use diff::git_diff;
use list_patches::git_list_patches;
use stage_patch::git_stage_patch;
use stage_patch_lines::git_stage_patch_lines;
use unstage::git_unstage;

pub async fn run(ctx: Context, t: Tool) -> ToolResult {
    let opts = &t.options;

    match t.name.trim_start_matches("git_") {
        "add_intent" => git_add_intent(&ctx.root, t.req("paths")?, opts).await,

        "commit" => git_commit(ctx.root, t.req("message")?, opts).await,

        "stage_patch" => git_stage_patch(ctx, &t.answers, t.req("patches")?, opts).await,

        "stage_patch_lines" => {
            let path: String = t.req("path")?;
            let patch_id: usize = t.req("patch_id")?;
            let lines: Vec<Value> = t.req("lines")?;
            git_stage_patch_lines(&ctx.root, &path, patch_id, lines, opts)
        }

        "list_patches" => git_list_patches(&ctx.root, t.opt("files")?, opts),

        "unstage" => git_unstage(&ctx.root, t.req("paths")?, opts).await,

        "diff" => git_diff(ctx.root, t.opt("paths")?, t.req("status")?, opts).await,

        _ => unknown_tool(t),
    }
}

/// Extract environment variables from the `env` tool option.
///
/// Returns a list of (key, value) pairs that should be passed to git
/// subprocesses. This allows callers (e.g. integration tests) to inject
/// env vars like `GIT_CONFIG_GLOBAL` to isolate git from host config.
fn env_from_options(options: &Map<String, Value>) -> Vec<(&str, &str)> {
    options
        .get("env")
        .and_then(Value::as_object)
        .map(|m| {
            m.iter()
                .filter_map(|(k, v)| v.as_str().map(|s| (k.as_str(), s)))
                .collect()
        })
        .unwrap_or_default()
}
