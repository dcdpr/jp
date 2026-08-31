use jp_tool::Capability;
use serde_json::{Map, Value};

use crate::{
    Context, Tool,
    util::{
        ToolResult, error,
        root::{GIT_DIR, configured_root, note_root, resolve_root},
        unknown_tool,
    },
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
    let subcommand = t.name.trim_start_matches("git_");

    // Which repository to operate in. Defaults to the root the tool was invoked
    // with; set `options.root` in the tool config to point the git tooling at
    // another checkout in the workspace.
    let configured = match configured_root(opts) {
        Ok(configured) => configured,
        Err(message) => return error(message),
    };

    let root = match resolve_root(
        &ctx.root,
        configured,
        ctx.access.as_ref(),
        required_capabilities(subcommand),
        &GIT_DIR,
    ) {
        Ok(root) => root,
        Err(message) => return error(message),
    };

    let outcome = match subcommand {
        "add_intent" => git_add_intent(&root, t.req("paths")?, opts).await,

        "commit" => git_commit(&root, t.req("message")?, opts).await,

        "stage_patch" => {
            git_stage_patch(&root, &ctx.action, &t.answers, t.req("patches")?, opts).await
        }

        "stage_patch_lines" => {
            let path: String = t.req("path")?;
            let patch_id: String = t.req("patch_id")?;
            let lines: Vec<Value> = t.req("lines")?;
            git_stage_patch_lines(&root, &path, &patch_id, lines, opts)
        }

        "list_patches" => git_list_patches(&root, t.opt("files")?, opts),

        "unstage" => git_unstage(&root, t.req("paths")?, opts).await,

        "diff" => git_diff(&root, t.opt("paths")?, t.req("status")?, opts).await,

        "log" => {
            git_log(
                &root,
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

        "show" => git_show(&root, t.req("revision")?, opts).await,

        "status" => git_status(&root, opts).await,

        "blame" => {
            git_blame(
                &root,
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
                &root,
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
                &root,
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

        _ => return unknown_tool(t),
    };

    if root == ctx.root {
        return outcome;
    }

    note_root(outcome, &root, "git")
}

/// Capabilities a git subcommand needs on the repository it runs in.
///
/// These gate whether git is spawned at all; they cannot bound what it does
/// once running, because the subprocess is not sandboxed.
///
/// Reading commands are declared read-only even though git refreshes
/// `.git/index` as it goes.
/// Declaring that write would make every grant that allows reading history also
/// allow rewriting it, which is the larger of the two inaccuracies.
fn required_capabilities(subcommand: &str) -> &'static [Capability] {
    match subcommand {
        // Write the index, and create it in a repository that has none yet.
        "add_intent" | "stage_patch" | "stage_patch_lines" | "unstage" => {
            &[Capability::Read, Capability::Create, Capability::Update]
        }
        // Writes objects and refs, and runs hooks — arbitrary code carrying the
        // process's own filesystem access.
        "commit" => &[
            Capability::Read,
            Capability::Create,
            Capability::Update,
            Capability::Execute,
        ],
        _ => &[Capability::Read],
    }
}

/// Build the environment for a git subprocess.
///
/// Starts from the defaults every git invocation gets, then appends the pairs
/// in the `env` tool option, which lets a caller (an integration test injecting
/// `GIT_CONFIG_GLOBAL` to isolate git from host config) both add variables and
/// override a default: the runner applies the pairs in order, so a later entry
/// wins.
fn env_from_options(options: &Map<String, Value>) -> Vec<(&str, &str)> {
    // Reading commands refresh `.git/index` as they go, which takes a lock and
    // rewrites the file — a write from a command declared read-only. Dropping
    // the optional lock leaves the declaration honest. Commands that need a
    // lock to do their job at all, `commit` and `apply`, still take one.
    let mut env = vec![("GIT_OPTIONAL_LOCKS", "0")];

    env.extend(
        options
            .get("env")
            .and_then(Value::as_object)
            .into_iter()
            .flat_map(|m| {
                m.iter()
                    .filter_map(|(k, v)| v.as_str().map(|s| (k.as_str(), s)))
            }),
    );

    env
}

#[cfg(test)]
#[path = "git_tests.rs"]
mod tests;
