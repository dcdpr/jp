use std::fs;

use camino_tempfile::Utf8TempDir;
use clap::Error as ClapError;
use jp_plugin::message::{OutputFormat, PathsInfo, ToolAction, WorkspaceInfo};
use serde_json::{Map, json};

use super::*;

fn parse(args: &[&str]) -> Result<Args, ClapError> {
    Args::try_parse_from(iter::once("jp rfd").chain(args.iter().copied()))
}

/// A workspace holding one RFD, `042`, planned first and in development, and
/// one draft in the backlog.
fn workspace() -> Utf8TempDir {
    let root = Utf8TempDir::new().unwrap();
    let rfds = root.path().join("docs/rfd");
    let tickets = root.path().join("docs/ticket");
    fs::create_dir_all(rfds.join("drafts")).unwrap();
    fs::create_dir_all(&tickets).unwrap();

    fs::write(
        rfds.join("042-ordered-things.md"),
        "# RFD 042: Ordered Things\n\n- **Status**: Accepted\n- **Category**: Design\n",
    )
    .unwrap();
    fs::write(
        rfds.join("drafts/D05-sketch.md"),
        "# RFD D05: Sketch\n\n- **Status**: Draft\n- **Category**: Guide\n",
    )
    .unwrap();
    fs::write(
        rfds.join(".priority.json"),
        r#"{"planned": [{"milestone": null, "ids": ["042"]}], "backlog": ["D05"]}"#,
    )
    .unwrap();
    fs::write(
        tickets.join("02wt0k1-track.md"),
        concat!(
            "# Ordered things\n\n",
            "- **Status**: In Progress\n",
            "- **Kind**: Feature\n",
            "- **Authors**: jp\n",
            "- **Date**: 2026-08-05\n",
            "- **Implements**: 042\n",
            "- **Label**: type=tracking\n",
        ),
    )
    .unwrap();

    root
}

fn init_message(root: &Utf8Path, args: &[&str]) -> InitMessage {
    InitMessage {
        version: jp_plugin::PROTOCOL_VERSION,
        workspace: WorkspaceInfo {
            root: root.to_path_buf(),
            storage: root.join(".jp"),
            id: "test".to_owned(),
        },
        paths: PathsInfo::default(),
        config: json!({}),
        options: Map::new(),
        args: args.iter().map(|arg| (*arg).to_owned()).collect(),
        log_level: 0,
        output_format: OutputFormat::default(),
        tool: None,
    }
}

fn init(root: &Utf8Path, args: &[&str]) -> HostToPlugin {
    HostToPlugin::Init(Box::new(init_message(root, args)))
}

/// Drive the plugin the way the host does: one JSON message in, the reply
/// stream out.
fn exchange(message: &HostToPlugin) -> Vec<PluginToHost> {
    let input = serde_json::to_string(message).unwrap() + "\n";
    let mut output = vec![];
    run(input.as_bytes(), &mut output).unwrap();

    String::from_utf8(output)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

/// What a command printed and how it exited.
fn printed(messages: &[PluginToHost]) -> (String, u8, Option<String>) {
    let mut text = String::new();
    let mut exit = None;
    for message in messages {
        match message {
            PluginToHost::Print(print) => text.push_str(&print.text),
            PluginToHost::Exit(done) => exit = Some((done.code, done.reason.clone())),
            _ => {}
        }
    }
    let (code, reason) = exit.expect("the plugin exits");

    (text, code, reason)
}

#[test]
fn cli_definition_is_valid() {
    Args::command().debug_assert();
}

#[test]
fn list_takes_a_category_and_the_section_flags() {
    match parse(&["list", "design", "--backlog", "--full"])
        .unwrap()
        .command
    {
        Command::List {
            category,
            planned,
            backlog,
            all,
            full,
            json,
        } => {
            assert_eq!(category.as_deref(), Some("design"));
            assert!(!planned && backlog && !all && full && !json);
        }
        other => panic!("expected list, got {other:?}"),
    }
}

#[test]
fn show_takes_an_id() {
    match parse(&["show", "d5"]).unwrap().command {
        Command::Show { id, json } => {
            assert_eq!(id, RfdId::Draft(5));
            assert!(!json);
        }
        other => panic!("expected show, got {other:?}"),
    }

    let error = parse(&["show", "nope"]).unwrap_err();
    assert!(
        error
            .to_string()
            .contains("Invalid RFD id 'nope'. Use a number (41) or draft ID (D01)."),
        "{error}"
    );
    assert!(parse(&["show"]).is_err());
}

#[test]
fn describe_reports_the_subcommand_it_serves() {
    let messages = exchange(&HostToPlugin::Describe);

    match messages.as_slice() {
        [PluginToHost::Describe(describe)] => {
            assert_eq!(describe.name, "rfd");
            assert_eq!(describe.manifest.command, ["rfd"]);
            assert_eq!(describe.help, help_text());
        }
        other => panic!("expected a single describe, got {other:?}"),
    }
}

/// The host refuses a plugin before spawning it when the manifest asks for a
/// newer protocol, so the manifest has to state the same need the handshake
/// does.
#[test]
fn the_manifest_states_the_protocol_the_handshake_needs() {
    let manifest = jp_plugin::manifest::find(MANIFEST.as_bytes())
        .unwrap()
        .unwrap();

    assert_eq!(manifest.protocol, REQUIRED_PROTOCOL);
}

#[test]
fn a_host_older_than_the_plugin_is_refused() {
    let root = workspace();
    let mut message = init_message(root.path(), &["list"]);
    message.version = REQUIRED_PROTOCOL - 1;

    let messages = exchange(&HostToPlugin::Init(Box::new(message)));

    match messages.as_slice() {
        [PluginToHost::Exit(exit)] => {
            assert_ne!(exit.code, 0);
            assert!(
                exit.reason
                    .as_deref()
                    .is_some_and(|reason| reason.contains("needs `jp` protocol 12")),
                "{:?}",
                exit.reason
            );
        }
        other => panic!("unexpected exchange: {other:?}"),
    }
}

#[test]
fn list_prints_the_board() {
    let root = workspace();

    let (text, code, _) = printed(&exchange(&init(root.path(), &["list", "--all"])));

    assert_eq!(code, 0);
    assert_eq!(
        text,
        concat!(
            "--planned (1), --backlog (1), --all (2), --full\n",
            "\n",
            "Planned (priority order, * in development)\n",
            "\n",
            "  1  *  042  Accepted    Ordered Things\n",
            "\n",
            "Backlog (not yet ranked)\n",
            "\n",
            "        D05  Draft       Sketch\n",
        )
    );
}

#[test]
fn list_json_parses_and_carries_every_rfd() {
    let root = workspace();

    let (text, code, _) = printed(&exchange(&init(root.path(), &["list", "--json"])));

    assert_eq!(code, 0);
    let entries: Value = serde_json::from_str(&text).unwrap();
    let nums: Vec<&str> = entries
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| entry["num"].as_str().unwrap())
        .collect();
    assert_eq!(nums, ["042", "D05"]);
}

/// The commands resolve their directories against the workspace root, so they
/// work from anywhere in it.
#[test]
fn the_rfd_directory_comes_from_the_options() {
    let root = workspace();
    fs::rename(root.path().join("docs/rfd"), root.path().join("design")).unwrap();
    let mut message = init_message(root.path(), &["show", "42"]);
    message.options = Map::from_iter([("dir".to_owned(), json!("design"))]);

    let (text, code, reason) = printed(&exchange(&HostToPlugin::Init(Box::new(message))));

    assert_eq!(code, 0, "{reason:?}");
    assert!(
        text.contains("- **File**: design/042-ordered-things.md\n"),
        "{text}"
    );
}

#[test]
fn show_prints_one_rfd() {
    let root = workspace();

    let (text, code, _) = printed(&exchange(&init(root.path(), &["show", "D5"])));

    assert_eq!(code, 0);
    assert_eq!(
        text,
        concat!(
            "# RFD D05: Sketch\n",
            "\n",
            "- **File**: docs/rfd/drafts/D05-sketch.md\n",
            "- **Status**: Draft\n",
            "- **Category**: Guide\n",
            "- **Board**: backlog\n",
            "- **In development**: no\n",
        )
    );
}

#[test]
fn show_json_is_one_tagged_entry() {
    let root = workspace();

    let (text, code, _) = printed(&exchange(&init(root.path(), &["show", "042", "--json"])));

    assert_eq!(code, 0);
    let entry: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(entry["num"], "042");
    assert_eq!(entry["group"], "planned");
    assert_eq!(entry["rank"], 1);
    assert_eq!(entry["inDevelopment"], true);
}

#[test]
fn show_reports_an_rfd_that_isnt_there() {
    let root = workspace();

    let (_, code, reason) = printed(&exchange(&init(root.path(), &["show", "43"])));

    assert_eq!(code, 1);
    assert_eq!(reason.as_deref(), Some("No RFD found with number 043."));
}

/// The board file is hand-editable; one that doesn't parse still lists, with a
/// warning on the log rather than in the output, which a script may be reading.
#[test]
fn a_broken_board_file_is_a_warning_not_a_failure() {
    let root = workspace();
    fs::write(root.path().join("docs/rfd/.priority.json"), "{ nope").unwrap();

    let messages = exchange(&init(root.path(), &["list", "--json"]));

    let warnings: Vec<&str> = messages
        .iter()
        .filter_map(|message| match message {
            PluginToHost::Log(log) if log.level == "warn" => Some(log.message.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(warnings.len(), 1, "{messages:?}");
    assert!(warnings[0].contains("is not valid JSON"), "{}", warnings[0]);

    let (text, code, _) = printed(&messages);
    assert_eq!(code, 0);
    serde_json::from_str::<Value>(&text).unwrap();
}

#[test]
fn a_missing_rfd_directory_ends_the_run() {
    let root = Utf8TempDir::new().unwrap();

    let (_, code, reason) = printed(&exchange(&init(root.path(), &["list"])));

    assert_eq!(code, 1);
    assert_eq!(
        reason,
        Some(format!(
            "There is no RFD directory at {}. Set `plugins.command.rfd.options.dir` to the \
             directory holding the RFDs.",
            root.path().join("docs/rfd")
        ))
    );
}

#[test]
fn unreadable_options_end_the_run() {
    let root = workspace();
    let mut message = init_message(root.path(), &["list"]);
    message.options = Map::from_iter([("categories".to_owned(), json!([]))]);

    let (_, code, reason) = printed(&exchange(&HostToPlugin::Init(Box::new(message))));

    assert_eq!(code, 1);
    assert_eq!(
        reason.as_deref(),
        Some("`plugins.command.rfd.options.categories` is empty; name at least one category.")
    );
}

#[test]
fn help_is_printed_and_is_not_a_failure() {
    let root = workspace();

    let (text, code, _) = printed(&exchange(&init(root.path(), &["--help"])));

    assert_eq!(code, 0);
    assert!(text.contains("Usage: jp rfd"), "{text}");
}

/// `tools sync` reads the categories from the plugin's options and the tools
/// from the whole configuration.
#[test]
fn tools_sync_writes_the_configured_categories() {
    let root = workspace();
    fs::create_dir_all(root.path().join("tools")).unwrap();
    let mut message = init_message(root.path(), &["tools", "sync", "tools"]);
    message.options = Map::from_iter([("categories".to_owned(), json!(["design", "adr"]))]);
    message.config = json!({ "conversation": { "tools": {
        "rfd_list": { "source": "plugin.command.rfd.list" }
    }}});

    let (_, code, reason) = printed(&exchange(&HostToPlugin::Init(Box::new(message))));

    assert_eq!(code, 0, "{reason:?}");
    let synced = fs::read_to_string(root.path().join("tools/synced.toml")).unwrap();
    assert!(
        synced.contains(concat!(
            "[conversation.tools.rfd_list.parameters.category]\n",
            "enum = [\n",
            "  \"design\",\n",
            "  \"adr\",\n",
            "]\n",
        )),
        "{synced}"
    );
}

/// A tool call is answered with one outcome and an exit, and nothing printed.
/// The host closed stdin after `init`, so the plugin must not wait for more.
#[test]
fn a_tool_call_is_answered_with_its_outcome_then_exit() {
    let root = workspace();
    let mut message = init_message(root.path(), &[]);
    message.tool = Some(ToolCall {
        action: ToolAction::Run,
        name: "show".to_owned(),
        arguments: Map::from_iter([("id".to_owned(), json!("042"))]),
        answers: Map::new(),
        options: Map::new(),
        access: None,
        conversation: "jp-c17000000000".to_owned(),
    });

    let messages = exchange(&HostToPlugin::Init(Box::new(message)));

    match messages.as_slice() {
        [
            PluginToHost::Ready(ready),
            PluginToHost::ToolOutcome(answer),
            PluginToHost::Exit(exit),
        ] => {
            assert_eq!(ready.protocol, REQUIRED_PROTOCOL);
            assert_eq!(answer.outcome["type"], "success");
            let content = answer.outcome["content"].as_str().unwrap_or_default();
            assert!(
                content.starts_with("```markdown\n# RFD 042: Ordered Things\n"),
                "{content}"
            );
            assert_eq!(exit.code, 0);
        }
        other => panic!("unexpected exchange: {other:?}"),
    }
}
