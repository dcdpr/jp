use camino_tempfile::Utf8TempDir;
use clap::CommandFactory;
use jp_plugin::message::WorkspaceInfo;

use super::*;

fn parse(args: &[&str]) -> Result<Args, clap::Error> {
    let args: Vec<String> = args.iter().map(|arg| (*arg).to_owned()).collect();

    Args::try_parse_from(std::iter::once("jp ticket".to_owned()).chain(with_show_alias(&args)))
}

/// Every command that writes needs an author; the tests supply one rather than
/// depending on the machine's git config.
fn run_command(dir: &Utf8TempDir, command: Command) -> Result<Output, String> {
    execute(
        dir.path(),
        command,
        &serde_json::json!({ "user": { "name": "tester" } }),
    )
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

fn init(root: &Utf8Path, args: &[&str]) -> HostToPlugin {
    init_at(jp_plugin::PROTOCOL_VERSION, root, args)
}

fn init_at(version: u32, root: &Utf8Path, args: &[&str]) -> HostToPlugin {
    HostToPlugin::Init(InitMessage {
        version,
        workspace: WorkspaceInfo {
            root: root.to_path_buf(),
            storage: root.join(".jp"),
            id: "test".to_owned(),
        },
        paths: jp_plugin::message::PathsInfo::default(),
        config: serde_json::json!({ "user": { "name": "tester" } }),
        options: serde_json::Map::new(),
        args: args.iter().map(|arg| (*arg).to_owned()).collect(),
        log_level: 0,
    })
}

#[test]
fn cli_definition_is_valid() {
    Args::command().debug_assert();
}

#[test]
fn add_takes_a_kind_and_a_title_and_defaults_the_author() {
    let args = parse(&["add", "bug", "Tool call header misaligned"]).unwrap();

    assert_eq!(args.dir, None);
    match args.command {
        Command::Add {
            kind,
            title,
            author,
            body,
            implements,
            source,
        } => {
            assert_eq!(kind, Some(Kind::Bug));
            assert_eq!(title.as_deref(), Some("Tool call header misaligned"));
            assert_eq!(author, None);
            assert_eq!(body, None);
            assert_eq!(implements, None);
            assert_eq!(source, None);
        }
        other => panic!("expected add, got {other:?}"),
    }
}

/// The whole point of `--source` is that a sweep can run on a timer: the second
/// add finds the ticket the first one wrote and writes nothing.
#[test]
fn adding_the_same_source_twice_files_one_ticket() {
    let dir = Utf8TempDir::new().unwrap();
    let add = || {
        run_command(&dir, Command::Add {
            kind: Some(Kind::Feature),
            title: Some("Auto import notes".to_owned()),
            author: Some("jean".to_owned()),
            body: Some("Caught on a phone.".to_owned()),
            implements: None,
            source: Some("bear:E340A2C4-8671-4233-860B-6AEFF7CB00D8".to_owned()),
        })
        .unwrap()
    };

    let first = add();
    let second = add();

    assert_eq!(
        first.text,
        format!(
            "Created {}/0001-auto-import-notes.md (T0001) from \
             bear:E340A2C4-8671-4233-860B-6AEFF7CB00D8\n",
            dir.path()
        )
    );
    assert_eq!(
        second.text,
        "bear:E340A2C4-8671-4233-860B-6AEFF7CB00D8 is already T0001\n"
    );

    let entries = store::list(dir.path()).unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(
        entries[0]
            .ticket
            .as_ref()
            .unwrap()
            .metadata
            .source
            .as_deref(),
        Some("bear:E340A2C4-8671-4233-860B-6AEFF7CB00D8")
    );
}

/// A ticket the sweep filed is written up here afterwards, and the next sweep
/// must not undo that.
#[test]
fn a_sourced_ticket_survives_being_rewritten() {
    let dir = Utf8TempDir::new().unwrap();
    let add = |title: &str, body: &str| {
        run_command(&dir, Command::Add {
            kind: Some(Kind::Feature),
            title: Some(title.to_owned()),
            author: Some("jean".to_owned()),
            body: Some(body.to_owned()),
            implements: None,
            source: Some("bear:note-1".to_owned()),
        })
        .unwrap()
    };

    add("rough note", "half a thought");
    run_command(&dir, Command::Edit {
        id: Some(TicketId::new(1)),
        title: Some("Sharpened title".to_owned()),
        body: Some("Written up properly.".to_owned()),
        kind: None,
        status: Some(Status::InProgress),
    })
    .unwrap();

    add("rough note", "half a thought, edited in the note");

    let entries = store::list(dir.path()).unwrap();
    let ticket = entries[0].ticket.as_ref().unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(ticket.title, "Sharpened title");
    assert_eq!(ticket.description, "Written up properly.");
    assert_eq!(ticket.metadata.status, Status::InProgress);
}

/// Sourced text comes from outside the repository, so it is escaped before it
/// reaches a file the site compiles as Vue.
#[test]
fn sourced_content_is_escaped_but_hand_written_content_is_not() {
    let dir = Utf8TempDir::new().unwrap();
    let add = |source: Option<&str>| {
        run_command(&dir, Command::Add {
            kind: Some(Kind::Chore),
            title: Some("Title".to_owned()),
            author: Some("jean".to_owned()),
            body: Some("Uses {{ interpolation }} and <div>tags</div>.".to_owned()),
            implements: None,
            source: source.map(ToOwned::to_owned),
        })
        .unwrap()
    };

    add(Some("bear:note-1"));
    add(None);

    let entries = store::list(dir.path()).unwrap();
    assert_eq!(
        entries[0].ticket.as_ref().unwrap().description,
        "Uses &#123;&#123; interpolation &#125;&#125; and &lt;div>tags&lt;/div>."
    );
    assert_eq!(
        entries[1].ticket.as_ref().unwrap().description,
        "Uses {{ interpolation }} and <div>tags</div>."
    );
}

#[test]
fn a_malformed_source_is_refused_before_anything_is_written() {
    let dir = Utf8TempDir::new().unwrap();

    let error = run_command(&dir, Command::Add {
        kind: Some(Kind::Chore),
        title: Some("Title".to_owned()),
        author: Some("jean".to_owned()),
        body: None,
        implements: None,
        source: Some("nocolon".to_owned()),
    })
    .unwrap_err();

    assert_eq!(
        error,
        "`nocolon` is not a `scheme:id` pair naming where it came from."
    );
    assert!(store::list(dir.path()).unwrap().is_empty());
}

/// A sweep passes the title after `--` so a note titled "- fix the thing" isn't
/// read as a flag.
#[test]
fn add_takes_a_title_after_a_double_dash() {
    let args = parse(&[
        "add",
        "bug",
        "--source",
        "bear:note-1",
        "--",
        "-fix the thing",
    ])
    .unwrap();

    match args.command {
        Command::Add {
            kind,
            title,
            source,
            ..
        } => {
            assert_eq!(kind, Some(Kind::Bug));
            assert_eq!(title.as_deref(), Some("-fix the thing"));
            assert_eq!(source.as_deref(), Some("bear:note-1"));
        }
        other => panic!("expected add, got {other:?}"),
    }
}

#[test]
fn add_rejects_an_unknown_kind() {
    let error = parse(&["add", "task", "Title"]).unwrap_err();

    assert!(error.to_string().contains("`task` is not a valid `Kind`"));
    assert!(
        error.use_stderr(),
        "a bad kind is a failure, not help output"
    );
}

#[test]
fn comment_takes_an_author_and_a_reply_target() {
    let args = parse(&["comment", "T0042", "--author", "jp", "--re", "1"]).unwrap();

    match args.command {
        Command::Comment {
            id,
            author,
            re,
            body,
        } => {
            assert_eq!(id, Some(TicketId::new(42)));
            assert_eq!(author.as_deref(), Some("jp"));
            assert_eq!(re, Some(1));
            assert_eq!(body, None);
        }
        other => panic!("expected comment, got {other:?}"),
    }
}

/// A bare id is the common case, so it reads as `show`.
#[test]
fn a_bare_id_is_a_show() {
    for args in [&["42"][..], &["042"], &["T0042"], &["t42"]] {
        match parse(args).unwrap().command {
            Command::Show { id, json } => {
                assert_eq!(id, Some(TicketId::new(42)), "{args:?}");
                assert!(!json);
            }
            other => panic!("expected show for {args:?}, got {other:?}"),
        }
    }

    match parse(&["T0042", "--json"]).unwrap().command {
        Command::Show { json, .. } => assert!(json),
        other => panic!("expected show, got {other:?}"),
    }
}

/// No subcommand name parses as an id, so the alias can't shadow one.
#[test]
fn subcommands_are_not_mistaken_for_ids() {
    for name in [
        "add", "comment", "close", "show", "promote", "import", "list",
    ] {
        assert_eq!(
            with_show_alias(&[name.to_owned()]),
            vec![name.to_owned()],
            "{name} was read as an id"
        );
    }
}

#[test]
fn the_author_falls_back_through_jp_then_git_then_the_environment() {
    let jp = serde_json::json!({ "user": { "name": "Jean Mertz" } });

    assert_eq!(
        resolve_author(Some("  Someone Else  ".to_owned()), &jp),
        Ok("Someone Else".to_owned())
    );
    // The configured name picks up git's email, so a ticket's author line reads
    // like an RFD's. The email itself depends on the machine.
    let resolved = resolve_author(None, &jp).unwrap();
    assert!(resolved.starts_with("Jean Mertz"), "{resolved}");

    // An empty or absent `user.name` falls through to git or `$USER`, both of
    // which exist in any environment this runs in.
    let blank = serde_json::json!({ "user": { "name": "   " } });
    assert!(resolve_author(None, &blank).is_ok());
    assert!(resolve_author(None, &serde_json::Value::Null).is_ok());
    assert!(resolve_author(Some(String::new()), &jp).is_ok());
}

#[test]
fn composed_text_splits_into_a_title_and_a_body() {
    // A single line, however many blank lines follow it, is a title alone.
    assert_eq!(
        Composition::read("Tool call header misaligned"),
        Composition::Title("Tool call header misaligned".to_owned())
    );
    assert_eq!(
        Composition::read("Tool call header misaligned\n\n\n"),
        Composition::Title("Tool call header misaligned".to_owned())
    );

    // Subject, blank line, body.
    assert_eq!(
        Composition::read("Header misaligned\n\nIt wraps one column early.\n"),
        Composition::TitleAndBody {
            title: "Header misaligned".to_owned(),
            body: "It wraps one column early.".to_owned(),
        }
    );

    // Prose running straight on from the first line has no subject.
    assert_eq!(
        Composition::read("The header wraps one column\nearly, below 80 columns."),
        Composition::Body("The header wraps one column\nearly, below 80 columns.".to_owned())
    );

    assert_eq!(Composition::read(""), Composition::Empty);
    assert_eq!(Composition::read("  \n\n"), Composition::Empty);
}

#[test]
fn list_filters_are_optional() {
    let args = parse(&["list", "--status", "In Progress"]).unwrap();

    match args.command {
        Command::List { status, kind, json } => {
            assert_eq!(status, Some(Status::InProgress));
            assert_eq!(kind, None);
            assert!(!json);
        }
        other => panic!("expected list, got {other:?}"),
    }
}

/// `--help` reaches us as an error, but it is output, not a failure.
#[test]
fn help_is_not_a_failure() {
    let error = parse(&["--help"]).unwrap_err();

    assert!(!error.use_stderr());
    assert!(error.to_string().contains("jp ticket"));
}

#[test]
fn the_ticket_directory_follows_the_workspace_root() {
    let root = Utf8Path::new("/repo");

    assert_eq!(resolve_dir(root, None), "/repo/docs/ticket");
    assert_eq!(
        resolve_dir(root, Some(Utf8Path::new("tmp/t"))),
        "/repo/tmp/t"
    );
    assert_eq!(
        resolve_dir(root, Some(Utf8Path::new("/elsewhere"))),
        "/elsewhere"
    );
}

#[test]
fn commands_run_against_the_resolved_directory() {
    let dir = Utf8TempDir::new().unwrap();

    let created = run_command(&dir, Command::Add {
        kind: Some(Kind::Bug),
        title: Some("Tool call header misaligned".to_owned()),
        author: Some("Jean Mertz".to_owned()),
        body: Some("The header renders one column left of the body.".to_owned()),
        implements: None,
        source: None,
    })
    .unwrap();
    assert!(created.text.contains("(T0001)"), "{}", created.text);
    assert!(created.warnings.is_empty());

    let commented = run_command(&dir, Command::Comment {
        id: Some(TicketId::new(1)),
        author: Some("jean".to_owned()),
        re: None,
        body: Some("Reproduced at 72 columns.".to_owned()),
    })
    .unwrap();
    assert_eq!(commented.text, "Added T0001#1 by jean\n");

    let listed = run_command(&dir, Command::List {
        status: None,
        kind: None,
        json: false,
    })
    .unwrap();
    assert_eq!(
        listed.text,
        "T0001  Todo         Bug      Tool call header misaligned\n"
    );

    let closed = run_command(&dir, Command::Close {
        id: Some(TicketId::new(1)),
    })
    .unwrap();
    assert!(closed.text.contains("Todo -> Done"), "{}", closed.text);

    let json = run_command(&dir, Command::List {
        status: Some(Status::Done),
        kind: None,
        json: true,
    })
    .unwrap();
    let rows: Vec<serde_json::Value> = serde_json::from_str(&json.text).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["id"], "T0001");
    assert_eq!(rows[0]["status"], "Done");
    assert_eq!(rows[0]["comments"], 1);
}

#[test]
fn an_empty_comment_is_refused() {
    let dir = Utf8TempDir::new().unwrap();

    let error = run_command(&dir, Command::Comment {
        id: Some(TicketId::new(1)),
        author: Some("jp".to_owned()),
        re: None,
        body: Some("   ".to_owned()),
    })
    .unwrap_err();

    assert_eq!(error, "Refusing to write an empty comment; pass --body.");
}

#[test]
fn a_missing_ticket_is_an_error() {
    let dir = Utf8TempDir::new().unwrap();

    let error = run_command(&dir, Command::Close {
        id: Some(TicketId::new(9)),
    })
    .unwrap_err();

    assert_eq!(error, "No ticket T0009.");
}

/// A reply names what it answers, as the MCP renderer does.
/// Without it the human output flattens a thread into a flat list.
#[test]
fn show_names_the_comment_a_reply_answers() {
    let dir = Utf8TempDir::new().unwrap();
    run_command(&dir, Command::Add {
        kind: Some(Kind::Bug),
        title: Some("Tool call header misaligned".to_owned()),
        author: Some("john".to_owned()),
        body: None,
        implements: None,
        source: None,
    })
    .unwrap();
    run_command(&dir, Command::Comment {
        id: Some(TicketId::new(1)),
        author: Some("john".to_owned()),
        re: None,
        body: Some("Reproduced at 72 columns.".to_owned()),
    })
    .unwrap();
    run_command(&dir, Command::Comment {
        id: Some(TicketId::new(1)),
        author: Some("jp".to_owned()),
        re: Some(1),
        body: Some("The wrap calculation is off.".to_owned()),
    })
    .unwrap();

    let shown = run_command(&dir, Command::Show {
        id: Some(TicketId::new(1)),
        json: false,
    })
    .unwrap();

    assert!(
        shown.text.contains("## T0001#1 \u{2014} john at "),
        "{}",
        shown.text
    );
    assert!(
        shown.text.contains("## T0001#2 \u{2014} jp at ")
            && shown.text.contains(", replying to T0001#1\n"),
        "{}",
        shown.text
    );
}

/// `show --json` flattens the ticket, so its fields sit at the top level.
///
/// Two consumers depend on that shape: the docs dev server spreads this object
/// into its response and the edit form reads `description` from there, and
/// `just ticket-promote` reads `.description` and `.metadata.promoted_to` with
/// `jq`.
/// Nesting the ticket would break both silently.
#[test]
fn show_json_keeps_the_ticket_fields_at_the_top_level() {
    let dir = Utf8TempDir::new().unwrap();
    run_command(&dir, Command::Add {
        kind: Some(Kind::Bug),
        title: Some("Tool call header misaligned".to_owned()),
        author: Some("john".to_owned()),
        body: Some("The header renders one column left of the body.".to_owned()),
        implements: None,
        source: None,
    })
    .unwrap();

    let shown = run_command(&dir, Command::Show {
        id: Some(TicketId::new(1)),
        json: true,
    })
    .unwrap();
    let value: Value = serde_json::from_str(&shown.text).unwrap();

    assert_eq!(value["id"], "T0001");
    assert_eq!(
        value["description"],
        "The header renders one column left of the body."
    );
    assert_eq!(value["metadata"]["status"], "Todo");
    assert!(
        value.get("ticket").is_none(),
        "the ticket is flattened, not nested: {value}"
    );
}

#[test]
fn describe_reports_the_subcommand_it_serves() {
    let messages = exchange(&HostToPlugin::Describe);

    match messages.as_slice() {
        [PluginToHost::Describe(describe)] => {
            assert_eq!(describe.name, "ticket");
            assert_eq!(describe.command, ["ticket"]);
            assert!(
                describe
                    .help
                    .as_ref()
                    .is_some_and(|help| help.contains("add"))
            );
        }
        other => panic!("expected a single describe, got {other:?}"),
    }
}

#[test]
fn a_run_reports_ready_then_output_then_exit() {
    let dir = Utf8TempDir::new().unwrap();
    let messages = exchange(&init(dir.path(), &[
        "add",
        "bug",
        "Tool call header misaligned",
    ]));

    match messages.as_slice() {
        [
            PluginToHost::Ready(_),
            PluginToHost::Print(print),
            PluginToHost::Exit(exit),
        ] => {
            assert!(print.text.contains("(T0001)"), "{}", print.text);
            assert_eq!(print.channel, "content");
            assert_eq!(exit.code, 0);
        }
        other => panic!("unexpected exchange: {other:?}"),
    }

    assert!(
        dir.path()
            .join("docs/ticket/0001-tool-call-header-misaligned.md")
            .exists()
    );
}

/// A plugin newer than its host can't ask for anything interactive, so it says
/// so and stops — without claiming ready — rather than hanging on a reply the
/// host can't produce.
#[test]
fn an_older_host_is_refused_up_front() {
    let dir = Utf8TempDir::new().unwrap();
    let messages = exchange(&init_at(REQUIRED_PROTOCOL - 1, dir.path(), &["list"]));

    match messages.as_slice() {
        [PluginToHost::Exit(exit)] => {
            assert_eq!(exit.code, 1);
            assert!(
                exit.reason
                    .as_ref()
                    .is_some_and(|reason| reason.contains("needs `jp` protocol 2")),
                "{:?}",
                exit.reason
            );
        }
        other => panic!("unexpected exchange: {other:?}"),
    }
}

#[test]
fn a_bad_argument_exits_non_zero_with_a_reason() {
    let dir = Utf8TempDir::new().unwrap();
    let messages = exchange(&init(dir.path(), &["close", "not-an-id"]));

    match messages.as_slice() {
        [PluginToHost::Ready(_), PluginToHost::Exit(exit)] => {
            assert_eq!(exit.code, 1);
            assert!(
                exit.reason
                    .as_ref()
                    .is_some_and(|reason| reason.contains("not-an-id")),
                "{:?}",
                exit.reason
            );
        }
        other => panic!("unexpected exchange: {other:?}"),
    }
}

/// An unreadable ticket is reported to the log, not mixed into stdout where it
/// would corrupt `--json`.
#[test]
fn unreadable_tickets_are_warned_about_separately() {
    let dir = Utf8TempDir::new().unwrap();
    run_command(&dir, Command::Add {
        kind: Some(Kind::Chore),
        title: Some("Readable".to_owned()),
        author: Some("jean".to_owned()),
        body: None,
        implements: None,
        source: None,
    })
    .unwrap();
    std::fs::write(dir.path().join("0009-mangled.md"), "no heading here\n").unwrap();

    let listed = run_command(&dir, Command::List {
        status: None,
        kind: None,
        json: true,
    })
    .unwrap();

    let rows: Vec<serde_json::Value> = serde_json::from_str(&listed.text).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(listed.warnings.len(), 1);
    assert!(listed.warnings[0].contains("0009-mangled.md"));
}
