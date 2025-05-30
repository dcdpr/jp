mod cargo;
mod github;

use std::path::PathBuf;

use cargo::test::cargo_test;
use github::{
    issues::github_issues,
    pulls::github_pulls,
    repo::{github_code_search, github_read_file},
};
use serde_json::{Map, Value};

#[derive(Debug, serde::Deserialize)]
pub struct Workspace {
    pub path: PathBuf,
}

#[derive(Debug, serde::Deserialize)]
pub struct Tool {
    pub name: String,
    pub arguments: Map<String, Value>,
}

impl Tool {
    fn req<T: serde::de::DeserializeOwned>(&self, key: &str) -> Result<T> {
        self.arguments
            .get(key)
            .cloned()
            .ok_or(format!("Missing argument '{key}' for tool '{}'", self.name))
            .and_then(|v| {
                serde_json::from_value(v).map_err(|error| {
                    format!(
                        "Unable to parse argument '{key}' for tool '{}': {error}",
                        self.name
                    )
                })
            })
            .map_err(Into::into)
    }

    fn opt<T: serde::de::DeserializeOwned>(&self, key: &str) -> Result<Option<T>> {
        if !self.arguments.contains_key(key) {
            return Ok(None);
        }

        self.req(key).map(Some)
    }
}

type Error = Box<dyn std::error::Error + Send + Sync + 'static>;
type Result<T> = std::result::Result<T, Error>;

pub async fn run(ws: Workspace, t: Tool) -> Result<String> {
    match t.name.as_str() {
        "cargo_test" => cargo_test(&ws, t.opt("package")?, t.opt("testname")?).await,
        "github_issues" => github_issues(t.opt("number")?).await,
        "github_pulls" => github_pulls(t.opt("number")?, t.opt("state")?).await,
        "github_code_search" => github_code_search(t.opt("repository")?, t.req("query")?).await,
        "github_read_file" => github_read_file(t.opt("repository")?, t.req("path")?).await,
        _ => todo!(),
    }
}

fn to_xml<T: serde::Serialize>(value: T) -> Result<String> {
    let mut buffer = String::new();
    let mut serializer = quick_xml::se::Serializer::new(&mut buffer);
    serializer.indent(' ', 2);
    value
        .serialize(serializer)
        .map_err(|e| format!("Unable to serialize XML: {e}"))?;

    Ok(format!("```xml\n{buffer}\n```"))
}
