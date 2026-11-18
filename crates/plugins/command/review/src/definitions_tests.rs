use std::fs;

use camino_tempfile::Utf8TempDir;
use pretty_assertions::assert_eq;

use super::*;
use crate::tool::TOOLS;

/// Every shipped definition, parsed.
fn shipped() -> Vec<toml::Value> {
    DEFINITIONS
        .files
        .iter()
        .map(|(name, source)| {
            toml::from_str(source).unwrap_or_else(|error| panic!("{name}: {error}"))
        })
        .collect()
}

/// The one tool a parsed definition defines, by its key.
fn defined(definition: &toml::Value) -> (&str, &toml::Value) {
    let tools = definition["conversation"]["tools"].as_table().unwrap();
    assert_eq!(tools.len(), 1, "{tools:?}");

    tools
        .iter()
        .next()
        .map(|(key, tool)| (key.as_str(), tool))
        .unwrap()
}

/// The host routes a call by its `source`, so each definition has to name a
/// tool this plugin serves.
#[test]
fn each_definition_is_served_by_this_plugin() {
    let mut served: Vec<&str> = vec![];
    let definitions = shipped();
    for definition in &definitions {
        let (key, tool) = defined(definition);
        let source = tool["source"].as_str().unwrap();
        let name = source.strip_prefix("plugin.command.review.").unwrap();

        assert_eq!(key, format!("review_{name}"));
        served.push(name);
    }
    served.sort_unstable();

    assert_eq!(served, TOOLS);
}

/// A tool only a pass can use is offered in no other conversation.
#[test]
fn the_definitions_leave_the_tools_off() {
    for definition in shipped() {
        let (key, tool) = defined(&definition);
        assert_eq!(tool["enable"].as_bool(), Some(false), "{key}");
    }
}

/// The configuration enables the tools the definitions define, under their
/// keys.
#[test]
fn the_configuration_enables_the_shipped_tools() {
    let config: toml::Value = toml::from_str(CONFIG).unwrap();
    let enabled = config["conversation"]["tools"].as_table().unwrap();

    let definitions = shipped();
    let mut keys: Vec<&str> = definitions
        .iter()
        .map(|definition| defined(definition).0)
        .collect();
    keys.sort_unstable();
    let mut configured: Vec<&str> = enabled.keys().map(String::as_str).collect();
    configured.sort_unstable();

    assert_eq!(configured, keys);
    for key in keys {
        assert_eq!(enabled[key]["enable"].as_bool(), Some(true), "{key}");
    }
    assert!(
        config["assistant"]["system_prompt_sections"][0]["content"]
            .as_str()
            .is_some_and(|content| content.contains("review_reply")),
        "{config:?}"
    );
}

#[test]
fn tools_init_writes_one_file_per_tool() {
    let dir = Utf8TempDir::new().unwrap();
    let target = dir.path().join("tools/review");

    let output = tools_init(&target, false).unwrap();

    assert_eq!(
        output.text,
        format!(
            "Wrote {}\nWrote {}\nLoad the directory from the workspace configuration to offer the \
             tools.\n",
            target.join("comment.toml"),
            target.join("reply.toml")
        )
    );
    assert_eq!(
        fs::read_to_string(target.join("reply.toml")).unwrap(),
        include_str!("../tools/reply.toml")
    );
}

#[test]
fn init_writes_the_configuration_and_says_where_it_goes() {
    let dir = Utf8TempDir::new().unwrap();

    let output = init(dir.path(), false).unwrap();

    assert_eq!(
        output.text,
        format!(
            "Wrote {}\nAdd it to `plugins.command.review.options.cfg`, the way `--cfg` takes it, \
             for passes to run under it.\n",
            dir.path().join(CONFIG_FILE)
        )
    );
    assert_eq!(
        fs::read_to_string(dir.path().join(CONFIG_FILE)).unwrap(),
        CONFIG
    );
}

/// The workspace owns the file once written, so an edit git can't restore is
/// left alone without `--force`.
#[test]
fn init_leaves_an_edited_configuration_unless_forced() {
    let dir = Utf8TempDir::new().unwrap();
    let path = dir.path().join(CONFIG_FILE);
    fs::write(&path, "# mine\n").unwrap();

    let refused = init(dir.path(), false).unwrap_err();
    assert_eq!(
        refused,
        format!(
            "Git couldn't restore these files, so `jp review init {}` leaves them:\n  {path} (not \
             in a git work tree)\nRun `jp review init {} --force` to overwrite them anyway.",
            dir.path(),
            dir.path()
        )
    );
    assert_eq!(fs::read_to_string(&path).unwrap(), "# mine\n");

    let forced = init(dir.path(), true).unwrap();
    assert!(
        forced.text.starts_with(&format!("Overwrote {path}\n")),
        "{}",
        forced.text
    );
    assert_eq!(fs::read_to_string(&path).unwrap(), CONFIG);
}
