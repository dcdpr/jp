use crate::{to_xml, Error, Tool, Workspace};

mod grep_files;
mod list_files;
mod read_file;

use grep_files::fs_grep_files;
use list_files::fs_list_files;
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

        _ => Err(format!("Unknown tool '{}'", t.name).into()),
    }
}
