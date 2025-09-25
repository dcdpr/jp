use crate::{to_xml, Error, Tool, Workspace};

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

pub async fn run(ws: Workspace, t: Tool) -> std::result::Result<String, Error> {
    match t.name.trim_start_matches("fs_") {
        "list_files" => fs_list_files(ws.path, t.opt("prefixes")?, t.opt("extensions")?)
            .await
            .and_then(to_xml),

        "read_file" => fs_read_file(ws.path, t.req("path")?).await,

        "grep_files" => {
            fs_grep_files(
                ws.path,
                t.req("pattern")?,
                t.opt("context")?,
                t.opt("paths")?,
            )
            .await
        }

        "grep_user_docs" => {
            fs_grep_files(
                ws.path,
                t.req("pattern")?,
                t.opt("context")?,
                Some(vec!["docs".to_owned()]),
            )
            .await
        }

        "create_file" => fs_create_file(ws.path, t.req("path")?, t.opt("contents")?).await,

        "delete_file" => fs_delete_file(ws.path, t.req("path")?).await,

        "modify_file" => {
            fs_modify_file(
                ws.path,
                t.req("path")?,
                t.req("string_to_replace")?,
                t.opt("new_string")?,
            )
            .await
        }

        _ => Err(format!("Unknown tool '{}'", t.name).into()),
    }
}
