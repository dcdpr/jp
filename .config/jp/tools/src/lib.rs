#![allow(clippy::too_many_arguments)]

mod cargo;
mod fs;
mod github;

use std::path::PathBuf;

use serde_json::{Map, Value};

type Error = Box<dyn std::error::Error + Send + Sync + 'static>;
type Result<T> = std::result::Result<T, Error>;

pub async fn run(ws: Workspace, t: Tool) -> Result<String> {
    match t.name.as_str() {
        s if s.starts_with("cargo_") => cargo::run(ws, t).await,
        s if s.starts_with("github_") => github::run(ws, t).await,
        s if s.starts_with("fs_") => fs::run(ws, t).await,
        _ => Err(format!("Unknown tool '{}'", t.name).into()),
    }
}

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

fn to_xml<T: serde::Serialize>(value: T) -> Result<String> {
    to_xml_with_root(value, "")
}

fn to_xml_with_root<T: serde::Serialize>(value: T, root: &str) -> Result<String> {
    let root = if root.is_empty() { None } else { Some(root) };
    let mut buffer = String::new();
    let mut serializer = quick_xml::se::Serializer::with_root(&mut buffer, root)?;
    serializer.indent(' ', 2);
    value
        .serialize(serializer)
        .map_err(|e| format!("Unable to serialize XML: {e}"))?;

    Ok(format!("```xml\n{buffer}\n```"))
}
