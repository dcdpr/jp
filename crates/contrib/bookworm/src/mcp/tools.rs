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
/// the client with excessive data. The limit is arbitrary, as there is no limit
/// defined by the protocol, but the `Claude.app` client has shown issues
/// handling larger responses.
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
/// Strings containing `<` or `>` (Rust code, HTML) are wrapped in CDATA so
/// the LLM sees them verbatim instead of XML-escaped (`&lt;Vec&lt;Foo&gt;&gt;`).
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

/// Write a primitive value's raw content, wrapping in CDATA if it contains
/// XML metacharacters so the LLM sees Rust code and HTML verbatim.
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

/// Guess a child-element tag for a top-level array, based on the Rust type
/// of the array element. `Vec<Url>` → `url`, `Vec<CrateInfo>` → `crate_info`.
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

/// Validate a crate version string. Accepts `latest` or a semver-compatible
/// version (including partial versions like `1` or `1.2`).
pub(crate) fn valid_crate_version(s: &str) -> bool {
    s == "latest" || semver::Version::parse(s).is_ok() || semver::VersionReq::parse(s).is_ok()
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use serde::Serialize;

    use super::*;

    // --- format_xml ---

    #[derive(Serialize)]
    struct Sample {
        path: String,
        kind: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        type_info: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        documentation: Option<String>,
    }

    #[test]
    fn format_xml_wraps_type_info_with_angle_brackets_in_cdata() {
        let sample = Sample {
            path: "serde_json::value::Value::pointer".into(),
            kind: "Method".into(),
            type_info: Some("pub fn pointer(&self, p: &str) -> Option<&Value>".into()),
            documentation: None,
        };
        let xml = format_xml(&sample, "Item").unwrap();

        // The signature contains `<` and `>` — must NOT be escaped.
        assert!(xml.contains("Option<&Value>"), "got:\n{xml}");
        assert!(!xml.contains("&lt;"), "angle brackets escaped:\n{xml}");
        assert!(xml.contains("<![CDATA["), "missing CDATA wrapper:\n{xml}");
        assert!(xml.contains("]]>"), "missing CDATA terminator:\n{xml}");
    }

    #[test]
    fn format_xml_omits_none_fields() {
        let sample = Sample {
            path: "foo".into(),
            kind: "Struct".into(),
            type_info: None,
            documentation: None,
        };
        let xml = format_xml(&sample, "Item").unwrap();

        assert!(!xml.contains("<type_info>"));
        assert!(!xml.contains("<documentation>"));
        assert!(xml.contains("<path>foo</path>"));
        assert!(xml.contains("<kind>Struct</kind>"));
    }

    #[test]
    fn format_xml_breaks_multiline_strings_onto_indented_lines() {
        let sample = Sample {
            path: "Foo".into(),
            kind: "Enum".into(),
            type_info: Some("pub enum Foo<T> {\n    A(T),\n    B,\n}".into()),
            documentation: None,
        };
        let xml = format_xml(&sample, "Item").unwrap();

        // Each line of the signature appears on its own line; angle
        // brackets are preserved verbatim via CDATA.
        assert!(xml.contains("A(T),"), "missing per-line content:\n{xml}");
        assert!(xml.contains("Foo<T>"), "signature scrubbed:\n{xml}");
        assert!(!xml.contains("&lt;T&gt;"), "generic escaped:\n{xml}");
    }

    #[test]
    fn format_xml_top_level_vec_uses_inferred_element_tag() {
        let urls: Vec<String> = vec![
            "crate://serde_json/1.0.140/src/lib.rs".into(),
            "crate://serde_json/1.0.140/src/value.rs".into(),
        ];
        let xml = format_xml(&urls, "Resources").unwrap();

        assert!(xml.starts_with("<Resources>"), "got:\n{xml}");
        assert!(xml.ends_with("</Resources>"), "got:\n{xml}");
        // Vec<String> => element tag falls back to "item".
        assert!(xml.contains("<item>crate://serde_json/1.0.140/src/lib.rs</item>"));
    }

    #[test]
    fn format_xml_plain_text_is_not_wrapped_in_cdata() {
        let sample = Sample {
            path: "crate_name".into(),
            kind: "Module".into(),
            type_info: None,
            documentation: Some("Just a plain sentence.".into()),
        };
        let xml = format_xml(&sample, "Item").unwrap();

        assert!(!xml.contains("<![CDATA["));
        assert!(xml.contains("<documentation>Just a plain sentence.</documentation>"));
    }

    // --- CrateUri ---

    struct TestCase {
        uri: &'static str,
        expected: Result<ExpectedUri, Error>,
    }

    #[derive(Debug, Clone, PartialEq, Default)]
    struct ExpectedUri {
        name: &'static str,
        version: Option<&'static str>,
        root: Option<PathRoot>,
        path: &'static str,
        fragment: Option<&'static str>,
    }

    impl From<ExpectedUri> for CrateUri {
        fn from(expected: ExpectedUri) -> Self {
            CrateUri {
                name: expected.name.to_owned(),
                version: expected.version.map(ToOwned::to_owned),
                root: expected.root,
                path: PathBuf::from(expected.path),
                fragment: expected.fragment.map(ToOwned::to_owned),
            }
        }
    }

    #[test]
    #[expect(clippy::too_many_lines, reason = "table-driven test cases")]
    fn test_try_from_url() {
        let mut test_cases: HashMap<&'static str, TestCase> = HashMap::new();

        test_cases.insert("complete uri with fragment", TestCase {
            uri: "crate://serde_json/1.0.0/src/value/mod.rs#L30",
            expected: Ok(ExpectedUri {
                name: "serde_json",
                version: Some("1.0.0"),
                root: Some(PathRoot::Src),
                path: "value/mod.rs",
                fragment: Some("L30"),
            }),
        });

        test_cases.insert("complete uri without fragment", TestCase {
            uri: "crate://tokio/1.2.3/items/io/struct.AsyncReadExt.html",
            expected: Ok(ExpectedUri {
                name: "tokio",
                version: Some("1.2.3"),
                root: Some(PathRoot::Items),
                path: "io/struct.AsyncReadExt.html",
                fragment: None,
            }),
        });

        test_cases.insert("readme with empty path", TestCase {
            uri: "crate://regex/latest/readme",
            expected: Ok(ExpectedUri {
                name: "regex",
                version: Some("latest"),
                root: Some(PathRoot::Readme),
                path: "",
                fragment: None,
            }),
        });

        test_cases.insert("items with method reference fragment", TestCase {
            uri: "crate://diesel/2.0.0/items/query_dsl/trait.FilterDsl.html#method.filter",
            expected: Ok(ExpectedUri {
                name: "diesel",
                version: Some("2.0.0"),
                root: Some(PathRoot::Items),
                path: "query_dsl/trait.FilterDsl.html",
                fragment: Some("method.filter"),
            }),
        });

        test_cases.insert("src with complex nested path", TestCase {
            uri: "crate://log/0.4.17/src/log/macros.rs",
            expected: Ok(ExpectedUri {
                name: "log",
                version: Some("0.4.17"),
                root: Some(PathRoot::Src),
                path: "log/macros.rs",
                fragment: None,
            }),
        });

        test_cases.insert("hyphenated crate name", TestCase {
            uri: "crate://proc-macro2/1.0.47/readme",
            expected: Ok(ExpectedUri {
                name: "proc-macro2",
                version: Some("1.0.47"),
                root: Some(PathRoot::Readme),
                path: "",
                fragment: None,
            }),
        });

        test_cases.insert("semver with pre-release tag", TestCase {
            uri: "crate://tokio/1.0.0-alpha.1/items/io/index.html",
            expected: Ok(ExpectedUri {
                name: "tokio",
                version: Some("1.0.0-alpha.1"),
                root: Some(PathRoot::Items),
                path: "io/index.html",
                fragment: None,
            }),
        });

        test_cases.insert("partial version", TestCase {
            uri: "crate://serde/1/items/index.html",
            expected: Ok(ExpectedUri {
                name: "serde",
                version: Some("1"),
                root: Some(PathRoot::Items),
                path: "index.html",
                fragment: None,
            }),
        });

        test_cases.insert("uri with version but no root", TestCase {
            uri: "crate://actix-web/4.0.0",
            expected: Ok(ExpectedUri {
                name: "actix-web",
                version: Some("4.0.0"),
                root: None,
                path: "",
                fragment: None,
            }),
        });

        test_cases.insert("uri with only crate name", TestCase {
            uri: "crate://clap",
            expected: Ok(ExpectedUri {
                name: "clap",
                version: None,
                root: None,
                path: "",
                fragment: None,
            }),
        });

        test_cases.insert("invalid scheme", TestCase {
            uri: "http://crates.io/serde_json",
            expected: Err(Error::InvalidResourceUri(
                "Invalid URI scheme: \"http\", expected `crate`".to_owned(),
            )),
        });

        test_cases.insert("missing host (crate name)", TestCase {
            uri: "crate:///1.0.0/src/value.rs",
            expected: Err(Error::InvalidResourceUri(
                "Missing crate name in uri host".to_owned(),
            )),
        });

        test_cases.insert("invalid root path", TestCase {
            uri: "crate://serde_json/1.0.0/invalid/value.rs",
            expected: Err(Error::InvalidResourceUri(
                "Unexpected path root: invalid, must be one of 'readme', 'items', or 'src'"
                    .to_owned(),
            )),
        });

        test_cases.insert("empty uri", TestCase {
            uri: "crate://",
            expected: Err(Error::InvalidResourceUri(
                "Missing crate name in uri host".to_owned(),
            )),
        });

        test_cases.insert("invalid path root", TestCase {
            uri: "crate://serde_json//",
            expected: Err(Error::InvalidResourceUri(
                "Unexpected path root: , must be one of 'readme', 'items', or 'src'".to_owned(),
            )),
        });

        for (name, test_case) in test_cases {
            let url = Url::parse(test_case.uri).expect("Failed to parse URL");
            let result = CrateUri::try_from(&url);

            match (result, test_case.expected) {
                (Ok(actual), Ok(expected)) => {
                    let expected_uri = CrateUri::from(expected);
                    assert_eq!(
                        actual.name, expected_uri.name,
                        "Case '{name}': name mismatch"
                    );
                    assert_eq!(
                        actual.version, expected_uri.version,
                        "Case '{name}': version mismatch"
                    );
                    assert_eq!(
                        actual.root, expected_uri.root,
                        "Case '{name}': root mismatch"
                    );
                    assert_eq!(
                        actual.path, expected_uri.path,
                        "Case '{name}': path mismatch"
                    );
                    assert_eq!(
                        actual.fragment, expected_uri.fragment,
                        "Case '{name}': fragment mismatch"
                    );
                }
                (Err(actual_error), Err(expected_error)) => {
                    assert_eq!(
                        format!("{actual_error:?}"),
                        format!("{expected_error:?}"),
                        "Case '{name}': error mismatch"
                    );
                }
                (Ok(actual), Err(expected_error)) => {
                    panic!(
                        "Case '{name}': expected error {expected_error:?}, got success: {actual:?}"
                    );
                }
                (Err(actual_error), Ok(expected)) => {
                    panic!(
                        "Case '{name}': expected success with {expected:?}, got error: \
                         {actual_error:?}"
                    );
                }
            }
        }
    }
}
