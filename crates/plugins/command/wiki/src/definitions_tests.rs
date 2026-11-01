use std::{env, fs};

use camino::Utf8PathBuf;
use camino_tempfile::Utf8TempDir;
use serde_json::{Map, json};
use tool_definitions::{SYNCED_FILE, Values};

use super::*;
use crate::tool::TOOLS;

/// JP's own definitions of the tools this plugin serves.
///
/// The root is read when the test runs rather than with `env!`: a target
/// directory shared between worktrees can run a binary another worktree
/// compiled, whose baked-in path names that worktree.
fn jp_definitions() -> Utf8PathBuf {
    let manifest = env::var("CARGO_MANIFEST_DIR")
        .expect("CARGO_MANIFEST_DIR: run tests via `cargo test` or `cargo nextest`");

    Utf8PathBuf::from(manifest).join("../../../../.jp/mcp/tools/wiki")
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

/// Every routed tool declares, with the type it takes, each parameter `sync`
/// writes values into: values alone would leave the tool out of every
/// conversation.
fn assert_declares_what_sync_writes_into(routed: &[(String, String, toml::Value)]) {
    let vocabulary = Vocabulary::default();
    for (name, key, tool) in routed {
        for mirror in mirrors(name, &vocabulary) {
            let expected = match mirror.values {
                Values::OneOf(_) => "string",
                Values::Labels { .. } => "array",
            };
            assert_eq!(
                tool["parameters"][mirror.parameter]["type"].as_str(),
                Some(expected),
                "{key} takes `{}` from the configuration but doesn't declare it",
                mirror.parameter
            );
        }
    }
}

/// A definition served as `source`, declaring each of `parameters`.
fn declaring(source: &str, parameters: &[&str]) -> Value {
    let parameters: Map<String, Value> = parameters
        .iter()
        .map(|name| ((*name).to_owned(), json!({ "type": "string" })))
        .collect();

    json!({ "source": source, "parameters": parameters })
}

/// Options with `labels` as the vocabulary.
fn options(labels: &Value) -> Options {
    let Value::Object(map) = json!({ "labels": labels }) else {
        unreachable!("a table");
    };

    Options::read(&map).unwrap()
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
                .and_then(|source| source.strip_prefix("plugin.command.wiki."))
            else {
                continue;
            };

            routed.push((name.to_owned(), key.clone(), tool.clone()));
        }
    }
    routed.sort_by(|a, b| a.0.cmp(&b.0));

    routed
}

/// `create`, `edit`, and `list` take labels: `edit` in its `add_labels`, and
/// `list` what a filter takes.
#[test]
fn sync_offers_each_tool_the_labels() {
    let dir = Utf8TempDir::new().unwrap();
    let config = json!({ "conversation": { "tools": {
        "wiki_list": declaring("plugin.command.wiki.list", &["labels"]),
        "wiki_edit": declaring("plugin.command.wiki.edit", &["add_labels"]),
        "wiki_show": declaring("plugin.command.wiki.show", &["labels"]),
    }}});
    let options = options(
        &json!({ "domain": { "description": "The subsystem", "values": ["llm", "storage"] } }),
    );

    let output = sync(dir.path(), &config, &options).unwrap();

    let path = dir.path().join(SYNCED_FILE);
    assert_eq!(
        output.text,
        format!("Wrote {path}, with labels for wiki_edit, wiki_list\n")
    );
    let schemas = r#"
[conversation.tools.wiki_edit.parameters.add_labels.items]
enum = [
  "domain=llm",
  "domain=storage",
]

[conversation.tools.wiki_list.parameters.labels.items]
enum = [
  "domain",
  "domain=llm",
  "domain=storage",
]
"#;
    let written = fs::read_to_string(&path).unwrap();
    assert_eq!(
        written,
        format!("{}\n{SYNCED_NOTES}{schemas}", DEFINITIONS.header())
    );
    toml::from_str::<toml::Value>(&written).unwrap();
}

/// The rules are told in terms of a page, and `create` requires labels when a
/// key needs one.
#[test]
fn sync_describes_the_rules_in_terms_of_a_page() {
    let dir = Utf8TempDir::new().unwrap();
    let config = json!({ "conversation": { "tools": {
        "wiki_create": declaring("plugin.command.wiki.create", &["labels"]),
    }}});
    let options = options(&json!({
        "relevant-rfd": { "description": "An RFD", "values": [] },
        "type": { "description": "The kind", "values": ["note", "study"], "min": 1, "max": 1 },
    }));

    sync(dir.path(), &config, &options).unwrap();

    let schemas = r#"
[conversation.tools.wiki_create.parameters.labels]
required = true

[conversation.tools.wiki_create.parameters.labels.items]
summary = "One `key=value` label: type=note, type=study, or relevant-rfd=<value> with any value. A page carries exactly one label under `type`."
"#;
    let written = fs::read_to_string(dir.path().join(SYNCED_FILE)).unwrap();
    assert_eq!(
        written,
        format!("{}\n{SYNCED_NOTES}{schemas}", DEFINITIONS.header())
    );
    toml::from_str::<toml::Value>(&written).unwrap();
}

/// Without a vocabulary the tools take any label, so no values are written, and
/// the file says why.
#[test]
fn sync_without_labels_says_the_tools_take_any() {
    let dir = Utf8TempDir::new().unwrap();
    let config = json!({ "conversation": { "tools": {
        "wiki_list": declaring("plugin.command.wiki.list", &["labels"]),
    }}});

    let output = sync(dir.path(), &config, &options(&Value::Null)).unwrap();

    let path = dir.path().join(SYNCED_FILE);
    assert_eq!(
        output.text,
        format!("Wrote {path}, with labels for wiki_list\n")
    );
    assert_eq!(
        fs::read_to_string(&path).unwrap(),
        format!(
            "{}\n{SYNCED_NOTES}#\n# The wiki defines no labels, so the tools take any label.\n",
            DEFINITIONS.header()
        )
    );
}

/// Each embedded definition is named for the tool it routes, every tool the
/// plugin serves has one, and each declares the parameters `sync` writes into.
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

    let routed = routed(&templates);
    let names: Vec<&str> = routed.iter().map(|(name, ..)| name.as_str()).collect();
    assert_eq!(names, TOOLS);
    assert_declares_what_sync_writes_into(&routed);
}

/// JP's own configuration routes every tool this plugin serves, and nothing
/// else: a definition naming a tool the plugin doesn't serve is an error at
/// conversation time.
#[test]
fn jp_routes_every_tool_this_plugin_serves() {
    let routed = routed(&read_definitions(&jp_definitions()));
    let names: Vec<&str> = routed.iter().map(|(name, ..)| name.as_str()).collect();

    assert_eq!(names, TOOLS);
    assert_declares_what_sync_writes_into(&routed);
}

/// The definitions name every argument the tools read.
#[test]
fn the_definitions_name_every_argument_the_tools_read() {
    let routed = routed(&templates());
    let parameters = |tool: &str| -> Vec<String> {
        let (_, _, definition) = routed
            .iter()
            .find(|(name, ..)| name == tool)
            .unwrap_or_else(|| panic!("{tool} has a definition"));
        let mut names: Vec<String> = definition["parameters"]
            .as_table()
            .map(|table| table.keys().cloned().collect())
            .unwrap_or_default();
        names.sort();
        names
    };

    assert_eq!(parameters("list"), [
        "full", "labels", "query", "status", "under"
    ]);
    assert_eq!(parameters("show"), [
        "body",
        "expand_embeds",
        "id",
        "section"
    ]);
    assert_eq!(parameters("create"), [
        "body", "labels", "parent", "status", "summary", "title"
    ]);
    assert_eq!(parameters("edit"), [
        "add_labels",
        "after",
        "before",
        "body",
        "id",
        "parent",
        "remove_labels",
        "section",
        "status",
        "summary",
        "superseded_by",
        "title",
    ]);
}
