use std::{env, fs};

use camino::Utf8PathBuf;
use camino_tempfile::Utf8TempDir;
use serde_json::json;
use ticket::Status;
use tool_definitions::SYNCED_FILE;

use super::*;
use crate::tool::{DEFAULT_MAX_TITLE_LENGTH, SHORTER_TITLE, TOOLS};

/// A vocabulary with a retired value and a bare key, which is every shape the
/// three lists treat differently.
fn vocabulary() -> Vocabulary {
    Vocabulary::parse(
        r#"{
            "client": {"description": "The client.", "values": ["cli", "macos"]},
            "package": {"description": "The crate.", "values": ["jp_cli"], "retired": ["jp_legacy"]},
            "triaged": {"description": "Someone has looked at it."}
        }"#,
    )
    .unwrap()
}

/// The root of JP's own repository, for the tests that read its tool
/// definitions and configuration.
///
/// Read when the test runs rather than with `env!`: a target directory shared
/// between worktrees can run a binary another worktree compiled, whose baked-in
/// path names that worktree.
fn jp_root() -> Utf8PathBuf {
    let manifest = env::var("CARGO_MANIFEST_DIR")
        .expect("CARGO_MANIFEST_DIR: run tests via `cargo test` or `cargo nextest`");

    Utf8PathBuf::from(manifest).join("../../../..")
}

/// JP's own definitions of the tools this plugin serves.
fn jp_definitions() -> Utf8PathBuf {
    jp_root().join(".jp/mcp/tools/ticket")
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
/// A definition carrying no `source`, such as the synced file's, is skipped: it
/// only adds to tools declared elsewhere.
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
            let Some(source) = tool.get("source").and_then(toml::Value::as_str) else {
                continue;
            };
            let name = source
                .strip_prefix("plugin.command.ticket.")
                .unwrap_or_else(|| panic!("{key} in {file} is served by {source}"));

            routed.push((name.to_owned(), key.clone(), tool.clone()));
        }
    }
    routed.sort_by(|a, b| a.0.cmp(&b.0));

    routed
}

/// How each tool in `files` batches several calls in one, by the name the
/// plugin knows it by: its `fan_out`, or `None` when it has none.
fn fan_out(files: &[(String, toml::Value)]) -> Vec<(String, Option<toml::Value>)> {
    routed(files)
        .into_iter()
        .map(|(name, _, tool)| (name, tool.get("fan_out").cloned()))
        .collect()
}

/// A `fan_out` value, written as a definition writes it.
fn fan_out_value(text: &str) -> Option<toml::Value> {
    toml::from_str::<toml::Value>(&format!("fan_out = {text}"))
        .unwrap()
        .get("fan_out")
        .cloned()
}

/// Read tools fan out concurrently; write tools run batched operations
/// sequentially in call order so each step sees the previous step's writes.
/// Concurrent tool calls across separate invocations are serialized by
/// `.board.lock` rather than `fan_out`.
/// `list` already accepts filters, so it does not use `fan_out`.
#[test]
fn tools_batch_as_far_as_they_safely_can() {
    let one_at_a_time = fan_out_value("{ concurrency = 1 }");
    let expected = vec![
        ("comment".to_owned(), one_at_a_time.clone()),
        ("create".to_owned(), one_at_a_time.clone()),
        ("edit".to_owned(), one_at_a_time.clone()),
        ("list".to_owned(), None),
        ("move".to_owned(), one_at_a_time),
        ("show".to_owned(), fan_out_value("true")),
    ];

    assert_eq!(fan_out(&templates()), expected);
    assert_eq!(fan_out(&read_definitions(&jp_definitions())), expected);
}

fn enum_values(tool: &toml::Value, parameter: &str) -> Vec<String> {
    tool["parameters"][parameter]["enum"]
        .as_array()
        .unwrap_or_else(|| panic!("`{parameter}` has no enum"))
        .iter()
        .map(|value| value.as_str().expect("string variant").to_owned())
        .collect()
}

/// The statuses a definition advertises must be ones the parser accepts, in
/// both directions: nothing advertised that is rejected, nothing accepted that
/// is hidden.
fn assert_advertised_values_match_the_parser(routed: &[(String, String, toml::Value)]) {
    for (name, key, tool) in routed {
        if !matches!(name.as_str(), "edit" | "list" | "move") {
            continue;
        }

        let values = enum_values(tool, "status");
        for value in &values {
            assert!(
                value.parse::<Status>().is_ok(),
                "{key} advertises `{value}` for `status`"
            );
        }

        let count = [Status::Todo, Status::InProgress, Status::Done]
            .into_iter()
            .filter(|status| values.iter().any(|value| value.parse() == Ok(*status)))
            .count();
        assert_eq!(count, 3, "{key} hides an accepted `status`");
    }
}

/// `create` routes its retitle question to the assistant and advertises the
/// limit it enforces, both by hand, so both are checked against the tool.
fn assert_create_declares_the_question_and_the_limit(routed: &[(String, String, toml::Value)]) {
    let limit = format!("{DEFAULT_MAX_TITLE_LENGTH} characters");

    for (_, key, tool) in routed.iter().filter(|(name, ..)| name == "create") {
        assert_eq!(
            tool["questions"][SHORTER_TITLE]["target"].as_str(),
            Some("assistant"),
            "{key} doesn't route the retitle question to the assistant"
        );

        for field in [
            tool["description"].as_str().expect("description"),
            tool["parameters"]["title"]["summary"]
                .as_str()
                .expect("title summary"),
        ] {
            assert!(
                field.contains(&limit),
                "{key} doesn't advertise the limit `{limit}`: {field}"
            );
        }
    }
}

/// A definition declaring the `labels` parameter, served as `source`.
fn labelled(source: &str) -> Value {
    declaring(source, "labels")
}

/// A definition declaring `parameter`, served as `source`.
fn declaring(source: &str, parameter: &str) -> Value {
    json!({ "source": source, "parameters": { parameter: { "type": "array" } } })
}

/// Tools are found by their source, so a project that exposes `create` under a
/// name of its own gets the labels under that name too.
/// `create` and `edit` are offered what they may add, `edit` in its
/// `add_labels`, and `list` what a filter takes.
#[test]
fn sync_offers_each_tool_its_labels_found_by_source() {
    let dir = Utf8TempDir::new().unwrap();
    let config = json!({
        "conversation": {
            "tools": {
                "*": { "run": "ask" },
                "ticket_create": labelled("plugin.command.ticket.create"),
                "track": labelled("plugin.command.ticket.create"),
                "ticket_edit": declaring("plugin.command.ticket.edit", "add_labels"),
                "list": labelled("plugin.command.ticket"),
                "ticket_show": { "source": "plugin.command.ticket.show" },
                "fs_read_file": { "source": "local" },
                "rfd_list": { "source": "plugin.command.rfd.list" }
            }
        }
    });

    let output = sync(dir.path(), &config, &vocabulary()).unwrap();

    let path = dir.path().join(SYNCED_FILE);
    assert_eq!(
        output.text,
        format!("Wrote {path}, with labels for list, ticket_create, ticket_edit, track\n")
    );
    let lists = r#"
[conversation.tools.list.parameters.labels.items]
enum = [
  "client",
  "client=cli",
  "client=macos",
  "package",
  "package=jp_cli",
  "package=jp_legacy",
  "triaged",
]

[conversation.tools.ticket_create.parameters.labels.items]
enum = [
  "client=cli",
  "client=macos",
  "package=jp_cli",
  "triaged",
]

[conversation.tools.ticket_edit.parameters.add_labels.items]
enum = [
  "client=cli",
  "client=macos",
  "package=jp_cli",
  "triaged",
]

[conversation.tools.track.parameters.labels.items]
enum = [
  "client=cli",
  "client=macos",
  "package=jp_cli",
  "triaged",
]
"#;
    assert_eq!(
        fs::read_to_string(&path).unwrap(),
        format!("{}\n{SYNCED_NOTES}{lists}", DEFINITIONS.header())
    );
}

/// A tool whose definition leaves the parameter out is left out too, rather
/// than given a parameter with no type, which would drop the tool from every
/// conversation.
#[test]
fn sync_leaves_out_a_tool_that_declares_no_labels() {
    let dir = Utf8TempDir::new().unwrap();
    let config = json!({ "conversation": { "tools": {
        "ticket_create": labelled("plugin.command.ticket.create"),
        "ticket_list": {
            "source": "plugin.command.ticket.list",
            "parameters": { "labels": { "items": { "enum": ["client=cli"] } } }
        },
        "ticket_edit": { "source": "plugin.command.ticket.edit" }
    }}});

    let output = sync(dir.path(), &config, &vocabulary()).unwrap();

    let path = dir.path().join(SYNCED_FILE);
    assert_eq!(
        output.text,
        format!(
            "Wrote {path}, with labels for ticket_create\nLeft out ticket_edit (no `add_labels` \
             parameter), ticket_list (no `labels` parameter): the values go into that parameter, \
             which their definitions don't declare.\n"
        )
    );
    let synced = fs::read_to_string(&path).unwrap();
    assert!(!synced.contains("ticket_list"), "{synced}");
    assert!(!synced.contains("ticket_edit"), "{synced}");
}

/// A key a ticket needs makes `create`'s labels required, and the bounds are
/// told in terms of a ticket.
#[test]
fn sync_describes_the_bounds_to_the_tools_that_write_labels() {
    let dir = Utf8TempDir::new().unwrap();
    let config = json!({ "conversation": { "tools": {
        "ticket_create": labelled("plugin.command.ticket.create"),
        "ticket_edit": declaring("plugin.command.ticket.edit", "add_labels"),
        "ticket_list": labelled("plugin.command.ticket.list")
    }}});
    let vocabulary = Vocabulary::parse(
        r#"{
            "type": {"values": ["bug"], "min": 1, "max": 1},
            "team": {"values": ["core"], "max": 1}
        }"#,
    )
    .unwrap();

    sync(dir.path(), &config, &vocabulary).unwrap();

    let lists = r#"
[conversation.tools.ticket_create.parameters.labels]
required = true

[conversation.tools.ticket_create.parameters.labels.items]
summary = "One `key=value` label. A ticket carries at most one label under `team` and exactly one label under `type`."
enum = [
  "team=core",
  "type=bug",
]

[conversation.tools.ticket_edit.parameters.add_labels.items]
summary = "One `key=value` label. A ticket carries at most one label under `team` and exactly one label under `type`."
enum = [
  "team=core",
  "type=bug",
]

[conversation.tools.ticket_list.parameters.labels.items]
enum = [
  "team",
  "team=core",
  "type",
  "type=bug",
]
"#;
    assert_eq!(
        fs::read_to_string(dir.path().join(SYNCED_FILE)).unwrap(),
        format!("{}\n{SYNCED_NOTES}{lists}", DEFINITIONS.header())
    );
}

/// An empty `enum` accepts nothing, which some providers refuse outright, so a
/// board without labels writes no lists at all, and says why.
#[test]
fn sync_with_an_empty_vocabulary_offers_no_lists() {
    let dir = Utf8TempDir::new().unwrap();
    let config = json!({ "conversation": { "tools": {
        "ticket_create": labelled("plugin.command.ticket.create")
    }}});

    sync(dir.path(), &config, &Vocabulary::default()).unwrap();

    assert_eq!(
        fs::read_to_string(dir.path().join(SYNCED_FILE)).unwrap(),
        format!(
            "{}\n{SYNCED_NOTES}#\n# The board defines no labels, so the tools take any label.\n",
            DEFINITIONS.header()
        )
    );
}

/// JP's definitions declare the parameter in every tool `sync` writes labels
/// into, so none of them is left out.
#[test]
fn jp_definitions_declare_the_labels_parameter() {
    let vocabulary = Vocabulary::default();
    for (name, key, tool) in routed(&read_definitions(&jp_definitions())) {
        for mirror in mirrors(&name, &vocabulary) {
            assert!(
                tool["parameters"][mirror.parameter].get("type").is_some(),
                "{key} declares no `{}` parameter",
                mirror.parameter
            );
        }
    }
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
}

#[test]
fn the_starting_definitions_advertise_what_the_tools_accept() {
    let routed = routed(&templates());

    assert_advertised_values_match_the_parser(&routed);
    assert_create_declares_the_question_and_the_limit(&routed);
}

/// JP's own configuration routes every tool this plugin serves, and nothing
/// else: a definition naming a tool the plugin doesn't serve is an error at
/// conversation time.
#[test]
fn jp_routes_every_tool_this_plugin_serves() {
    let names: Vec<String> = routed(&read_definitions(&jp_definitions()))
        .into_iter()
        .map(|(name, ..)| name)
        .collect();

    assert_eq!(names, TOOLS);
}

#[test]
fn jp_definitions_advertise_what_the_tools_accept() {
    let routed = routed(&read_definitions(&jp_definitions()));

    assert_advertised_values_match_the_parser(&routed);
    assert_create_declares_the_question_and_the_limit(&routed);
}

/// JP's board vocabulary, in its workspace configuration, has to parse,
/// whatever it currently holds.
///
/// Deliberately not a comparison against the lists JP's definitions advertise:
/// those are a mirror kept by `jp ticket tools sync`, and adding a label should
/// never turn a second file into a failing test.
#[test]
fn the_jp_board_vocabulary_parses() {
    let path = jp_root().join(".jp/config.toml");
    let config: toml::Value = toml::from_str(&fs::read_to_string(&path).unwrap()).unwrap();

    let labels = ["plugins", "command", "ticket", "options", "labels"]
        .iter()
        .try_fold(&config, |value, key| value.get(key))
        .unwrap_or_else(|| panic!("{path} defines no `plugins.command.ticket.options.labels`"));

    Vocabulary::from_value(serde_json::to_value(labels).unwrap())
        .unwrap_or_else(|error| panic!("{path}: {error}"));
}
