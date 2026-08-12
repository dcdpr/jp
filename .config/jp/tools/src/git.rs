use serde_json::{Map, Value};

use crate::{
    Context, Tool,
    util::{ToolResult, unknown_tool},
};

mod add_intent;
mod apply;
mod blame;
mod commit;
mod diff;
mod diff_commit;
mod diff_file;
mod diff_filter;
mod hunk;
mod list_patches;
mod log;
mod show;
mod stage_patch;
mod stage_patch_lines;
mod status;
mod unstage;

use add_intent::git_add_intent;
use blame::git_blame;
use commit::git_commit;
use diff::git_diff;
use diff_commit::git_diff_commit;
use diff_file::git_diff_file;
use list_patches::git_list_patches;
use log::git_log;
use show::git_show;
use stage_patch::git_stage_patch;
use stage_patch_lines::git_stage_patch_lines;
use status::git_status;
use unstage::git_unstage;

pub async fn run(ctx: Context, t: Tool) -> ToolResult {
    let opts = &t.options;

    match t.name.trim_start_matches("git_") {
        "add_intent" => git_add_intent(&ctx.root, t.req("paths")?, opts).await,

        "commit" => git_commit(ctx.root, t.req("message")?, opts).await,

        "stage_patch" => git_stage_patch(ctx, &t.answers, t.req("patches")?, opts).await,

        "stage_patch_lines" => {
            let path: String = t.req("path")?;
            let patch_id: String = t.req("patch_id")?;
            let lines: Vec<Value> = t.req("lines")?;
            git_stage_patch_lines(&ctx.root, &path, &patch_id, lines, opts)
        }

        "list_patches" => git_list_patches(&ctx.root, t.opt("files")?, opts),

        "unstage" => git_unstage(&ctx.root, t.req("paths")?, opts).await,

        "diff" => git_diff(ctx.root, t.opt("paths")?, t.req("status")?, opts).await,

        "log" => {
            git_log(
                ctx.root,
                t.opt("query")?,
                t.opt("content")?,
                t.opt("content_regex")?,
                t.opt("paths")?,
                t.opt("count")?,
                t.opt("since")?,
                opts,
            )
            .await
        }

        "show" => git_show(ctx.root, t.req("revision")?, opts).await,

        "status" => git_status(ctx.root, opts).await,

        "blame" => {
            git_blame(
                ctx.root,
                t.req("path")?,
                t.req("start_line")?,
                t.req("end_line")?,
                t.opt("revision")?,
                t.opt("ignore_whitespace")?,
                opts,
            )
            .await
        }

        "diff_commit" => {
            git_diff_commit(
                ctx.root,
                t.req("revision")?,
                t.req("paths")?,
                t.opt("pattern")?,
                t.opt("context")?,
                t.opt("start_line")?,
                t.opt("end_line")?,
                opts,
            )
            .await
        }

        "diff_file" => {
            git_diff_file(
                ctx.root,
                t.req("status")?,
                t.req("paths")?,
                t.opt("pattern")?,
                t.opt("context")?,
                t.opt("start_line")?,
                t.opt("end_line")?,
                opts,
            )
            .await
        }

        _ => unknown_tool(t),
    }
}

/// Extract environment variables from the `env` tool option.
///
/// Returns a list of (key, value) pairs that should be passed to git
/// subprocesses.
/// This allows callers (e.g. integration tests) to inject env vars like
/// `GIT_CONFIG_GLOBAL` to isolate git from host config.
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
