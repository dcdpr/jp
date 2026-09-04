use utils::suppress_matcher;

use crate::{
    Context, Tool,
    util::{OneOrMany, ToolResult},
};

mod create_file;
mod delete_file;
mod grep_files;
mod list_files;
mod modify_file;
mod move_file;
mod read_file;
pub(crate) mod utils;

use create_file::fs_create_file;
use delete_file::fs_delete_file;
use grep_files::fs_grep_files;
use list_files::fs_list_files;
use modify_file::fs_modify_file;
use move_file::fs_move_file;
use read_file::fs_read_file;

pub async fn run(ctx: Context, t: Tool) -> ToolResult {
    // Paths these tools may read but never return. Honored by the tools that exist
    // to hand file contents or paths back; the write tools return confirmations
    // rather than content, and what they may touch is the access policy's
    // question.
    //
    // Parsed strictly rather than through `option_or`: a disclosure control that
    // falls back to "suppress nothing" when its configuration is malformed hands
    // over the very paths it was told to hold back, and does it silently.
    let patterns: Vec<String> = match t.options.get("suppress") {
        None => vec![],
        Some(value) => serde_json::from_value(value.clone())
            .map_err(|error| format!("Invalid `suppress` option for tool '{}': {error}", t.name))?,
    };
    let suppress = suppress_matcher(&ctx.root, &patterns)?;

    match t.name.trim_start_matches("fs_") {
        "list_files" => fs_list_files(
            &ctx.root,
            ctx.access.as_ref(),
            t.opt("prefixes")?,
            t.opt("extensions")?,
            &suppress,
        )
        .await
        .map(|files| files.render())
        .map(Into::into),

        "read_file" => {
            fs_read_file(
                &ctx,
                &suppress,
                t.req("path")?,
                t.opt("start_line")?,
                t.opt("end_line")?,
            )
            .await
        }

        "grep_files" => fs_grep_files(
            &ctx.root,
            ctx.access.as_ref(),
            t.req("pattern")?,
            t.opt("context")?,
            t.opt("paths")?,
            None,
            &suppress,
        )
        .await
        .map(Into::into),

        // Scope to the docs tree and restrict to markdown sources. The
        // `.ignore` whitelist already prunes the rendered `.vitepress/dist`
        // and `cache` output; the extension filter additionally drops the
        // vitepress build config and theme (`.mts`, `.vue`, `.css`, ...),
        // leaving only the documentation prose.
        "grep_user_docs" => fs_grep_files(
            &ctx.root,
            ctx.access.as_ref(),
            t.req("pattern")?,
            t.opt("context")?,
            Some(vec!["docs".to_owned()].into()),
            Some(vec!["md".to_owned()].into()),
            &suppress,
        )
        .await
        .map(Into::into),

        "create_file" => fs_create_file(ctx, &t.answers, t.req("path")?, t.opt("content")?).await,

        "delete_file" => {
            fs_delete_file(&ctx.root, ctx.access.as_ref(), &t.answers, t.req("path")?).await
        }

        "move_file" => {
            fs_move_file(
                &ctx.root,
                ctx.access.as_ref(),
                &t.answers,
                t.req("source")?,
                t.req("target")?,
            )
            .await
        }

        "modify_file" => {
            fs_modify_file(
                ctx,
                &t.answers,
                &t.options,
                t.opt("path")?,
                t.req::<OneOrMany<_>>("patterns")?.into_vec(),
                t.opt("replace_using_regex")?.unwrap_or(false),
                t.opt("replace_all")?.unwrap_or(true),
                t.opt("case_sensitive")?.unwrap_or(true),
            )
            .await
        }

        _ => Err(format!("Unknown tool '{}'", t.name).into()),
    }
}

#[cfg(test)]
#[path = "fs_tests.rs"]
mod tests;
