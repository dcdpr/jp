use crate::{
    Context, Tool,
    util::{ToolResult, unknown_tool},
};

mod utils;

use utils::unix_utils;

#[expect(
    clippy::needless_pass_by_value,
    reason = "consistent with other module run fns"
)]
pub fn run(ctx: Context, t: Tool) -> ToolResult {
    match t.name.trim_start_matches("unix_") {
        "utils" => {
            let stdin: Option<String> = t.opt("stdin")?;
            unix_utils(
                &ctx,
                &t.req::<String>("util")?,
                t.opt("args")?,
                stdin.as_deref(),
            )
        }
        _ => unknown_tool(t),
    }
}
