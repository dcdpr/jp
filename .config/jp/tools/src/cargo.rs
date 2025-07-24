use crate::{Error, Tool, Workspace};

pub(crate) mod check;
pub(crate) mod expand;
pub(crate) mod test;

use check::cargo_check;
use expand::cargo_expand;
use test::cargo_test;

pub async fn run(ws: Workspace, t: Tool) -> std::result::Result<String, Error> {
    match t.name.trim_start_matches("cargo_") {
        "check" => cargo_check(&ws, t.opt("package")?).await,
        "expand" => cargo_expand(&ws, t.req("item")?, t.opt("package")?).await,
        "test" => cargo_test(&ws, t.opt("package")?, t.opt("testname")?).await,
        _ => Err(format!("Unknown tool '{}'", t.name).into()),
    }
}
