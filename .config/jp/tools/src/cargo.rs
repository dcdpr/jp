use crate::{Context, Tool, util::ToolResult};

mod check;
mod expand;
mod test;

use check::cargo_check;
use expand::cargo_expand;
use test::cargo_test;

pub async fn run(ctx: Context, t: Tool) -> ToolResult {
    match t.name.trim_start_matches("cargo_") {
        "check" => cargo_check(&ctx, t.opt("package")?).await,
        "expand" => cargo_expand(&ctx, t.req("item")?, t.opt("package")?)
            .await
            .map(Into::into),
        "test" => cargo_test(&ctx, t.opt("package")?, t.opt("testname")?)
            .await
            .map(Into::into),
        _ => Err(format!("Unknown tool '{}'", t.name).into()),
    }
}
