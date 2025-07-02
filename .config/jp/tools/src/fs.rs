use crate::{to_xml, Error, Tool, Workspace};

mod grep_files;
mod list_files;

use grep_files::fs_grep_files;
use list_files::fs_list_files;

pub async fn run(ws: Workspace, t: Tool) -> std::result::Result<String, Error> {
    match t.name.trim_start_matches("fs_") {
        "list_files" => fs_list_files(ws.path, t.opt("prefixes")?, t.opt("extensions")?)
            .await
            .and_then(to_xml),

        "grep_files" => {
            fs_grep_files(
                ws.path,
                t.req("pattern")?,
                t.opt("paths")?,
                t.opt("return_entire_file")?,
            )
            .await
        }

        _ => Err(format!("Unknown tool '{}'", t.name).into()),
    }
}
