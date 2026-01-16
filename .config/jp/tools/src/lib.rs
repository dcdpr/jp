#![expect(clippy::too_many_arguments)]
#![allow(clippy::print_stdout, clippy::print_stderr)]

mod cargo;
mod fs;
mod git;
mod github;
mod util;
mod web;

use jp_tool::{Context, Outcome};
use serde::Serialize;
use serde_json::{Map, Value};

type Error = Box<dyn std::error::Error + Send + Sync + 'static>;
type Result<T> = std::result::Result<T, Error>;

pub async fn run(ctx: Context, t: Tool) -> Result<Outcome> {
    match t.name.as_str() {
        s if s.starts_with("cargo_") => cargo::run(ctx, t).await,
        s if s.starts_with("github_") => github::run(ctx, t).await.map(Into::into),
        s if s.starts_with("fs_") => fs::run(ctx, t).await,
        s if s.starts_with("web_") => web::run(ctx, t).await.map(Into::into),
        s if s.starts_with("git_") => git::run(ctx, t).await,
        _ => Err(format!("Unknown tool '{}'", t.name).into()),
    }
}

#[derive(Debug, serde::Deserialize)]
pub struct Tool {
    pub name: String,
    pub arguments: Map<String, Value>,
    #[serde(default)]
    pub answers: Map<String, Value>,
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

    fn opt_or_empty<T: serde::de::DeserializeOwned>(&self, key: &str) -> Result<Option<T>> {
        match self.opt(key) {
            opt @ Ok(_) => opt,
            err @ Err(_) => match self.req::<String>(key) {
                Ok(v) if v.is_empty() => Ok(None),
                _ => err,
            },
        }
    }
}

fn to_xml<T: serde::Serialize>(value: T) -> Result<String> {
    to_xml_with_root(&value, "").or_else(|_| to_xml_with_root(&value, "result"))
}

fn to_xml_with_root<T: serde::Serialize>(value: &T, root: &str) -> Result<String> {
    let root = if root.is_empty() { None } else { Some(root) };
    let mut buffer = String::new();
    let mut serializer = quick_xml::se::Serializer::with_root(&mut buffer, root)?;
    serializer.indent(' ', 2);
    value
        .serialize(serializer)
        .map_err(|e| format!("Unable to serialize XML: {e}"))?;

    Ok(format!("```xml\n{buffer}\n```"))
}

/// Serializes a struct into an LLM-friendly XML format.
pub fn to_simple_xml<T: Serialize>(data: &T) -> Result<String> {
    util::xml::to_simple_xml_with_root(data, "root")
}

pub fn to_simple_xml_with_root<T: Serialize>(data: &T, root: &str) -> Result<String> {
    util::xml::to_simple_xml_with_root(data, root)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_to_xml_with_root() {
        #[derive(serde::Serialize)]
        struct Data {
            foo: String,
            baz: Vec<u64>,
        }

        let value = Data {
            foo: "bar".to_owned(),
            baz: vec![1, 2, 3],
        };

        assert_eq!(to_xml(value).unwrap(), indoc::indoc! {"
            ```xml
            <Data>
              <foo>bar</foo>
              <baz>1</baz>
              <baz>2</baz>
              <baz>3</baz>
            </Data>
            ```"});
    }

    #[test]
    fn test_to_xml_without_root() {
        let value = serde_json::json!({
            "foo": "bar",
            "baz": [1, 2, 3],
        });

        assert_eq!(to_xml(value).unwrap(), indoc::indoc! {"
            ```xml
            <result>
              <foo>bar</foo>
              <baz>1</baz>
              <baz>2</baz>
              <baz>3</baz>
            </result>
            ```"});
    }
}
