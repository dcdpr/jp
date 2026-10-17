use camino::Utf8PathBuf;
use camino_tempfile::Utf8TempDir;
use serde_json::json;

use super::*;
use crate::tool::TOOLS;

fn categories() -> Vec<String> {
    vec!["design".to_owned(), "guide".to_owned()]
}

/// JP's own definitions of the RFD tools.
fn jp_definitions() -> Utf8PathBuf {
    Utf8PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../../.jp/mcp/tools/rfd")
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
        .iter()
        .map(|(name, source)| {
            let value = toml::from_str(source).unwrap_or_else(|error| panic!("{name}: {error}"));
            ((*name).to_owned(), value)
        })
        .collect()
}

/// The tools a set of definitions routes to this plugin, as `(tool, key,
/// definition)`, where `tool` is the name the plugin knows it by.
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

/// Every tool that takes a category declares the parameter `sync` writes the
/// accepted values into.
fn assert_category_tools_declare_the_parameter(routed: &[(String, String, toml::Value)]) {
    for (name, key, tool) in routed {
        if !TAKES_A_CATEGORY.contains(&name.as_str()) {
            continue;
        }

        assert_eq!(
            tool["parameters"]["category"]["type"].as_str(),
            Some("string"),
            "{key} takes a category but doesn't declare one"
        );
    }
}

#[test]
fn init_writes_one_file_per_tool() {
    let dir = Utf8TempDir::new().unwrap();
    let target = dir.path().join("tools/rfd");

    let output = init(&target).unwrap();

    for (name, content) in DEFINITIONS {
        assert_eq!(fs::read_to_string(target.join(name)).unwrap(), *content);
        assert!(
            output
                .text
                .contains(&format!("Wrote {}\n", target.join(name))),
            "{}",
            output.text
        );
    }
}

/// A project owns its definitions once they are written, so a second `init`
/// must not undo its edits, and refuses before writing any file.
#[test]
fn init_refuses_to_overwrite_and_writes_nothing() {
    let dir = Utf8TempDir::new().unwrap();
    fs::write(dir.path().join("show.toml"), "# mine\n").unwrap();

    let error = init(dir.path()).unwrap_err();

    assert_eq!(
        error,
        format!(
            "Refusing to overwrite {}. Move them aside first.",
            dir.path().join("show.toml")
        )
    );
    assert_eq!(
        fs::read_to_string(dir.path().join("show.toml")).unwrap(),
        "# mine\n"
    );
    assert!(!dir.path().join("list.toml").exists());
}

/// A definition declaring the `category` parameter, served as `source`.
fn categorized(source: &str) -> serde_json::Value {
    json!({ "source": source, "parameters": { "category": { "type": "string" } } })
}

/// Tools are found by their source, so a project that exposes `list` under a
/// name of its own gets the categories under that name too, and a tool of the
/// same name served by something else gets none.
#[test]
fn sync_offers_each_tool_the_categories_found_by_source() {
    let dir = Utf8TempDir::new().unwrap();
    let config = json!({
        "conversation": {
            "tools": {
                "*": { "run": "ask" },
                "rfd_list": categorized("plugin.command.rfd.list"),
                "board": categorized("plugin.command.rfd.list"),
                "new_doc": categorized("plugin.command.rfd.draft"),
                "rfd_show": { "source": "plugin.command.rfd.show" },
                "ticket_list": categorized("plugin.command.ticket.list"),
                "rfd_draft": categorized("local")
            }
        }
    });

    let output = sync(dir.path(), &config, &categories()).unwrap();

    let path = dir.path().join(SYNCED_FILE);
    assert_eq!(
        output.text,
        format!("Wrote {path}, with categories for board, new_doc, rfd_list\n")
    );
    let lists = r#"
[conversation.tools.board.parameters.category]
enum = [
  "design",
  "guide",
]

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
        format!("{SYNCED_HEADER}\n{SYNCED_NOTES}{lists}")
    );
}

/// The file is rewritten whole, so a tool removed from the configuration drops
/// out of it rather than lingering.
#[test]
fn sync_drops_a_tool_no_longer_configured() {
    let dir = Utf8TempDir::new().unwrap();
    let configured = json!({ "conversation": { "tools": {
        "rfd_list": categorized("plugin.command.rfd.list")
    }}});

    sync(dir.path(), &configured, &categories()).unwrap();
    let output = sync(dir.path(), &json!({}), &categories()).unwrap();

    let path = dir.path().join(SYNCED_FILE);
    assert_eq!(
        output.text,
        format!("Wrote {path}; no configured tool takes a category from `jp rfd`.\n")
    );
    assert_eq!(
        fs::read_to_string(&path).unwrap(),
        format!("{SYNCED_HEADER}\n{SYNCED_NOTES}")
    );
}

/// A tool whose definition leaves the parameter out is left out too, rather
/// than given a parameter with no type, which would drop the tool from every
/// conversation.
/// The values an earlier sync wrote don't declare it.
#[test]
fn sync_leaves_out_a_tool_that_declares_no_category() {
    let dir = Utf8TempDir::new().unwrap();
    let config = json!({ "conversation": { "tools": {
        "rfd_list": categorized("plugin.command.rfd.list"),
        "rfd_draft": {
            "source": "plugin.command.rfd.draft",
            "parameters": { "category": { "enum": ["design"] } }
        }
    }}});

    let output = sync(dir.path(), &config, &categories()).unwrap();

    let path = dir.path().join(SYNCED_FILE);
    assert_eq!(
        output.text,
        format!(
            "Wrote {path}, with categories for rfd_list\nLeft out rfd_draft: their definitions \
             declare no `category` parameter.\n"
        )
    );
    assert!(!fs::read_to_string(&path).unwrap().contains("rfd_draft"));
}

/// `sync` reads a declared parameter's type under `type`, which is how the host
/// sends the configuration.
#[test]
fn the_host_sends_a_declared_parameter_with_its_type() {
    let partial: jp_config::PartialAppConfig = toml::from_str(
        r#"
        [conversation.tools.rfd_list]
        source = "plugin.command.rfd.list"

        [conversation.tools.rfd_list.parameters.category]
        type = "string"

        [conversation.tools.rfd_draft]
        source = "plugin.command.rfd.draft"

        [conversation.tools.rfd_draft.parameters.category]
        enum = ["design"]
        "#,
    )
    .unwrap();
    let config = serde_json::to_value(&partial).unwrap();

    assert!(declares(&config, "rfd_list", "category"), "{config:#}");
    assert!(!declares(&config, "rfd_draft", "category"), "{config:#}");
}

/// JP's definitions declare the parameter in every tool `sync` writes
/// categories into, so none of them is left out.
#[test]
fn jp_definitions_declare_the_category_parameter() {
    for (name, key, tool) in routed(&read_definitions(&jp_definitions())) {
        if !TAKES_A_CATEGORY.contains(&name.as_str()) {
            continue;
        }

        assert!(
            tool["parameters"]["category"].get("type").is_some(),
            "{key} declares no `category` parameter"
        );
    }
}

/// A file in the synced file's place that `sync` didn't write is someone's, and
/// stays as it is.
/// `jp ticket tools sync` writes a file of the same name, so pointing both
/// plugins at one directory must not let either clobber the other's.
#[test]
fn sync_leaves_a_file_it_did_not_write_alone() {
    let dir = Utf8TempDir::new().unwrap();
    let path = dir.path().join(SYNCED_FILE);
    fs::write(&path, "# Generated by `jp ticket tools sync`.\n").unwrap();

    let error = sync(dir.path(), &json!({}), &categories()).unwrap_err();

    assert_eq!(
        error,
        format!(
            "{path} was not written by `jp rfd tools sync`, so it is left alone. Move it aside \
             first."
        )
    );
    assert_eq!(
        fs::read_to_string(&path).unwrap(),
        "# Generated by `jp ticket tools sync`.\n"
    );
}

#[test]
fn sync_needs_an_existing_directory() {
    let dir = Utf8TempDir::new().unwrap();
    let missing = dir.path().join("nowhere");

    assert_eq!(
        sync(&missing, &json!({}), &categories()).unwrap_err(),
        format!("{missing} is not a directory.")
    );
}

#[test]
fn a_tool_named_by_the_plugin_alone_is_known_by_its_key() {
    let config = json!({ "conversation": { "tools": {
        "list": { "source": "plugin.command.rfd" },
        "rfd_show": { "source": "plugin.command.rfd.show" },
        "rfdx": { "source": "plugin.command.rfdx.list" }
    }}});

    assert_eq!(served_tools(&config), [
        ("list".to_owned(), "list".to_owned()),
        ("rfd_show".to_owned(), "show".to_owned()),
    ]);
}

/// The file `sync` writes has to load, or the whole workspace configuration
/// fails with it.
#[test]
fn the_synced_file_parses() {
    let source = synced(
        &["my tool".to_owned(), "rfd_list".to_owned()],
        &categories(),
    );

    let value: toml::Value = toml::from_str(&source).unwrap();
    assert_eq!(
        value["conversation"]["tools"]["my tool"]["parameters"]["category"]["enum"]
            .as_array()
            .map(Vec::len),
        Some(2)
    );
    assert_eq!(toml_key("a.b"), "\"a.b\"");
    assert_eq!(toml_string("say \"hi\""), r#""say \"hi\"""#);
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
        "start",
        "stop",
        "supersede",
        "track",
    ]);
    assert!(names.iter().all(|name| TOOLS.contains(name)), "{names:?}");
    assert_category_tools_declare_the_parameter(&routed);
}
