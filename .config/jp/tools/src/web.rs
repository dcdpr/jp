use crate::{Context, Error, Tool};

mod fetch;

use fetch::web_fetch;

pub async fn run(_: Context, t: Tool) -> std::result::Result<String, Error> {
    match t.name.trim_start_matches("web_") {
        "fetch" => web_fetch(t.req("url")?).await,

        _ => Err(format!("Unknown tool '{}'", t.name).into()),
    }
}
