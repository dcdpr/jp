use std::fs;

use camino::{Utf8Path, Utf8PathBuf};
use changelog::Type;

use super::*;
use crate::{tests::jp_root, tool::TOOLS};

fn jp_definitions() -> Utf8PathBuf {
    jp_root().join(".jp/mcp/tools/release")
}

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

fn routed(files: &[(String, toml::Value)]) -> Vec<(String, toml::Value)> {
    let mut routed = vec![];
    for (file, value) in files {
        let tools = value
            .get("conversation")
            .and_then(|conversation| conversation.get("tools"))
            .and_then(toml::Value::as_table)
            .unwrap_or_else(|| panic!("{file} declares no tools"));

        for (key, tool) in tools {
            let source = tool
                .get("source")
                .and_then(toml::Value::as_str)
                .unwrap_or_else(|| panic!("{key} in {file} names no source"));
            let name = source
                .strip_prefix("plugin.command.release.")
                .unwrap_or_else(|| panic!("{key} in {file} is served by {source}"));

            routed.push((name.to_owned(), tool.clone()));
        }
    }
    routed.sort_by(|a, b| a.0.cmp(&b.0));

    routed
}

/// The types a definition offers are the seven a fragment takes, in order:
/// nothing offered that is refused, nothing accepted that is hidden.
fn assert_offers_the_seven_types(routed: &[(String, toml::Value)]) {
    let ids: Vec<&str> = Type::ALL.iter().map(|type_| type_.id()).collect();

    for (name, tool) in routed {
        let offered: Vec<&str> = tool["parameters"]["type"]["enum"]
            .as_array()
            .unwrap_or_else(|| panic!("`{name}` offers no types"))
            .iter()
            .map(|value| value.as_str().expect("a type id"))
            .collect();

        assert_eq!(offered, ids, "{name}");
    }
}

/// Each embedded definition is named for the tool it routes, and every tool the
/// plugin serves has one.
#[test]
fn every_served_tool_has_a_starting_definition() {
    let templates = templates();
    for (file, value) in &templates {
        let routed = routed(&[(file.clone(), value.clone())]);
        let [(name, _)] = routed.as_slice() else {
            panic!("{file} should route exactly one tool, routes {routed:?}");
        };
        assert_eq!(format!("{name}.toml"), *file);
    }

    let names: Vec<String> = routed(&templates)
        .into_iter()
        .map(|(name, _)| name)
        .collect();
    assert_eq!(names, TOOLS);
}

/// JP's own configuration routes every tool this plugin serves, and nothing
/// else: a definition naming a tool the plugin doesn't serve is an error at
/// conversation time.
#[test]
fn jp_routes_every_tool_this_plugin_serves() {
    let names: Vec<String> = routed(&read_definitions(&jp_definitions()))
        .into_iter()
        .map(|(name, _)| name)
        .collect();

    assert_eq!(names, TOOLS);
}

#[test]
fn the_definitions_offer_the_seven_types() {
    assert_offers_the_seven_types(&routed(&templates()));
    assert_offers_the_seven_types(&routed(&read_definitions(&jp_definitions())));
}
