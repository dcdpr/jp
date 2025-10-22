use crate::{Context, Error, Tool};

mod check;
mod expand;
mod test;

use check::cargo_check;
use expand::cargo_expand;
use test::cargo_test;

pub async fn run(ctx: Context, t: Tool) -> std::result::Result<String, Error> {
    match t.name.trim_start_matches("cargo_") {
        "check" => cargo_check(&ctx, t.opt("package")?).await,
        "expand" => cargo_expand(&ctx, t.req("item")?, t.opt("package")?).await,
        "test" => cargo_test(&ctx, t.opt("package")?, t.opt("testname")?).await,
        _ => Err(format!("Unknown tool '{}'", t.name).into()),
    }
}
