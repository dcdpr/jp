use crate::{
    Context, Tool,
    util::{ToolResult, unknown_tool},
};

mod fetch;

use fetch::web_fetch;

pub async fn run(_: Context, t: Tool) -> ToolResult {
    match t.name.trim_start_matches("web_") {
        "fetch" => {
            web_fetch(
                t.req("url")?,
                t.opt("list_sections")?.unwrap_or(false),
                t.opt("sections")?,
            )
            .await
        }
        _ => unknown_tool(t),
    }
}
