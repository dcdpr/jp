use std::{fmt, fmt::Write as _, path::PathBuf, str::FromStr};

use convert_case::ccase;
use rmcp::model::{Content, RawContent, ResourceContents};
use serde::Serialize;
use serde_json::Value;
use url::Url;

use crate::error::Error;

const INDENT_WIDTH: usize = 2;

/// Maximum size of the search results response in bytes.
///
/// If the response exceeds this size, it will be truncated to avoid overflowing
/// the client with excessive data.
/// The limit is arbitrary, as there is no limit defined by the protocol, but
/// the `Claude.app` client has shown issues handling larger responses.
pub(crate) const MAX_RESPONSE_SIZE_BYTES: usize = 256 * 1024; // 256KiB limit

#[derive(Debug, Clone, PartialEq)]
pub struct CrateUri {
    pub name: String,
    pub version: Option<String>,
    pub root: Option<PathRoot>,
    pub path: PathBuf,
    pub fragment: Option<String>,
}

impl CrateUri {
    pub(crate) fn versions(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            version: None,
            root: None,
            path: PathBuf::new(),
            fragment: None,
        }
    }

    pub(crate) fn metadata(name: impl Into<String>, version: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            version: Some(version.into()),
            root: None,
            path: PathBuf::new(),
            fragment: None,
        }
    }

    pub(crate) fn readme(name: impl Into<String>, version: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            version: Some(version.into()),
            root: Some(PathRoot::Readme),
            path: PathBuf::new(),
            fragment: None,
        }
    }

    pub(crate) fn src(name: impl Into<String>, version: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            version: Some(version.into()),
            root: Some(PathRoot::Src),
            path: PathBuf::new(),
            fragment: None,
        }
    }
}

impl From<&CrateUri> for Url {
    fn from(uri: &CrateUri) -> Self {
        let mut url = Url::parse(&format!("crate://{}", uri.name)).expect("valid base URL");

        {
            let mut path = url.path_segments_mut().expect("not cannot-be-a-base");

            if let Some(version) = &uri.version {
                path.push(version);
            }

            if let Some(root) = uri.root {
                path.push(root.as_str());
            }

            for segment in &uri.path {
                path.push(&segment.to_string_lossy());
            }
        }

        if let Some(fragment) = &uri.fragment {
            url.set_fragment(Some(fragment));
        }

        url
    }
}

impl fmt::Display for CrateUri {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        Url::from(self).fmt(f)
    }
}

impl From<CrateUri> for String {
    fn from(uri: CrateUri) -> Self {
        uri.to_string()
    }
}

impl TryFrom<&Url> for CrateUri {
    type Error = Error;

    fn try_from(uri: &Url) -> Result<Self, Self::Error> {
        let mut crate_uri = CrateUri {
            name: String::new(),
            version: None,
            root: None,
            path: PathBuf::new(),
            fragment: None,
        };

        if uri.scheme() != "crate" {
            return Err(Error::InvalidResourceUri(format!(
                "Invalid URI scheme: {:?}, expected `crate`",
                uri.scheme()
            )));
        }

        crate_uri.name = uri
            .host_str()
            .ok_or(Error::InvalidResourceUri(
                "Missing crate name in uri host".to_owned(),
            ))?
            .to_owned();

        let Some(mut segments) = uri.path_segments() else {
            return Ok(crate_uri);
        };

        crate_uri.version = segments.next().map(str::to_owned);
        crate_uri.root = segments.next().map(PathRoot::from_str).transpose()?;
        crate_uri.path = PathBuf::from(segments.collect::<Vec<_>>().join("/"));
        crate_uri.fragment = uri.fragment().map(ToOwned::to_owned);

        Ok(crate_uri)
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PathRoot {
    Readme,
    Items,
    Src,
}

impl PathRoot {
    fn as_str(self) -> &'static str {
        match self {
            PathRoot::Readme => "readme",
            PathRoot::Items => "items",
            PathRoot::Src => "src",
        }
    }
}

impl FromStr for PathRoot {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "readme" => Ok(PathRoot::Readme),
            "items" => Ok(PathRoot::Items),
            "src" => Ok(PathRoot::Src),
            _ => Err(Error::InvalidResourceUri(format!(
                "Unexpected path root: {s}, must be one of 'readme', 'items', or 'src'"
            ))),
        }
    }
}

/// Serialize any `Serialize` value into pretty-printed, LLM-friendly XML.
///
/// Strings containing `<` or `>` (Rust code, HTML) are wrapped in CDATA so the
/// LLM sees them verbatim instead of XML-escaped (`&lt;Vec&lt;Foo&gt;&gt;`).
/// Multi-line strings are split onto separate indented lines for readability.
/// `None`/missing fields are skipped.
pub(crate) fn format_xml<T: Serialize>(value: &T, root: &str) -> Result<String, Error> {
    let value = serde_json::to_value(value)?;

    let mut out = format!("<{root}>\n");
    match value {
        Value::Array(items) => {
            let tag = infer_array_tag_name::<T>();
            for item in items {
                write_xml_node(&mut out, &tag, &item, 1);
            }
        }
        Value::Object(map) => {
            for (key, val) in map {
                write_xml_node(&mut out, &key, &val, 1);
            }
        }
        _ => {
            write_indent(&mut out, 1);
            write_content(&mut out, &value);
            out.push('\n');
        }
    }
    let _ = write!(out, "</{root}>");
    Ok(out)
}

fn write_xml_node(out: &mut String, key: &str, value: &Value, depth: usize) {
    match value {
        // Skip nulls — matches `#[serde(skip_serializing_if = "Option::is_none")]`.
        Value::Null => {}
        Value::Array(items) => {
            // Flatten: each item is emitted at the parent's depth with the
            // parent's key. Result is `<key>...</key><key>...</key>`.
            for item in items {
                write_xml_node(out, key, item, depth);
            }
        }
        Value::Object(map) => {
            write_indent(out, depth);
            let _ = writeln!(out, "<{key}>");
            for (child_key, child_val) in map {
                write_xml_node(out, child_key, child_val, depth + 1);
            }
            write_indent(out, depth);
            let _ = writeln!(out, "</{key}>");
        }
        _ => {
            // Primitive leaf — inline `<key>value</key>`, except multi-line
            // strings which break to indented lines for readability.
            write_indent(out, depth);
            let _ = write!(out, "<{key}>");
            if let Some(s) = value.as_str()
                && s.contains('\n')
            {
                out.push('\n');
                for line in s.lines() {
                    write_indent(out, depth + 1);
                    let _ = writeln!(out, "{line}");
                }
                write_indent(out, depth);
            } else {
                write_content(out, value);
            }
            let _ = writeln!(out, "</{key}>");
        }
    }
}

fn write_indent(out: &mut String, depth: usize) {
    for _ in 0..(depth * INDENT_WIDTH) {
        out.push(' ');
    }
}

/// Write a primitive value's raw content, wrapping in CDATA if it contains XML
/// metacharacters so the LLM sees Rust code and HTML verbatim.
fn write_content(out: &mut String, value: &Value) {
    match value {
        Value::String(s) => {
            if s.contains('<') || s.contains('>') {
                let _ = write!(out, "<![CDATA[{s}]]>");
            } else {
                out.push_str(s);
            }
        }
        Value::Bool(b) => {
            let _ = write!(out, "{b}");
        }
        Value::Number(n) => {
            let _ = write!(out, "{n}");
        }
        Value::Null | Value::Array(_) | Value::Object(_) => {}
    }
}

/// Guess a child-element tag for a top-level array, based on the Rust type of
/// the array element.
/// `Vec<Url>` → `url`, `Vec<CrateInfo>` → `crate_info`.
fn infer_array_tag_name<T: ?Sized>() -> String {
    let type_name = std::any::type_name::<T>();
    let inner = type_name
        .find('<')
        .zip(type_name.rfind('>'))
        .map_or(type_name, |(start, end)| &type_name[start + 1..end]);
    let tag = inner.rsplit("::").next().unwrap_or("item");
    match tag.to_lowercase().as_str() {
        "string" | "str" | "i32" | "i64" | "u32" | "u64" | "f64" | "bool" => "item".to_owned(),
        _ => ccase!(snake, tag),
    }
}

/// Cap total response size by trimming embedded-resource entries from the tail.
///
/// Plain-text entries are not counted because they're small fixed messages.
pub(crate) fn truncate_resources(mut content: Vec<Content>) -> Vec<Content> {
    let total = content.len();
    let mut bytes = content
        .iter()
        .fold(0, |acc, c| acc + resource_text_len(&c.raw));

    while bytes > MAX_RESPONSE_SIZE_BYTES {
        if content.len() == 1 {
            break;
        }
        let Some(last) = content.pop() else { break };

        bytes -= resource_text_len(&last.raw);
    }

    if content.len() != total {
        content.push(Content::text(indoc::formatdoc! {"
            NOTE: Query returned {total} matches, \
            but only showing {len} to stay within size limits.

            Please refine your query for more specific results.",
            len = content.len()
        }));
    }

    content
}

fn resource_text_len(raw: &RawContent) -> usize {
    match raw {
        RawContent::Resource(r) => match &r.resource {
            ResourceContents::TextResourceContents { text, .. } => text.len(),
            ResourceContents::BlobResourceContents { blob, .. } => blob.len(),
        },
        _ => 0,
    }
}

/// Validate a crate version string.
/// Accepts `latest` or a semver-compatible version (including partial versions
/// like `1` or `1.2`).
pub(crate) fn valid_crate_version(s: &str) -> bool {
    s == "latest" || semver::Version::parse(s).is_ok() || semver::VersionReq::parse(s).is_ok()
}

#[cfg(test)]
#[path = "tools_tests.rs"]
mod tests;
