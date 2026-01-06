use crate::{Context, Error, Outcome, Tool, to_xml};

mod create_file;
mod delete_file;
mod grep_files;
mod list_files;
mod modify_file;
mod read_file;
mod utils;

use create_file::fs_create_file;
use delete_file::fs_delete_file;
use grep_files::fs_grep_files;
use list_files::fs_list_files;
use modify_file::fs_modify_file;
use read_file::fs_read_file;

pub async fn run(ctx: Context, t: Tool) -> std::result::Result<Outcome, Error> {
    match t.name.trim_start_matches("fs_") {
        "list_files" => fs_list_files(ctx.root, t.opt("prefixes")?, t.opt("extensions")?)
            .await
            .and_then(to_xml)
            .map(Into::into),

        "read_file" => {
            fs_read_file(
                ctx.root,
                t.req("path")?,
                t.opt("start_line")?,
                t.opt("end_line")?,
            )
            .await
        }

        "grep_files" => fs_grep_files(
            ctx.root,
            t.req("pattern")?,
            t.opt("context")?,
            t.opt("paths")?,
        )
        .await
        .map(Into::into),

        "grep_user_docs" => fs_grep_files(
            ctx.root,
            t.req("pattern")?,
            t.opt("context")?,
            Some(vec!["docs".to_owned()].into()),
        )
        .await
        .map(Into::into),

        "create_file" => fs_create_file(ctx, &t.answers, t.req("path")?, t.opt("content")?).await,

        "delete_file" => fs_delete_file(ctx.root, &t.answers, t.req("path")?).await,

        "modify_file" => {
            fs_modify_file(
                ctx,
                &t.answers,
                t.req("path")?,
                t.req("string_to_replace")?,
                t.req("new_string")?,
                t.req("replace_using_regex")?,
            )
            .await
        }

        _ => Err(format!("Unknown tool '{}'", t.name).into()),
    }
}
