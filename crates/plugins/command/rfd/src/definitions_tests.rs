use std::fs;

use camino::Utf8PathBuf;
use camino_tempfile::Utf8TempDir;
use serde_json::json;
use tool_definitions::SYNCED_FILE;

use super::*;
use crate::{tests::jp_root, tool::TOOLS};

fn categories() -> Vec<String> {
    vec!["design".to_owned(), "guide".to_owned()]
}

/// JP's own definitions of the RFD tools.
fn jp_definitions() -> Utf8PathBuf {
    jp_root().join(".jp/mcp/tools/rfd")
}

/// Every file in `dir`, parsed, by file name.
fn read_definitions(dir: &Utf8Path) -> Vec<(String, toml::Value)> {
    let mut files: Vec<(String, toml::Value)> = fs::read_dir(dir)
        .unwrap_or_else(|error| panic!("{dir}: {error}"))
        .map(|entry| {
            let path = Utf8PathBuf::try_from(entry.unwrap().path()).unwrap();
            let source = fs::read_to_string(&path).unwrap();
            let value = toml::from_str(&source).unwrap_or_else(|error| panic!("{path}: {error}"));

            (path.file_name().unwrap().to_owned(), value)
        })
        .collect();
    files.sort_by(|a, b| a.0.cmp(&b.0));

    files
}

/// The embedded definitions, parsed, by file name.
fn templates() -> Vec<(String, toml::Value)> {
    DEFINITIONS
        .files
        .iter()
        .map(|(name, source)| {
            let value = toml::from_str(source).unwrap_or_else(|error| panic!("{name}: {error}"));
            ((*name).to_owned(), value)
        })
        .collect()
}

/// The tools a set of definitions routes to this plugin, each as the name the
/// plugin knows it by, its key, and its definition.
///
/// Tools served by anything else, and definitions carrying no `source`, such as
/// the synced file's, are skipped.
fn routed(files: &[(String, toml::Value)]) -> Vec<(String, String, toml::Value)> {
    let mut routed = vec![];
    for (file, value) in files {
        let Some(tools) = value
            .get("conversation")
            .and_then(|conversation| conversation.get("tools"))
            .and_then(toml::Value::as_table)
        else {
            panic!("{file} declares no tools");
        };

        for (key, tool) in tools {
            let Some(name) = tool
                .get("source")
                .and_then(toml::Value::as_str)
                .and_then(|source| source.strip_prefix("plugin.command.rfd."))
            else {
                continue;
            };

            routed.push((name.to_owned(), key.clone(), tool.clone()));
        }
    }
    routed.sort_by(|a, b| a.0.cmp(&b.0));

    routed
}

/// Only `show` batches several calls in one: the other tools change an RFD's
/// lifecycle or relations, each a decision made one at a time, and `list`
/// filters already.
/// JP's own definitions say the same as the ones `tools init` writes.
#[test]
fn only_show_batches() {
    for files in [templates(), read_definitions(&jp_definitions())] {
        let batching: Vec<(String, toml::Value)> = routed(&files)
            .into_iter()
            .filter_map(|(name, _, tool)| Some((name, tool.get("fan_out")?.clone())))
            .collect();

        assert_eq!(batching, vec![(
            "show".to_owned(),
            toml::Value::Boolean(true)
        )]);
    }
}

/// Every tool that takes a category declares the parameter `sync` writes the
/// accepted values into.
fn assert_category_tools_declare_the_parameter(routed: &[(String, String, toml::Value)]) {
    for (name, key, tool) in routed {
        for mirror in mirrors(name, &[]) {
            assert_eq!(
                tool["parameters"][mirror.parameter]["type"].as_str(),
                Some("string"),
                "{key} takes `{}` but doesn't declare it",
                mirror.parameter
            );
        }
    }
}

/// A definition declaring the `category` parameter, served as `source`.
fn categorized(source: &str) -> serde_json::Value {
    json!({ "source": source, "parameters": { "category": { "type": "string" } } })
}

/// `draft` and `list` take a category, and nothing else does.
#[test]
fn sync_offers_the_categories_to_draft_and_list() {
    let dir = Utf8TempDir::new().unwrap();
    let config = json!({
        "conversation": {
            "tools": {
                "rfd_list": categorized("plugin.command.rfd.list"),
                "new_doc": categorized("plugin.command.rfd.draft"),
                "rfd_show": categorized("plugin.command.rfd.show"),
            }
        }
    });

    let output = sync(dir.path(), &config, &categories(), false).unwrap();

    let path = dir.path().join(SYNCED_FILE);
    assert_eq!(
        output.text,
        format!("Wrote {path}, with categories for new_doc, rfd_list\n")
    );
    let lists = r#"
[conversation.tools.new_doc.parameters.category]
enum = [
  "design",
  "guide",
]

[conversation.tools.rfd_list.parameters.category]
enum = [
  "design",
  "guide",
]
"#;
    assert_eq!(
        fs::read_to_string(&path).unwrap(),
        format!("{}\n{SYNCED_NOTES}{lists}", DEFINITIONS.header())
    );
}

/// JP's definitions declare the parameter in every tool `sync` writes
/// categories into, so none of them is left out.
#[test]
fn jp_definitions_declare_the_category_parameter() {
    assert_category_tools_declare_the_parameter(&routed(&read_definitions(&jp_definitions())));
}

/// Each embedded definition is named for the tool it routes, and every tool the
/// plugin serves has one.
#[test]
fn every_served_tool_has_a_starting_definition() {
    let templates = templates();
    for (file, value) in &templates {
        let routed = routed(&[(file.clone(), value.clone())]);
        let [(name, ..)] = routed.as_slice() else {
            panic!("{file} should route exactly one tool, routes {routed:?}");
        };
        assert_eq!(format!("{name}.toml"), *file);
    }

    let names: Vec<String> = routed(&templates)
        .into_iter()
        .map(|(name, ..)| name)
        .collect();
    assert_eq!(names, TOOLS);
    assert_category_tools_declare_the_parameter(&routed(&templates));
}

/// JP routes every tool this plugin serves but `promote`, and nothing else: a
/// definition naming a tool the plugin doesn't serve is an error at
/// conversation time.
/// `rfd_promote` runs JP's `just rfd-promote`, which calls `jp rfd promote` and
/// then archives the conversations JP's own workflow keeps for a draft.
#[test]
fn jp_routes_every_tool_this_plugin_serves_but_promote() {
    let routed = routed(&read_definitions(&jp_definitions()));
    let names: Vec<&str> = routed.iter().map(|(name, ..)| name.as_str()).collect();

    assert_eq!(names, [
        "abandon",
        "draft",
        "extend",
        "list",
        "rename",
        "renumber",
        "require",
        "show",
        "supersede",
    ]);
    assert!(names.iter().all(|name| TOOLS.contains(name)), "{names:?}");
}
