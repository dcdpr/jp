use crate::{Error, Tool, Workspace};

pub(crate) mod test;

use test::cargo_test;

pub async fn run(ws: Workspace, t: Tool) -> std::result::Result<String, Error> {
    match t.name.trim_start_matches("cargo_") {
        "test" => cargo_test(&ws, t.opt("package")?, t.opt("testname")?).await,
        _ => Err(format!("Unknown tool '{}'", t.name).into()),
    }
}
