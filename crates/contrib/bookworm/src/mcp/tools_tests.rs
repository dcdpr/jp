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
fn test_try_from_url_parses_valid_uris() {
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

    assert_cases(test_cases);
}

#[test]
fn test_try_from_url_rejects_invalid_uris() {
    let mut test_cases: HashMap<&'static str, TestCase> = HashMap::new();

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
            "Unexpected path root: invalid, must be one of 'readme', 'items', or 'src'".to_owned(),
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

    assert_cases(test_cases);
}

/// Parse each case's URI and compare the result with what it expects.
fn assert_cases(test_cases: HashMap<&'static str, TestCase>) {
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
                panic!("Case '{name}': expected error {expected_error:?}, got success: {actual:?}");
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
