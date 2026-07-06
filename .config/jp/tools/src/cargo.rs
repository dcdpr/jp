use crate::{
    Context, Tool,
    util::{ToolResult, unknown_tool},
};

mod check;
mod expand;
mod format;
mod test;

use check::cargo_check;
use expand::cargo_expand;
use format::cargo_format;
use test::cargo_test;

pub async fn run(ctx: Context, t: Tool) -> ToolResult {
    // Opt-in to cargo's checksum-based freshness checks (see
    // rust-lang/cargo#14136). Requires nightly cargo, so it defaults to off
    // and is enabled per-tool via `options.checksum_freshness` in the tool
    // config.
    let checksum_freshness = t.option_or("checksum_freshness", false);

    match t.name.trim_start_matches("cargo_") {
        "check" => cargo_check(&ctx, t.opt("package")?, checksum_freshness).await,
        "expand" => cargo_expand(&ctx, t.req("item")?, t.opt("package")?, checksum_freshness).await,
        "test" => {
            cargo_test(
                &ctx,
                t.opt("package")?,
                t.opt("testname")?,
                t.opt("backtrace")?,
                checksum_freshness,
            )
            .await
        }
        "format" => cargo_format(&ctx, t.opt("package")?).await,
        _ => unknown_tool(t),
    }
}
