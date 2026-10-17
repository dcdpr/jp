use std::{fs, slice};

use camino_tempfile::Utf8TempDir;
use chrono::NaiveDate;
use clap::Error as ClapError;
use jp_plugin::message::{
    ComposeResponse, CreatedResponse, ErrorResponse, OutputFormat, PathsInfo,
    QueryCompleteResponse, QueryRequest, ToolAction, WorkspaceInfo,
};
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
    exchange_all(slice::from_ref(message))
}

/// Drive the plugin through a conversation: `messages` are what the host
/// answers, in order, starting with `init`.
fn exchange_all(messages: &[HostToPlugin]) -> Vec<PluginToHost> {
    let input: String = messages
        .iter()
        .map(|message| serde_json::to_string(message).unwrap() + "\n")
        .collect();
    let mut output = vec![];
    run(input.as_bytes(), &mut output).unwrap();

    String::from_utf8(output)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

/// What a command printed on stdout and how it exited.
fn printed(messages: &[PluginToHost]) -> (String, u8, Option<String>) {
    let mut text = String::new();
    let mut exit = None;
    for message in messages {
        match message {
            PluginToHost::Print(print) if print.channel == "content" => {
                text.push_str(&print.text);
            }
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

/// `jp vite serve` reads the revision of the site files this plugin was built
/// alongside out of the parentheses.
#[test]
fn the_version_names_the_site_revision() {
    let root = Utf8TempDir::new().unwrap();

    let (text, code, _) = printed(&exchange(&init(root.path(), &["--version"])));

    assert_eq!(code, 0);
    assert_eq!(text, format!("jp rfd {VERSION}\n"));

    let revision = VERSION
        .split_once("(site ")
        .and_then(|(_, rest)| rest.strip_suffix(')'))
        .unwrap();
    assert_eq!(revision.len(), 16, "{VERSION}");
    assert!(revision.chars().all(|c| c.is_ascii_hexdigit()), "{VERSION}");
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
fn draft_takes_a_category_and_a_title_or_a_ticket() {
    match parse(&["draft", "design", "My", "Feature"])
        .unwrap()
        .command
    {
        Command::Draft {
            category,
            title,
            from_ticket,
        } => {
            assert_eq!(category, "design");
            assert_eq!(title, ["My", "Feature"]);
            assert_eq!(from_ticket, None);
        }
        other => panic!("expected draft, got {other:?}"),
    }

    match parse(&["draft", "design", "--from-ticket", "02wt0kx"])
        .unwrap()
        .command
    {
        Command::Draft {
            title, from_ticket, ..
        } => {
            assert!(title.is_empty());
            assert_eq!(from_ticket, Some("T-02wt0kx".parse().unwrap()));
        }
        other => panic!("expected draft, got {other:?}"),
    }

    assert!(parse(&["draft", "design"]).is_err());
}

/// `extend` and `require` read the same way: the first RFD holds the
/// relationship.
#[test]
fn relationships_take_the_holding_rfd_first() {
    match parse(&["extend", "116", "100"]).unwrap().command {
        Command::Extend { id, target } => {
            assert_eq!((id, target), (RfdId::Published(116), RfdId::Published(100)));
        }
        other => panic!("expected extend, got {other:?}"),
    }
    match parse(&["require", "116", "72"]).unwrap().command {
        Command::Require { id, target } => {
            assert_eq!((id, target), (RfdId::Published(116), RfdId::Published(72)));
        }
        other => panic!("expected require, got {other:?}"),
    }
}

#[test]
fn rank_takes_exactly_one_place() {
    match parse(&["rank", "60", "--before", "50"]).unwrap().command {
        Command::Rank {
            id,
            before,
            after,
            backlog,
        } => {
            assert_eq!(id, RfdId::Published(60));
            assert_eq!(before, Some(RfdId::Published(50)));
            assert!(after.is_none() && !backlog);
        }
        other => panic!("expected rank, got {other:?}"),
    }

    assert!(parse(&["rank", "60"]).is_err());
    assert!(parse(&["rank", "60", "--before", "50", "--backlog"]).is_err());
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

/// What a command printed on stderr, one entry per print.
fn chrome(messages: &[PluginToHost]) -> Vec<&str> {
    messages
        .iter()
        .filter_map(|message| match message {
            PluginToHost::Print(print) if print.channel == "chrome" => Some(print.text.as_str()),
            _ => None,
        })
        .collect()
}

/// The board file is hand-editable; one that doesn't parse still lists, with a
/// warning on stderr rather than in the output, which a script may be reading.
#[test]
fn a_broken_board_file_is_a_warning_not_a_failure() {
    let root = workspace();
    fs::write(root.path().join("docs/rfd/.priority.json"), "{ nope").unwrap();

    let messages = exchange(&init(root.path(), &["list", "--json"]));

    let warnings = chrome(&messages);
    assert_eq!(warnings.len(), 1, "{messages:?}");
    assert!(warnings[0].contains("is not valid JSON"), "{}", warnings[0]);

    let (text, code, _) = printed(&messages);
    assert_eq!(code, 0);
    serde_json::from_str::<Value>(&text).unwrap();
}

/// A draft is attributed to the user JP's configuration names, and dated the
/// day it is filed.
#[test]
fn draft_writes_a_draft_by_the_configured_user() {
    let root = workspace();
    fs::write(
        root.path().join("docs/rfd/000-design-template.md"),
        "# RFD NNN: TITLE\n\n- **Status**: Draft\n- **Category**: Design\n- **Authors**: \
         AUTHOR\n- **Date**: DATE\n",
    )
    .unwrap();
    let mut message = init_message(root.path(), &["draft", "design", "My", "Feature"]);
    message.config = json!({ "user": { "name": "Jane Doe <jane@example.com>" } });

    let (text, code, reason) = printed(&exchange(&HostToPlugin::Init(Box::new(message))));

    assert_eq!((code, reason), (0, None));
    assert_eq!(text, "Created docs/rfd/drafts/D01-my-feature.md\n");
    let draft = fs::read_to_string(root.path().join("docs/rfd/drafts/D01-my-feature.md")).unwrap();
    assert!(
        draft.contains("- **Authors**: Jane Doe <jane@example.com>\n"),
        "{draft}"
    );
    let date = draft
        .lines()
        .find_map(|line| line.strip_prefix("- **Date**: "))
        .unwrap();
    assert!(
        NaiveDate::parse_from_str(date, "%Y-%m-%d").is_ok(),
        "{date}"
    );
}

/// A warning names something to check now, so it goes to stderr, where the user
/// sees it without `-v`, and stays out of what a script reads.
#[test]
fn a_commands_warnings_go_to_stderr() {
    let root = workspace();
    fs::write(
        root.path().join("docs/rfd/043-depended-on.md"),
        "# RFD 043: Depended On\n\n- **Status**: Accepted\n- **Required by**: [RFD \
         050](050-x.md)\n",
    )
    .unwrap();

    let messages = exchange(&init(root.path(), &["abandon", "43", "Not", "needed."]));

    let (text, code, _) = printed(&messages);
    assert_eq!(code, 0);
    assert_eq!(
        text,
        "docs/rfd/043-depended-on.md: Abandoned (Accepted -> Abandoned)\n"
    );
    let warning = "Warning: the following RFDs depend on this one (Required by):\n  RFD \
                   050\nTheir dependency on RFD 043 is now broken; review and update.\n";
    assert_eq!(chrome(&messages), [warning]);
    assert!(
        fs::read_to_string(root.path().join("docs/rfd/043-depended-on.md"))
            .unwrap()
            .contains("> Not needed.\n")
    );
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

#[test]
fn track_start_and_stop_take_an_id() {
    assert!(matches!(
        parse(&["track", "116"]).unwrap().command,
        Command::Track {
            id: RfdId::Published(116)
        }
    ));
    assert!(matches!(
        parse(&["start", "116"]).unwrap().command,
        Command::Start {
            id: RfdId::Published(116),
            no_plan: false,
        }
    ));
    assert!(matches!(
        parse(&["start", "116", "--no-plan"]).unwrap().command,
        Command::Start {
            id: RfdId::Published(116),
            no_plan: true,
        }
    ));
    assert!(matches!(
        parse(&["stop", "116"]).unwrap().command,
        Command::Stop {
            id: RfdId::Published(116)
        }
    ));
    assert!(parse(&["track"]).is_err());
}

/// [`workspace`], with RFD 043 Accepted and untracked, and RFD 044 in
/// Discussion.
fn trackable() -> Utf8TempDir {
    let root = workspace();
    fs::write(
        root.path().join("docs/rfd/043-more-order.md"),
        "# RFD 043: More Order\n\n- **Status**: Accepted\n\n## Implementation Plan\n",
    )
    .unwrap();
    fs::write(
        root.path().join("docs/rfd/044-discussed.md"),
        "# RFD 044: Discussed\n\n- **Status**: Discussion\n",
    )
    .unwrap();

    root
}

/// The init for running `args`, attributed to a configured user, on a board
/// whose ticket vocabulary defines the tracking label.
fn init_by_jane(root: &Utf8Path, args: &[&str]) -> HostToPlugin {
    let mut message = init_message(root, args);
    message.config = json!({
        "user": { "name": "Jane Doe <jane@example.com>" },
        "plugins": { "command": { "ticket": { "options": {
            "labels": { "type": { "description": "The type", "values": ["tracking"] } }
        } } } }
    });

    HostToPlugin::Init(Box::new(message))
}

/// The host's answers to a plan query that finished with `data`.
fn plan_answered(data: Value) -> [HostToPlugin; 2] {
    [
        HostToPlugin::Created(CreatedResponse {
            id: Some("plan".to_owned()),
            conversation: "jp-c17000000001".to_owned(),
        }),
        HostToPlugin::QueryComplete(QueryCompleteResponse {
            id: Some("plan".to_owned()),
            conversation: "jp-c17000000001".to_owned(),
            data: Some(data),
        }),
    ]
}

fn one_phase() -> Value {
    json!({ "phases": [{ "title": "Order things", "summary": "Keeps them in order." }] })
}

/// The queries the plugin asked the host to run.
fn queries(messages: &[PluginToHost]) -> Vec<&QueryRequest> {
    messages
        .iter()
        .filter_map(|message| match message {
            PluginToHost::Query(query) => Some(query),
            _ => None,
        })
        .collect()
}

/// The questions the plugin asked the host to put to the user.
fn questions(messages: &[PluginToHost]) -> Vec<&ComposeRequest> {
    messages
        .iter()
        .filter_map(|message| match message {
            PluginToHost::Compose(request) => Some(request),
            _ => None,
        })
        .collect()
}

/// The tracking ticket RFD 043 got, as written.
fn tracking_ticket(root: &Utf8TempDir) -> Option<String> {
    fs::read_dir(root.path().join("docs/ticket"))
        .unwrap()
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .find(|name| name.ends_with("-more-order.md"))
        .map(|name| fs::read_to_string(root.path().join("docs/ticket").join(name)).unwrap())
}

/// The plan is read by a turn the host runs, and the ticket's id is all that
/// lands on stdout, for a script to pick up.
#[test]
fn track_reads_the_plan_through_the_host() {
    let root = trackable();
    let [created, complete] = plan_answered(one_phase());

    let messages = exchange_all(&[
        init_by_jane(root.path(), &["track", "43"]),
        created,
        complete,
    ]);

    let [query] = queries(&messages)[..] else {
        panic!("expected one query: {messages:?}");
    };
    assert!(query.new);
    assert!(query.schema.is_some());
    assert!(
        query
            .content
            .contains("<document path=\"docs/rfd/043-more-order.md\">")
    );

    let (text, code, reason) = printed(&messages);
    assert_eq!((code, reason), (0, None));
    assert!(text.starts_with("T-") && text.ends_with('\n'), "{text}");
    assert!(
        chrome(&messages)[0].starts_with("Filed docs/ticket/"),
        "{messages:?}"
    );

    let ticket = tracking_ticket(&root).unwrap();
    assert!(
        ticket.contains("- **Authors**: Jane Doe <jane@example.com>\n"),
        "{ticket}"
    );
    assert!(
        ticket.ends_with("## Implementation plan\n\n- **Order things**: Keeps them in order.\n"),
        "{ticket}"
    );
}

#[test]
fn a_query_the_host_refuses_leaves_the_plan_pending() {
    let root = trackable();

    let messages = exchange_all(&[
        init_by_jane(root.path(), &["track", "43"]),
        HostToPlugin::Error(ErrorResponse {
            id: Some("plan".to_owned()),
            request: Some("query".to_owned()),
            message: "no provider is configured".to_owned(),
        }),
    ]);

    let (_, code, _) = printed(&messages);
    assert_eq!(code, 0);
    assert!(
        chrome(&messages).iter().any(|line| line.starts_with(
            "The Implementation Plan of RFD 043 could not be read: no provider is configured."
        )),
        "{messages:?}"
    );
    assert!(
        tracking_ticket(&root)
            .unwrap()
            .ends_with("<!-- rfd-plan: pending -->\n")
    );
}

/// Interrupting the plan's read files no ticket without its plan.
#[test]
fn an_interrupted_track_files_nothing() {
    let root = trackable();

    let messages = exchange_all(&[
        init_by_jane(root.path(), &["track", "43"]),
        HostToPlugin::Shutdown,
    ]);

    let (_, code, reason) = printed(&messages);
    assert_eq!((code, reason.as_deref()), (1, Some("Interrupted.")));
    assert_eq!(tracking_ticket(&root), None);
}

/// The promotion is reported before the question about it, and a yes files the
/// ticket with its plan read.
#[test]
fn promoting_to_accepted_offers_the_tracking_ticket() {
    let root = trackable();
    fs::write(
        root.path().join("docs/rfd/043-more-order.md"),
        "# RFD 043: More Order\n\n- **Status**: Discussion\n",
    )
    .unwrap();
    let [created, complete] = plan_answered(one_phase());

    let messages = exchange_all(&[
        init_by_jane(root.path(), &["promote", "43"]),
        HostToPlugin::Composed(ComposeResponse {
            id: None,
            text: Some("file".to_owned()),
            values: vec![],
        }),
        created,
        complete,
    ]);

    let promoted = messages
        .iter()
        .position(|message| {
            matches!(message, PluginToHost::Print(print)
                if print.text == "docs/rfd/043-more-order.md: Discussion -> Accepted\n")
        })
        .expect("the promotion is reported");
    let asked = messages
        .iter()
        .position(|message| matches!(message, PluginToHost::Compose(_)))
        .expect("the tracking ticket is offered");
    assert!(promoted < asked, "{messages:?}");

    let [question] = questions(&messages)[..] else {
        panic!("expected one question: {messages:?}");
    };
    assert_eq!(question.message, "File the tracking ticket for RFD 043?");
    assert!(
        matches!(&question.mode, ComposeMode::Select { default: Some(value), .. } if value == "file")
    );

    let (text, code, _) = printed(&messages);
    assert_eq!(code, 0);
    assert!(text.contains("\nReview T-"), "{text}");
    assert!(
        text.ends_with(" before committing it; `jp ticket board` shows the board.\n"),
        "{text}"
    );
    assert!(
        tracking_ticket(&root)
            .unwrap()
            .contains("- **Order things**: Keeps them in order.")
    );
}

/// No, and no terminal to ask on, both leave the ticket for later: accepting an
/// RFD is not a commitment to start building it.
#[test]
fn a_declined_or_unanswerable_offer_files_nothing() {
    for answer in [Some("skip".to_owned()), None] {
        let root = trackable();

        let messages = exchange_all(&[
            init_by_jane(root.path(), &["promote", "44"]),
            HostToPlugin::Composed(ComposeResponse {
                id: None,
                text: answer.clone(),
                values: vec![],
            }),
        ]);

        let (_, code, _) = printed(&messages);
        assert_eq!(code, 0, "{answer:?}");
        assert_eq!(
            chrome(&messages),
            ["Skipped the tracking ticket. File it later with `jp rfd track 044`.\n"],
            "{answer:?}"
        );
        assert!(queries(&messages).is_empty(), "{answer:?}");
        assert!(
            fs::read_to_string(root.path().join("docs/rfd/044-discussed.md"))
                .unwrap()
                .contains("- **Status**: Accepted\n")
        );
    }
}

/// A ticket that can't be filed doesn't undo the promotion before it.
#[test]
fn a_tracking_ticket_that_cannot_be_filed_leaves_the_promotion() {
    let root = trackable();
    let mut unlabelled = init_message(root.path(), &["promote", "44"]);
    unlabelled.config = json!({ "user": { "name": "Jane Doe <jane@example.com>" } });

    let messages = exchange_all(&[
        HostToPlugin::Init(Box::new(unlabelled)),
        HostToPlugin::Composed(ComposeResponse {
            id: None,
            text: Some("file".to_owned()),
            values: vec![],
        }),
    ]);

    let (_, code, _) = printed(&messages);
    assert_eq!(code, 0);
    let warning = "Could not file the tracking ticket: Tracking tickets carry the label \
                   `type=tracking`, which the ticket vocabulary doesn't define. Add `tracking` to \
                   the values of `plugins.command.ticket.options.labels.type`.\nRun `jp rfd track \
                   044` to retry.\n";
    assert_eq!(chrome(&messages), [warning]);
}

/// Only acceptance offers the ticket, and a project without tickets is never
/// asked.
#[test]
fn other_promotions_offer_nothing() {
    let root = trackable();
    let messages = exchange(&init_by_jane(root.path(), &["promote", "42"]));
    assert!(questions(&messages).is_empty(), "{messages:?}");
    assert_eq!(printed(&messages).1, 0);

    let root = trackable();
    fs::remove_dir_all(root.path().join("docs/ticket")).unwrap();
    let messages = exchange(&init_by_jane(root.path(), &["promote", "44"]));
    assert!(questions(&messages).is_empty(), "{messages:?}");
    assert_eq!(printed(&messages).1, 0);
}

#[test]
fn start_and_stop_move_the_tracking_ticket() {
    let root = trackable();
    let [created, complete] = plan_answered(one_phase());

    let started = exchange_all(&[
        init_by_jane(root.path(), &["start", "43"]),
        created,
        complete,
    ]);
    let (text, code, _) = printed(&started);
    assert_eq!(code, 0);
    assert!(text.ends_with(": Todo -> In Progress\n"), "{text}");

    let stopped = exchange(&init_by_jane(root.path(), &["stop", "43"]));
    let (text, code, _) = printed(&stopped);
    assert_eq!(code, 0);
    assert!(text.ends_with(": In Progress -> Todo\n"), "{text}");
    assert!(queries(&stopped).is_empty());
}

/// `--no-plan` files the ticket without waiting on a model, and a later `track`
/// reads the plan into it.
#[test]
fn start_without_the_plan_leaves_it_for_track() {
    let root = trackable();

    let started = exchange(&init_by_jane(root.path(), &["start", "43", "--no-plan"]));
    assert!(queries(&started).is_empty(), "{started:?}");
    let (text, code, _) = printed(&started);
    assert_eq!(code, 0);
    assert!(text.ends_with(": Todo -> In Progress\n"), "{text}");
    let note = "The tracking ticket holds `<!-- rfd-plan: pending -->` in place of the \
                Implementation Plan of RFD 043 until `jp rfd track 043` reads it.\n";
    assert!(chrome(&started).contains(&note), "{started:?}");
    assert!(
        tracking_ticket(&root)
            .unwrap()
            .ends_with("<!-- rfd-plan: pending -->\n")
    );

    let [created, complete] = plan_answered(one_phase());
    let tracked = exchange_all(&[
        init_by_jane(root.path(), &["track", "43"]),
        created,
        complete,
    ]);
    assert_eq!(printed(&tracked).1, 0);

    let ticket = tracking_ticket(&root).unwrap();
    assert!(ticket.contains("- **Status**: In Progress\n"), "{ticket}");
    assert!(
        ticket.ends_with("## Implementation plan\n\n- **Order things**: Keeps them in order.\n"),
        "{ticket}"
    );
}
