use camino_tempfile::Utf8TempDir;
use clap::CommandFactory;
use jp_plugin::message::{
    EditFilesRequest, EditedResponse, ErrorResponse, OutputFormat, ToolAction, WorkspaceInfo,
};
use serde_json::{Map, json};

use super::*;

fn parse(args: &[&str]) -> Result<Args, clap::Error> {
    let args: Vec<String> = args.iter().map(|arg| (*arg).to_owned()).collect();

    Args::try_parse_from(std::iter::once("jp ticket".to_owned()).chain(with_show_alias(&args)))
}

/// Every command that writes needs an author; the tests supply one rather than
/// depending on the machine's git config.
///
/// The board defines [`labelled`]'s vocabulary, so label writes have something
/// to check against.
fn run_command(dir: &Utf8TempDir, command: Command) -> Result<Output, String> {
    run_with_options(dir.path(), command, &labelled())
}

/// The plugin's default options, with a label vocabulary.
fn labelled() -> Options {
    let options = json!({
        "labels": {
            "client": { "description": "The client.", "values": ["cli", "macos"] },
            "package": {
                "description": "The crate.",
                "values": ["jp_cli", "jp_config"],
                "retired": ["jp_legacy"]
            }
        }
    });
    let Value::Object(options) = options else {
        unreachable!("options are an object");
    };

    Options::read(&options).unwrap()
}

/// Run a command against `dir` as both the workspace root and the ticket
/// directory, with the plugin's default options.
fn run_in(dir: &Utf8Path, command: Command) -> Result<Output, String> {
    run_with_options(dir, command, &Options::default())
}

fn run_with_options(dir: &Utf8Path, command: Command, options: &Options) -> Result<Output, String> {
    run_with(dir, command, options, OutputFormat::default())
}

/// [`run_command`], as `jp -F json` runs it.
fn run_json(dir: &Utf8TempDir, command: Command) -> Result<Output, String> {
    run_with(dir.path(), command, &labelled(), OutputFormat::Json)
}

fn run_with(
    dir: &Utf8Path,
    command: Command,
    options: &Options,
    format: OutputFormat,
) -> Result<Output, String> {
    let config = serde_json::json!({
      "user": {
        "name": "tester"
      }
    });

    execute(
        &Context {
            dir: dir.to_path_buf(),
            root: dir,
            config: &config,
            options,
            format,
        },
        command,
    )
}

/// Write a ticket under a fixed id at `status`, so a test can name it in the
/// board's order file.
fn write_ticket(dir: &Utf8Path, body: &str, title: &str, status: Status) -> TicketId {
    let document = ticket::render::ticket(&NewTicket {
        title,
        authors: "john",
        date: "2026-08-05",
        implements: None,
        labels: &Labels::default(),
        description: "",
    });
    let document = ticket::render::set_metadata(&document, "Status", &status.to_string()).unwrap();
    std::fs::create_dir_all(dir).unwrap();
    std::fs::write(dir.join(format!("{body}-slug.md")), document).unwrap();

    body.parse().unwrap()
}

/// File a labelled ticket and hand back its id.
fn add_labelled(dir: &Utf8TempDir, title: &str, labels: &[&str]) -> TicketId {
    run_command(dir, Command::Add {
        title: Some(title.to_owned()),
        author: Some("john".to_owned()),
        body: None,
        implements: None,
        labels: labels.iter().map(|label| (*label).to_owned()).collect(),
    })
    .unwrap();

    store::list(dir.path())
        .unwrap()
        .into_iter()
        .find(|entry| entry.ticket.as_ref().is_ok_and(|t| t.title == title))
        .expect("the ticket that was just filed")
        .id
}

/// Run git in `dir`, failing the test with its stderr rather than a bare status
/// code.
fn git(dir: &Utf8Path, args: &[&str]) -> String {
    let output = std::process::Command::new("git")
        .current_dir(dir)
        .args(args)
        .output()
        .unwrap_or_else(|error| panic!("git {args:?}: {error}"));

    assert!(
        output.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    String::from_utf8(output.stdout).unwrap()
}

/// A repository with `docs/ticket/`, one commit on `main`, and nothing else.
///
/// The config is set locally so the machine's identity, default branch name,
/// and signing setup can't reach in and change what the test does.
fn git_repo() -> Utf8TempDir {
    let dir = Utf8TempDir::new().unwrap();
    let root = dir.path();

    git(root, &["init", "--initial-branch=main"]);
    git(root, &["config", "user.name", "tester"]);
    git(root, &["config", "user.email", "tester@example.com"]);
    git(root, &["config", "commit.gpgsign", "false"]);

    std::fs::create_dir_all(root.join("docs/ticket")).unwrap();
    std::fs::write(root.join("README.md"), "# Fixture\n").unwrap();
    commit(root, "Initial commit", None);

    dir
}

/// Stage everything and commit, optionally at a fixed instant.
///
/// `at` fixes both dates so a test that reads a commit's timestamp back gets
/// the value it wrote rather than whatever the clock said.
fn commit(root: &Utf8Path, message: &str, at: Option<&str>) {
    git(root, &["add", "-A"]);

    let mut command = std::process::Command::new("git");
    command.current_dir(root).args(["commit", "-m", message]);
    if let Some(at) = at {
        command
            .env("GIT_AUTHOR_DATE", at)
            .env("GIT_COMMITTER_DATE", at);
    }

    let output = command.output().unwrap();
    assert!(
        output.status.success(),
        "git commit failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
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

/// Drive a full exchange: one message in, then a scripted reply to each request
/// the plugin makes.
///
/// The plugin reads its replies from the same stream, so a composition is just
/// more lines of input.
fn exchange_with(message: &HostToPlugin, replies: &[HostToPlugin]) -> Vec<PluginToHost> {
    let mut input = serde_json::to_string(message).unwrap() + "\n";
    for reply in replies {
        input.push_str(&serde_json::to_string(reply).unwrap());
        input.push('\n');
    }

    let mut output = vec![];
    run(input.as_bytes(), &mut output).unwrap();

    String::from_utf8(output)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

/// A composed answer carrying `text`.
fn composed(text: &str) -> HostToPlugin {
    HostToPlugin::Composed(jp_plugin::message::ComposeResponse {
        id: None,
        text: Some(text.to_owned()),
        values: vec![],
    })
}

fn init(root: &Utf8Path, args: &[&str]) -> HostToPlugin {
    init_at(jp_plugin::PROTOCOL_VERSION, root, args)
}

fn init_at(version: u32, root: &Utf8Path, args: &[&str]) -> HostToPlugin {
    HostToPlugin::Init(Box::new(init_message(version, root, args)))
}

fn init_message(version: u32, root: &Utf8Path, args: &[&str]) -> InitMessage {
    InitMessage {
        version,
        workspace: WorkspaceInfo {
            root: root.to_path_buf(),
            storage: root.join(".jp"),
            id: "test".to_owned(),
        },
        paths: jp_plugin::message::PathsInfo::default(),
        config: serde_json::json!({
          "user": {
            "name": "tester"
          }
        }),
        options: serde_json::Map::new(),
        args: args.iter().map(|arg| (*arg).to_owned()).collect(),
        log_level: 0,
        output_format: OutputFormat::default(),
        tool: None,
    }
}

/// An `init` carrying the plugin's `options`.
fn init_with_options(root: &Utf8Path, args: &[&str], options: serde_json::Value) -> HostToPlugin {
    let mut message = init_message(jp_plugin::PROTOCOL_VERSION, root, args);
    let serde_json::Value::Object(options) = options else {
        panic!("options are an object");
    };
    message.options = options;

    HostToPlugin::Init(Box::new(message))
}

#[test]
fn cli_definition_is_valid() {
    Args::command().debug_assert();
}

#[test]
fn add_takes_a_title_and_defaults_the_author() {
    let args = parse(&["add", "Tool call header misaligned"]).unwrap();

    assert_eq!(args.dir, None);
    match args.command {
        Command::Add {
            title,
            author,
            body,
            implements,
            labels,
        } => {
            assert_eq!(title.as_deref(), Some("Tool call header misaligned"));
            assert_eq!(author, None);
            assert_eq!(body, None);
            assert_eq!(implements, None);
            assert!(labels.is_empty());
        }
        other => panic!("expected add, got {other:?}"),
    }
}

/// A title is one argument, so `add bug "Title"` is refused rather than filing
/// a ticket titled `bug`.
#[test]
fn add_takes_one_title() {
    let error = parse(&["add", "bug", "Title"]).unwrap_err();

    assert!(
        error.to_string().contains("unexpected argument 'Title'"),
        "{error}"
    );
    assert!(
        error.use_stderr(),
        "a stray argument is a failure, not help output"
    );
}

#[test]
fn comment_takes_an_author_and_a_reply_target() {
    let args = parse(&["comment", "T-02wt0kx", "--author", "jp", "--re", "1"]).unwrap();

    match args.command {
        Command::Comment {
            id,
            author,
            re,
            body,
        } => {
            assert_eq!(id, Some("T-02wt0kx".parse().unwrap()));
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
    for args in [&["02wt0kx"][..], &["T02wt0kx"], &["T-02wt0kx"], &[
        "t-02WT0KX",
    ]] {
        match parse(args).unwrap().command {
            Command::Show { id } => {
                assert_eq!(id, Some("T-02wt0kx".parse().unwrap()), "{args:?}");
            }
            other => panic!("expected show for {args:?}, got {other:?}"),
        }
    }
}

/// An exact subcommand always wins over id parsing.
///
/// `comment` and `promote` are seven characters that fold onto the id alphabet,
/// so they genuinely parse as ids; only the subcommand check keeps the alias
/// from swallowing them.
#[test]
fn subcommands_are_not_mistaken_for_ids() {
    assert!(
        "comment".parse::<TicketId>().is_ok(),
        "this test guards nothing if `comment` stops parsing as an id"
    );

    for name in [
        "add", "comment", "close", "show", "promote", "import", "list", "status", "rename",
        "board", "rank", "init", "tools",
    ] {
        assert_eq!(
            with_show_alias(&[name.to_owned()]),
            vec![name.to_owned()],
            "{name} was read as an id"
        );
    }

    match parse(&["comment", "T-02wt0kx", "--body", "Hi."])
        .unwrap()
        .command
    {
        Command::Comment { .. } => {}
        other => panic!("expected comment, got {other:?}"),
    }
}

#[test]
fn the_author_falls_back_through_jp_then_git_then_the_environment() {
    let jp = serde_json::json!({
      "user": {
        "name": "John Doe"
      }
    });

    // An explicit author wins outright and never reaches git, so this is the
    // one case with a fully fixed answer.
    assert_eq!(
        resolve_author(Some("  Someone Else  ".to_owned()), &jp),
        Ok("Someone Else".to_owned())
    );

    // A blank `--author` is no answer at all, so the configured name applies
    // just as it would with no flag at all.
    assert_eq!(
        resolve_author(Some("   ".to_owned()), &jp),
        resolve_author(None, &jp)
    );

    // The configured name picks up git's email, so a ticket's author line reads
    // like an RFD's. The address is whatever the machine has configured and may
    // be missing entirely, so only the shape is pinned: the name, and nothing
    // after it but a bracketed address.
    let resolved = resolve_author(None, &jp).unwrap();
    assert!(
        resolved == "John Doe" || (resolved.starts_with("John Doe <") && resolved.ends_with('>')),
        "{resolved}"
    );

    // The git and OS-username fallbacks are deliberately not exercised. Whether
    // they yield a name or an error is a property of the machine, not of this
    // function: CI has neither a git identity nor a username in the
    // environment, and a developer's machine has both. Asserting `is_ok()` here
    // passes for whichever branch happens to fire, which is how this test came
    // to fail on Windows. Covering them means having `resolve_author` take
    // those two values as arguments instead of reading them itself.
}

/// A `--body` given on the command line stays the description.
///
/// It used to seed the whole-ticket buffer, where a single line reads back as a
/// title: the description became the summary and the ticket was filed with
/// none.
/// Only the title is asked for now, so the text keeps the field it was given
/// for.
#[test]
fn an_explicit_body_is_not_read_back_as_the_title() {
    let dir = Utf8TempDir::new().unwrap();

    let messages = exchange_with(
        &init(dir.path(), &["add", "--body", "Steps to reproduce"]),
        &[composed("Header misaligned")],
    );

    // One prompt, and it asks for the title rather than the whole ticket.
    let asked: Vec<&jp_plugin::message::ComposeRequest> = messages
        .iter()
        .filter_map(|message| match message {
            PluginToHost::Compose(request) => Some(request),
            _ => None,
        })
        .collect();

    assert_eq!(asked.len(), 1, "asked more than once: {messages:?}");
    assert_eq!(asked[0].message, "Title");
    assert!(
        matches!(asked[0].mode, ComposeMode::Line { .. }),
        "asked for a whole ticket, not a title: {:?}",
        asked[0].mode
    );

    // Read back through the store rather than by filename: the id is allocated,
    // so the path is not something the test gets to predict.
    let entries = ticket::store::list(&dir.path().join(ticket::store::DEFAULT_DIR)).unwrap();
    let [entry] = entries.as_slice() else {
        panic!("expected exactly one ticket, got {entries:?}");
    };
    let filed = entry.ticket.as_ref().expect("a well-formed ticket");

    assert_eq!(filed.title, "Header misaligned");
    assert_eq!(
        filed.description.trim(),
        "Steps to reproduce",
        "the description should be the text `--body` supplied"
    );
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
        Command::List {
            status,
            labels,
            query,
        } => {
            assert_eq!(query, None);
            assert_eq!(status, Some(Status::InProgress));
            assert!(labels.is_empty());
        }
        other => panic!("expected list, got {other:?}"),
    }
}

/// Repeating `--label` narrows the listing rather than replacing the previous
/// value, so several can be required at once.
#[test]
fn list_takes_repeated_label_filters() {
    let args = parse(&[
        "list",
        "--label",
        "package=jp_cli",
        "--label",
        "client=macos",
    ])
    .unwrap();

    match args.command {
        Command::List { labels, .. } => assert_eq!(labels, ["package=jp_cli", "client=macos"]),
        other => panic!("expected list, got {other:?}"),
    }
}

/// An omitted id reaches the picker rather than the "no terminal" refusal,
/// which is what the subcommand's `Omit it to choose` promises.
///
/// Asserted through the picker's own empty-board error: reaching it at all is
/// the thing under test, and it fires before any protocol exchange.
#[test]
fn labelling_without_an_id_offers_the_picker() {
    let dir = Utf8TempDir::new().unwrap();

    let error = compose_missing(
        dir.path(),
        &labelled(),
        Command::Label {
            id: None,
            labels: vec!["client=cli".to_owned()],
        },
        &mut &b""[..],
        &mut Vec::new(),
    )
    .unwrap_err();

    assert_eq!(error, "No tickets to choose from.");
}

/// `jp vite serve` reads the revision of the site files this plugin was built
/// alongside out of the parentheses.
#[test]
fn the_version_names_the_site_revision() {
    let error = parse(&["--version"]).unwrap_err();

    assert!(!error.use_stderr());
    assert_eq!(error.to_string(), format!("jp ticket {VERSION}\n"));

    let revision = VERSION
        .split_once("(site ")
        .and_then(|(_, rest)| rest.strip_suffix(')'))
        .unwrap();
    assert_eq!(revision.len(), 16, "{VERSION}");
    assert!(revision.chars().all(|c| c.is_ascii_hexdigit()), "{VERSION}");
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

    assert_eq!(
        resolve_dir(root, Utf8Path::new("docs/ticket")),
        "/repo/docs/ticket"
    );
    assert_eq!(resolve_dir(root, Utf8Path::new("/elsewhere")), "/elsewhere");
}

/// `--dir` names the directory for one run; without it, the configured one
/// applies.
#[test]
fn the_ticket_directory_comes_from_the_options_unless_dir_names_one() {
    let dir = Utf8TempDir::new().unwrap();

    exchange(&init_with_options(
        dir.path(),
        &["add", "Filed in the configured place"],
        serde_json::json!({ "dir": "work/items" }),
    ));
    exchange(&init_with_options(
        dir.path(),
        &["--dir", "elsewhere", "add", "Filed where --dir says"],
        serde_json::json!({ "dir": "work/items" }),
    ));

    assert_eq!(
        store::list(&dir.path().join("work/items")).unwrap().len(),
        1
    );
    assert_eq!(store::list(&dir.path().join("elsewhere")).unwrap().len(), 1);
    assert!(!dir.path().join("docs/ticket").exists());
}

/// Options the plugin can't read end the run before anything is written, with a
/// reason naming the setting.
#[test]
fn unreadable_options_end_the_run() {
    let dir = Utf8TempDir::new().unwrap();

    let messages = exchange(&init_with_options(
        dir.path(),
        &["board"],
        serde_json::json!({ "board": { "done_limit": "eight" } }),
    ));

    match messages.as_slice() {
        [PluginToHost::Ready(_), PluginToHost::Exit(exit)] => {
            assert_eq!(exit.code, 1);
            assert!(
                exit.reason
                    .as_deref()
                    .is_some_and(|reason| reason.starts_with("`plugins.command.ticket.options`")),
                "{:?}",
                exit.reason
            );
        }
        other => panic!("unexpected exchange: {other:?}"),
    }
}

/// A retitle moves the file with it, so a filename never disagrees with the
/// heading it carries.
/// Both paths are reported: the old one is what the caller was holding.
#[test]
fn rename_moves_the_file() {
    let dir = Utf8TempDir::new().unwrap();
    run_command(&dir, Command::Add {
        title: Some("Tool call header misaligned".to_owned()),
        author: Some("John Doe".to_owned()),
        body: None,
        implements: None,
        labels: vec![],
    })
    .unwrap();
    let id = store::list(dir.path()).unwrap()[0].id;
    let prefix = id.file_prefix();

    // Joined rather than spelled with `/`, because the reported path carries
    // native separators and reads `\` on Windows.
    let before = dir
        .path()
        .join(format!("{prefix}tool-call-header-misaligned.md"));
    let after = dir
        .path()
        .join(format!("{prefix}the-wrap-calculation-is-off.md"));

    let renamed = run_command(&dir, Command::Rename {
        id,
        title: "The wrap calculation is off".to_owned(),
    })
    .unwrap();

    assert_eq!(renamed.text, format!("Renamed {before} -> {after}\n"));
    assert!(!before.exists());

    // A title that slugs to the name the file has leaves it where it is.
    let retitled = run_command(&dir, Command::Rename {
        id,
        title: "The wrap calculation is off!".to_owned(),
    })
    .unwrap();
    assert_eq!(retitled.text, format!("Retitled {after}\n"));
}

#[test]
fn edit_takes_every_part_of_a_ticket() {
    let args = parse(&[
        "edit",
        "T-02wt0kx",
        "--title",
        "New",
        "--label",
        "client=cli",
        "--label",
        "package=jp_cli",
    ])
    .unwrap();

    match args.command {
        Command::Edit { title, labels, .. } => {
            assert_eq!(title.as_deref(), Some("New"));
            assert_eq!(labels, ["client=cli", "package=jp_cli"]);
        }
        other => panic!("expected edit, got {other:?}"),
    }
}

#[test]
fn edit_rewrites_the_description_and_keeps_the_title() {
    let dir = Utf8TempDir::new().unwrap();
    let id = write_ticket(dir.path(), "02wt0kx", "Kept title", Status::Todo);

    let edited = run_command(&dir, Command::Edit {
        id: Some(id),
        title: None,
        body: Some("A new description.".to_owned()),
        status: None,
        labels: vec![],
    })
    .unwrap();

    let path = dir.path().join("02wt0kx-slug.md");
    assert_eq!(edited.text, format!("Edited {path}\n"));
    let ticket = ticket::parse::document(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(ticket.title, "Kept title");
    assert_eq!(ticket.description, "A new description.");
}

/// A new title moves the file, as `rename` does, and the labels named are the
/// ones the ticket ends up with.
#[test]
fn edit_changes_the_title_and_the_labels_at_once() {
    let dir = Utf8TempDir::new().unwrap();
    let id = write_ticket(dir.path(), "02wt0kx", "Wrapping is wrong", Status::Todo);

    let edited = run_command(&dir, Command::Edit {
        id: Some(id),
        title: Some("The wrap calculation is off".to_owned()),
        body: None,
        status: None,
        labels: vec!["client=cli".to_owned()],
    })
    .unwrap();

    let path = dir.path().join("02wt0kx-the-wrap-calculation-is-off.md");
    assert_eq!(edited.text, format!("Edited {path}\n"));
    let ticket = ticket::parse::document(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(ticket.title, "The wrap calculation is off");
    assert_eq!(ticket.metadata.labels.to_tokens(), ["client=cli"]);
}

/// Labels are checked before anything is written, so a refused one leaves the
/// title, and the file's name, as they were.
#[test]
fn edit_refuses_a_label_before_changing_anything() {
    let dir = Utf8TempDir::new().unwrap();
    let id = write_ticket(dir.path(), "02wt0kx", "Wrapping is wrong", Status::Todo);
    let path = dir.path().join("02wt0kx-slug.md");
    let before = std::fs::read_to_string(&path).unwrap();

    let error = run_command(&dir, Command::Edit {
        id: Some(id),
        title: Some("The wrap calculation is off".to_owned()),
        body: None,
        status: None,
        labels: vec!["client=windows".to_owned()],
    })
    .unwrap_err();

    assert!(error.contains("client=windows"), "{error}");
    assert_eq!(std::fs::read_to_string(&path).unwrap(), before);
}

/// With nothing to change, `edit` asks the host to open the ticket's file, and
/// reads it back once the editor closes.
#[test]
fn edit_with_nothing_to_change_opens_the_editor_through_the_host() {
    let dir = Utf8TempDir::new().unwrap();
    let tickets = dir.path().join("docs/ticket");
    write_ticket(&tickets, "02wt0kx", "Wrapping is wrong", Status::Todo);
    let path = tickets.join("02wt0kx-slug.md");

    let messages = exchange_with(&init(dir.path(), &["edit", "T-02wt0kx"]), &[
        HostToPlugin::Edited(EditedResponse {
            id: None,
            saved: true,
        }),
    ]);

    assert!(
        messages.contains(&PluginToHost::EditFiles(EditFilesRequest {
            id: None,
            paths: vec![path.clone()],
        })),
        "{messages:?}"
    );
    assert!(
        messages.iter().any(|message| matches!(
            message,
            PluginToHost::Print(print) if print.text == format!("{path}: unchanged\n")
        )),
        "{messages:?}"
    );
    assert!(
        matches!(messages.last(), Some(PluginToHost::Exit(exit)) if exit.code == 0),
        "{messages:?}"
    );
}

/// A host that can't open an editor says why, and the flags are named as the
/// way to make the same change without one.
#[test]
fn edit_without_an_editor_names_the_flags() {
    let dir = Utf8TempDir::new().unwrap();
    write_ticket(
        &dir.path().join("docs/ticket"),
        "02wt0kx",
        "Wrapping is wrong",
        Status::Todo,
    );

    let messages = exchange_with(&init(dir.path(), &["edit", "T-02wt0kx"]), &[
        HostToPlugin::Error(ErrorResponse {
            id: None,
            request: Some("edit_files".to_owned()),
            message: "there is no terminal to open an editor on".to_owned(),
        }),
    ]);

    assert_eq!(
        messages.last(),
        Some(&PluginToHost::Exit(ExitMessage {
            code: 1,
            reason: Some(
                "Couldn't open an editor: there is no terminal to open an editor on. Pass what to \
                 change instead: --title, --body, --status, or --label."
                    .to_owned()
            ),
        }))
    );
}

#[test]
fn status_moves_a_ticket_to_any_column() {
    let dir = Utf8TempDir::new().unwrap();
    let id = write_ticket(dir.path(), "02wt0kx", "Moving", Status::Todo);
    let path = dir.path().join("02wt0kx-slug.md");

    let moved = run_command(&dir, Command::Status {
        id,
        status: Status::InProgress,
        before: None,
        after: None,
    })
    .unwrap();
    assert_eq!(moved.text, format!("{path}: Todo -> In Progress\n"));

    let again = run_command(&dir, Command::Status {
        id,
        status: Status::InProgress,
        before: None,
        after: None,
    })
    .unwrap();
    assert_eq!(again.text, format!("{path}: already In Progress\n"));
}

/// A drag on the site's board is this one command: the column and the place in
/// it change together.
#[test]
fn status_places_the_ticket_next_to_its_neighbour() {
    let dir = Utf8TempDir::new().unwrap();
    let root = dir.path();
    let moving = write_ticket(root, "02wt0k1", "Moving", Status::Todo);
    let started = write_ticket(root, "02wt0k2", "Started", Status::InProgress);
    write_ticket(root, "02wt0k3", "Also started", Status::InProgress);

    let moved = run_in(root, Command::Status {
        id: moving,
        status: Status::InProgress,
        before: None,
        after: Some(started),
    })
    .unwrap();
    assert_eq!(
        moved.text,
        format!(
            "{}: Todo -> In Progress, below T-02wt0k2\n",
            root.join("02wt0k1-slug.md")
        )
    );

    let board = run_in(root, Command::Board).unwrap();
    assert!(
        board.text.contains(
            "In Progress (3)\nT-02wt0k2 Started\nT-02wt0k1 Moving\nT-02wt0k3 Also started\n"
        ),
        "{}",
        board.text
    );
}

/// The neighbour is checked before the ticket moves, so a drop next to a card
/// that has since left the column changes nothing.
#[test]
fn status_refuses_a_neighbour_outside_the_column() {
    let dir = Utf8TempDir::new().unwrap();
    let root = dir.path();
    let moving = write_ticket(root, "02wt0k1", "Moving", Status::Todo);
    let done = write_ticket(root, "02wt0k2", "Finished", Status::Done);

    let error = run_in(root, Command::Status {
        id: moving,
        status: Status::InProgress,
        before: Some(done),
        after: None,
    })
    .unwrap_err();

    assert_eq!(
        error,
        "T-02wt0k2 is Done, not In Progress; a ticket is placed next to one in the column it \
         moves to."
    );
    let source = std::fs::read_to_string(root.join("02wt0k1-slug.md")).unwrap();
    assert!(source.contains("- **Status**: Todo"), "{source}");
}

#[test]
fn status_takes_the_id_and_the_column() {
    match parse(&["status", "T-02wt0kx", "in progress"])
        .unwrap()
        .command
    {
        Command::Status {
            id,
            status,
            before,
            after,
        } => {
            assert_eq!(id, "T-02wt0kx".parse().unwrap());
            assert_eq!(status, Status::InProgress);
            assert_eq!((before, after), (None, None));
        }
        other => panic!("expected status, got {other:?}"),
    }

    assert!(parse(&["status", "T-02wt0kx"]).is_err());
    assert!(parse(&["status", "T-02wt0kx", "done", "--after", "T-02wt0k2"]).is_ok());
    assert!(
        parse(&[
            "status",
            "T-02wt0kx",
            "done",
            "--before",
            "T-02wt0k1",
            "--after",
            "T-02wt0k2"
        ])
        .is_err()
    );
}

/// The board reads each column in board order, then the tickets nobody ranked;
/// Done runs newest first and stops at the configured limit.
#[test]
fn board_prints_each_column_in_board_order() {
    let dir = Utf8TempDir::new().unwrap();
    let root = dir.path();
    write_ticket(root, "02wt0k1", "First filed", Status::Todo);
    write_ticket(root, "02wt0k2", "Second filed", Status::Todo);
    write_ticket(root, "02wt0k3", "Third filed", Status::Todo);
    write_ticket(root, "02wt0k4", "Older done", Status::Done);
    write_ticket(root, "02wt0k5", "Newer done", Status::Done);
    std::fs::write(
        root.join(board::BOARD_FILE),
        r#"{"todo": ["T-02wt0k3"], "in_progress": [], "done": []}"#,
    )
    .unwrap();
    let options = Options {
        board: options::BoardOptions { done_limit: 1 },
        ..Options::default()
    };

    let output = run_with_options(root, Command::Board, &options).unwrap();

    assert_eq!(
        output.text,
        "Todo (3)\nT-02wt0k3 Third filed\nT-02wt0k1 First filed\nT-02wt0k2 Second filed\n\nIn \
         Progress (0)\n\nDone (2, showing 1)\nT-02wt0k5 Newer done\n"
    );
}

#[test]
fn rank_places_a_ticket_and_the_board_follows() {
    let dir = Utf8TempDir::new().unwrap();
    let root = dir.path();
    let first = write_ticket(root, "02wt0k1", "First filed", Status::Todo);
    let second = write_ticket(root, "02wt0k2", "Second filed", Status::Todo);

    let ranked = run_in(root, Command::Rank {
        id: second,
        before: Some(first),
        after: None,
    })
    .unwrap();
    assert_eq!(ranked.text, "T-02wt0k2 now sits above T-02wt0k1 in Todo\n");

    let board = run_in(root, Command::Board).unwrap();
    assert!(
        board
            .text
            .starts_with("Todo (2)\nT-02wt0k2 Second filed\nT-02wt0k1 First"),
        "{}",
        board.text
    );
}

#[test]
fn rank_takes_exactly_one_place() {
    assert!(parse(&["rank", "T-02wt0k1"]).is_err());
    assert!(
        parse(&[
            "rank",
            "T-02wt0k1",
            "--before",
            "T-02wt0k2",
            "--after",
            "T-02wt0k3"
        ])
        .is_err()
    );
    assert!(parse(&["rank", "T-02wt0k1", "--after", "T-02wt0k2"]).is_ok());
}

/// The order within one column says nothing about another.
#[test]
fn rank_refuses_tickets_in_different_columns() {
    let dir = Utf8TempDir::new().unwrap();
    let root = dir.path();
    let open = write_ticket(root, "02wt0k1", "Open", Status::Todo);
    let done = write_ticket(root, "02wt0k2", "Done", Status::Done);

    let error = run_in(root, Command::Rank {
        id: open,
        before: None,
        after: Some(done),
    })
    .unwrap_err();

    assert_eq!(
        error,
        "T-02wt0k1 is Todo and T-02wt0k2 is Done; a ticket is ranked within its own column."
    );
}

/// `init` creates the directory and nothing in it: the board needs no order
/// file until something is ranked.
#[test]
fn init_creates_the_ticket_directory() {
    let dir = Utf8TempDir::new().unwrap();
    let tickets = dir.path().join("docs/ticket");

    let created = run_in(&tickets, Command::Init).unwrap();
    assert_eq!(created.text, format!("Created {tickets}\n"));
    assert!(tickets.is_dir());
    assert_eq!(std::fs::read_dir(&tickets).unwrap().count(), 0);

    let again = run_in(&tickets, Command::Init).unwrap();
    assert_eq!(again.text, format!("{tickets} already exists.\n"));
}

/// The repository is named on every call; there is no default to land issues
/// from the wrong place.
#[test]
fn import_requires_a_repository() {
    let error = parse(&["import", "123"]).unwrap_err();
    assert!(error.to_string().contains("--repo"), "{error}");

    match parse(&["import", "123", "--repo", "dcdpr/jp"])
        .unwrap()
        .command
    {
        Command::Import { numbers, repo, .. } => {
            assert_eq!(numbers, [123]);
            assert_eq!(repo, "dcdpr/jp");
        }
        other => panic!("expected import, got {other:?}"),
    }
}

/// The definitions directory is taken against the workspace root, like `--dir`.
#[test]
fn tools_init_writes_into_a_directory_under_the_root() {
    let dir = Utf8TempDir::new().unwrap();

    let output = run_in(dir.path(), Command::Tools {
        command: ToolsCommand::Init {
            path: Utf8PathBuf::from("config/tools"),
        },
    })
    .unwrap();

    let written = dir.path().join("config/tools/create.toml");
    assert!(written.exists());
    assert!(
        output.text.contains(&format!("Wrote {written}\n")),
        "{}",
        output.text
    );
}

#[test]
fn commands_run_against_the_resolved_directory() {
    let dir = Utf8TempDir::new().unwrap();

    let created = run_command(&dir, Command::Add {
        title: Some("Tool call header misaligned".to_owned()),
        author: Some("John Doe".to_owned()),
        body: Some("The header renders one column left of the body.".to_owned()),
        implements: None,
        labels: vec![],
    })
    .unwrap();
    assert!(created.warnings.is_empty());

    // Ids are generated, so the rest of the lifecycle follows the one that was
    // just handed out rather than a fixed number.
    let id = store::list(dir.path()).unwrap()[0].id;
    assert!(
        created.text.contains(&format!("({id})")),
        "{}",
        created.text
    );

    let commented = run_command(&dir, Command::Comment {
        id: Some(id),
        author: Some("john".to_owned()),
        re: None,
        body: Some("Reproduced at 72 columns.".to_owned()),
    })
    .unwrap();
    assert_eq!(commented.text, format!("Added {id}#1 by john\n"));

    let listed = run_command(&dir, Command::List {
        status: None,
        labels: vec![],
        query: None,
    })
    .unwrap();
    assert_eq!(
        listed.text,
        format!("{id} Todo         Tool call header misaligned\n")
    );

    let closed = run_command(&dir, Command::Close { id: Some(id) }).unwrap();
    assert!(closed.text.contains("Todo -> Done"), "{}", closed.text);

    let json = run_json(&dir, Command::List {
        status: Some(Status::Done),
        labels: vec![],
        query: None,
    })
    .unwrap();
    assert!(json.json);
    let rows: Vec<serde_json::Value> = serde_json::from_str(&json.text).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["id"], id.to_string());
    assert_eq!(rows[0]["status"], "Done");
    assert_eq!(rows[0]["comments"], 1);
}

/// The whole point of `refresh`: the branch's own references follow the new id,
/// and references that were already on `base` do not — they belong to the
/// ticket that kept it.
#[test]
fn refresh_rewrites_the_branch_and_leaves_the_base_alone() {
    let repo = git_repo();
    let root = repo.path();
    let dir = root.join("docs/ticket");

    // On `main`: the ticket that wins the id, and a file naming it.
    std::fs::write(
        dir.join("02wt0kx-winner.md"),
        "# Winner\n\n- **Status**: Todo\n- **Authors**: john\n- **Date**: 2026-08-14\n\nBody.\n",
    )
    .unwrap();
    std::fs::write(root.join("NOTES.md"), "See T-02wt0kx for the winner.\n").unwrap();
    commit(root, "Add the winning ticket", None);

    // On the branch: a second ticket that drew the same id, and a file of its
    // own naming it.
    git(root, &["checkout", "-b", "loser"]);
    let losing = dir.join("02wt0kx-loser.md");
    std::fs::write(
        &losing,
        "# Loser\n\n- **Status**: Todo\n- **Authors**: jane\n- **Date**: 2026-08-14\n\nOther \
         body.\n",
    )
    .unwrap();
    std::fs::write(root.join("BRANCH.md"), "Fixes T-02wt0kx on this branch.\n").unwrap();
    commit(root, "Add the losing ticket", Some("2026-08-14T12:00:00Z"));

    let output = run_in(&dir, Command::Refresh {
        path: losing.clone(),
        base: "main".to_owned(),
    })
    .unwrap();

    // The loser moved.
    assert!(!losing.exists());
    let fresh = store::list(&dir)
        .unwrap()
        .into_iter()
        .find(|entry| entry.ticket.as_ref().is_ok_and(|t| t.title == "Loser"))
        .expect("the losing ticket survived under a new id");
    assert_ne!(fresh.id.to_string(), "T-02wt0kx");

    // The branch's own reference followed it.
    assert_eq!(
        std::fs::read_to_string(root.join("BRANCH.md")).unwrap(),
        format!("Fixes {} on this branch.\n", fresh.id)
    );

    // The reference that was already on `main` did not.
    assert_eq!(
        std::fs::read_to_string(root.join("NOTES.md")).unwrap(),
        "See T-02wt0kx for the winner.\n"
    );
    assert_eq!(
        std::fs::read_to_string(dir.join("02wt0kx-winner.md"))
            .unwrap()
            .lines()
            .next(),
        Some("# Winner")
    );

    assert!(
        output
            .text
            .starts_with(&format!("T-02wt0kx -> {} at ", fresh.id)),
        "{}",
        output.text
    );

    // `NOTES.md` names the winner but the branch never touched it, so it is not
    // a candidate at all and there is nothing to report.
    assert!(output.warnings.is_empty(), "{:?}", output.warnings);
}

/// A reply names a position, not a ticket, so a rename has nothing to fix
/// inside the file.
/// That is what lets `reassign` be a plain rename.
#[test]
fn refresh_leaves_a_reply_alone() {
    let repo = git_repo();
    let root = repo.path();
    let dir = root.join("docs/ticket");

    let losing = dir.join("02wt0kx-loser.md");
    std::fs::write(
        &losing,
        "# Loser\n\n- **Status**: Todo\n- **Authors**: jane\n- **Date**: \
         2026-08-14\n\nBody.\n\n## Comments\n\n-----\n\n- **From**: john\n- **Date**: \
         2026-08-14T10:00:00Z\n\nFirst.\n\n-----\n\n- **From**: jp\n- **Date**: \
         2026-08-14T11:00:00Z\n- **Re**: #1\n\nSecond.\n",
    )
    .unwrap();
    let before = std::fs::read_to_string(&losing).unwrap();
    commit(root, "Add the losing ticket", None);

    let output = run_in(&dir, Command::Refresh {
        path: losing,
        base: "main".to_owned(),
    })
    .unwrap();

    let entry = store::list(&dir).unwrap().pop().unwrap();
    assert_eq!(std::fs::read_to_string(&entry.path).unwrap(), before);
    assert_eq!(entry.ticket.unwrap().comments[1].re.as_deref(), Some("#1"));
    assert!(output.warnings.is_empty(), "{:?}", output.warnings);
}

/// Prose inside the renamed ticket naming its old id is ambiguous the same way
/// any other file's is — it may mean the ticket that kept the id — so it is
/// reported rather than redirected.
#[test]
fn refresh_reports_prose_naming_the_old_id_in_the_renamed_ticket() {
    let repo = git_repo();
    let root = repo.path();
    let dir = root.join("docs/ticket");

    let losing = dir.join("02wt0kx-loser.md");
    std::fs::write(
        &losing,
        "# Loser\n\n- **Status**: Todo\n- **Authors**: jane\n- **Date**: 2026-08-14\n\nProbably a \
         duplicate of T-02wt0kx.\n",
    )
    .unwrap();
    commit(root, "Add the losing ticket", None);

    let output = run_in(&dir, Command::Refresh {
        path: losing,
        base: "main".to_owned(),
    })
    .unwrap();

    let entry = store::list(&dir).unwrap().pop().unwrap();
    let source = std::fs::read_to_string(&entry.path).unwrap();
    assert!(source.contains("duplicate of T-02wt0kx."), "{source}");

    assert_eq!(output.warnings.len(), 1, "{:?}", output.warnings);
    assert!(
        output.warnings[0].contains(entry.path.as_str()),
        "{}",
        output.warnings[0]
    );
}

/// `just ticket-refresh` documents a workspace-relative path, and git runs in
/// the ticket directory, so the path has to be resolved before either sees it.
#[test]
fn refresh_accepts_a_repository_relative_path() {
    let repo = git_repo();
    let root = repo.path();
    let dir = root.join("docs/ticket");

    std::fs::write(
        dir.join("02wt0kx-filed-earlier.md"),
        "# Filed earlier\n\n- **Status**: Todo\n- **Authors**: john\n- **Date**: \
         2026-08-14\n\nBody.\n",
    )
    .unwrap();
    // 2026-08-14T00:00:00Z is 345,600 seconds past the epoch: bucket 69,120.
    commit(root, "File it", Some("2026-08-14T00:00:00Z"));

    run_in(&dir, Command::Refresh {
        path: Utf8PathBuf::from("docs/ticket/02wt0kx-filed-earlier.md"),
        base: "main".to_owned(),
    })
    .unwrap();

    // The add-date drove the bucket, which only happens when git resolved the
    // path. A silent fallback would put it at the current bucket instead.
    let fresh = store::list(&dir).unwrap()[0].id;
    assert_eq!(fresh.bucket(), 69_120);
}

/// A file the branch edited that already named the id on `base` is ambiguous:
/// the occurrence may be the winning ticket's.
/// Report it, don't redirect it.
#[test]
fn refresh_reports_a_file_that_named_the_id_on_base() {
    let repo = git_repo();
    let root = repo.path();
    let dir = root.join("docs/ticket");

    std::fs::write(root.join("SHARED.md"), "Tracked in T-02wt0kx.\n").unwrap();
    commit(root, "Reference the winner", None);

    git(root, &["checkout", "-b", "loser"]);
    let losing = dir.join("02wt0kx-loser.md");
    std::fs::write(
        &losing,
        "# Loser\n\n- **Status**: Todo\n- **Authors**: jane\n- **Date**: 2026-08-14\n\nBody.\n",
    )
    .unwrap();
    // The branch touches the file for an unrelated reason.
    std::fs::write(
        root.join("SHARED.md"),
        "Tracked in T-02wt0kx.\nAlso worth reading.\n",
    )
    .unwrap();
    commit(root, "Add the losing ticket", None);

    let output = run_in(&dir, Command::Refresh {
        path: losing,
        base: "main".to_owned(),
    })
    .unwrap();

    assert_eq!(
        std::fs::read_to_string(root.join("SHARED.md")).unwrap(),
        "Tracked in T-02wt0kx.\nAlso worth reading.\n"
    );
    assert_eq!(output.warnings.len(), 1, "{:?}", output.warnings);
    assert!(
        output.warnings[0].contains("SHARED.md"),
        "{}",
        output.warnings[0]
    );
}

/// The new id must sit where the ticket was filed, not where the clock is now,
/// or a refreshed ticket jumps ahead of everything it was created alongside.
#[test]
fn refresh_keeps_the_ticket_in_the_bucket_it_was_created_in() {
    let repo = git_repo();
    let root = repo.path();
    let dir = root.join("docs/ticket");

    let path = dir.join("02wt0kx-filed-earlier.md");
    std::fs::write(
        &path,
        "# Filed earlier\n\n- **Status**: Todo\n- **Authors**: john\n- **Date**: \
         2026-08-14\n\nBody.\n",
    )
    .unwrap();
    // 2026-08-14T00:00:00Z is 345,600 seconds past the epoch: bucket 69,120.
    commit(root, "File it", Some("2026-08-14T00:00:00Z"));

    run_in(&dir, Command::Refresh {
        path,
        base: "main".to_owned(),
    })
    .unwrap();

    let fresh = store::list(&dir).unwrap()[0].id;
    assert_eq!(fresh.bucket(), 69_120);
}

/// An unreadable candidate must not be mistaken for one that holds no
/// reference, or `refresh` reports success on an incomplete repair.
#[test]
fn rewriting_an_unreadable_file_is_an_error() {
    let dir = Utf8TempDir::new().unwrap();
    let directory = dir.path().join("a-directory");
    std::fs::create_dir(&directory).unwrap();

    // Reading a directory fails with neither `NotFound` nor `InvalidData`.
    let error = rewrite(&directory, "T-02wt0kx", "T-03abcde").unwrap_err();

    assert!(error.starts_with(directory.as_str()), "{error}");
}

/// A file the branch deleted is expected to be gone, and a binary one is
/// expected not to be text.
/// Neither is a failure.
#[test]
fn rewriting_a_missing_or_binary_file_is_skipped() {
    let dir = Utf8TempDir::new().unwrap();

    assert!(!rewrite(&dir.path().join("gone.md"), "a", "b").unwrap());

    let binary = dir.path().join("binary.bin");
    std::fs::write(&binary, [0xff, 0xfe, 0x00]).unwrap();
    assert!(!rewrite(&binary, "a", "b").unwrap());
}

/// The ticket document has no id of its own, so the output has to carry it.
#[test]
fn show_json_carries_the_id() {
    let dir = Utf8TempDir::new().unwrap();
    run_command(&dir, Command::Add {
        title: Some("Tool call header misaligned".to_owned()),
        author: Some("john".to_owned()),
        body: Some("The header renders one column left.".to_owned()),
        implements: None,
        labels: vec![],
    })
    .unwrap();
    let id = store::list(dir.path()).unwrap()[0].id;

    let out = run_json(&dir, Command::Show { id: Some(id) }).unwrap();

    assert!(out.json);
    let detail: serde_json::Value = serde_json::from_str(&out.text).unwrap();
    assert_eq!(detail["id"], id.to_string());
    assert_eq!(detail["title"], "Tool call header misaligned");
    assert_eq!(detail["description"], "The header renders one column left.");
}

#[test]
fn an_empty_comment_is_refused() {
    let dir = Utf8TempDir::new().unwrap();

    let error = run_command(&dir, Command::Comment {
        id: Some("T-02wt0kx".parse().unwrap()),
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
        id: Some("T-zzzzzzz".parse().unwrap()),
    })
    .unwrap_err();

    assert_eq!(error, "No ticket T-zzzzzzz.");
}

#[test]
fn describe_reports_the_subcommand_it_serves() {
    let messages = exchange(&HostToPlugin::Describe);

    match messages.as_slice() {
        [PluginToHost::Describe(describe)] => {
            assert_eq!(describe.name, "ticket");
            assert_eq!(describe.manifest.command, ["ticket"]);
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
fn a_run_reports_ready_then_output_then_exit() {
    let dir = Utf8TempDir::new().unwrap();
    let messages = exchange(&init(dir.path(), &["add", "Tool call header misaligned"]));

    match messages.as_slice() {
        [
            PluginToHost::Ready(_),
            PluginToHost::Print(print),
            PluginToHost::Exit(exit),
        ] => {
            assert_eq!(print.channel, "content");
            assert_eq!(exit.code, 0);

            // The id is generated, so the file it produced is what names it.
            let ticket_dir = dir.path().join("docs/ticket");
            let names: Vec<String> = std::fs::read_dir(&ticket_dir)
                .unwrap()
                .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
                .collect();
            assert_eq!(names.len(), 1, "{names:?}");

            let id = names[0]
                .strip_suffix("-tool-call-header-misaligned.md")
                .unwrap_or_else(|| panic!("unexpected filename {}", names[0]));

            // Joined rather than concatenated with `/`: the reported path comes
            // out of `Utf8Path::join`, which separates with `\` on Windows.
            let path = ticket_dir.join(&names[0]);
            assert_eq!(print.text, format!("Created {path} (T-{id})\n"));
        }
        other => panic!("unexpected exchange: {other:?}"),
    }
}

/// An `init` carrying a tool call that files a ticket.
fn tool_call_init(root: &Utf8Path, options: serde_json::Value) -> HostToPlugin {
    let mut message = init_message(jp_plugin::PROTOCOL_VERSION, root, &[]);
    let serde_json::Value::Object(options) = options else {
        panic!("options are an object");
    };
    message.options = options;
    message.tool = Some(ToolCall {
        action: ToolAction::Run,
        name: "create".to_owned(),
        arguments: Map::from_iter([("title".to_owned(), json!("Tool call header misaligned"))]),
        answers: Map::new(),
        options: Map::new(),
        access: None,
        conversation: "jp-c17000000000".to_owned(),
    });

    HostToPlugin::Init(Box::new(message))
}

/// A tool call is answered with one outcome and an exit, and nothing printed.
/// The host closed stdin after `init`, so the plugin must not wait for more.
#[test]
fn a_tool_call_is_answered_with_its_outcome_then_exit() {
    let dir = Utf8TempDir::new().unwrap();

    let messages = exchange(&tool_call_init(dir.path(), json!({ "assistant": "jp" })));

    match messages.as_slice() {
        [
            PluginToHost::Ready(ready),
            PluginToHost::ToolOutcome(answer),
            PluginToHost::Exit(exit),
        ] => {
            assert_eq!(ready.protocol, REQUIRED_PROTOCOL);
            assert_eq!(answer.outcome["type"], "success");
            let content = answer.outcome["content"].as_str().unwrap_or_default();
            assert!(content.starts_with("Created T-"), "{content}");
            assert_eq!(exit.code, 0);
        }
        other => panic!("unexpected exchange: {other:?}"),
    }

    // Filed through a tool, so attributed to the configured assistant rather
    // than the user `init.config` names.
    let entries = store::list(&dir.path().join("docs/ticket")).unwrap();
    let [entry] = entries.as_slice() else {
        panic!("expected one ticket, got {entries:?}");
    };
    assert_eq!(entry.ticket.as_ref().unwrap().metadata.authors, "jp");
}

/// The assistant can't fix the configuration, so options the plugin can't read
/// fail the plugin run rather than coming back as an outcome to retry.
#[test]
fn a_tool_call_with_unreadable_options_fails_the_run() {
    let dir = Utf8TempDir::new().unwrap();

    let messages = exchange(&tool_call_init(dir.path(), json!({ "assistant": "" })));

    match messages.as_slice() {
        [PluginToHost::Ready(_), PluginToHost::Exit(exit)] => {
            assert_eq!(exit.code, 1);
            assert_eq!(
                exit.reason.as_deref(),
                Some("`plugins.command.ticket.options.assistant` must be a name on one line.")
            );
        }
        other => panic!("unexpected exchange: {other:?}"),
    }
    assert!(!dir.path().join("docs/ticket").exists());
}

/// A plugin newer than its host cannot ask for anything interactive, so it says
/// so and stops, without claiming ready, rather than hanging on a reply the
/// host cannot produce.
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
                    .is_some_and(|reason| reason.contains("needs `jp` protocol 12")),
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

#[test]
fn labels_survive_the_round_trip_and_show_in_the_listing() {
    let dir = Utf8TempDir::new().unwrap();

    let id = add_labelled(&dir, "Window title truncates", &[
        "package=jp_config",
        "client=macos",
    ]);

    let listed = run_command(&dir, Command::List {
        status: None,
        labels: vec![],
        query: None,
    })
    .unwrap();

    assert_eq!(
        listed.text,
        format!("{id} Todo         Window title truncates [client=macos, package=jp_config]\n")
    );
}

#[test]
fn listing_narrows_to_tickets_carrying_every_label() {
    let dir = Utf8TempDir::new().unwrap();

    let both = add_labelled(&dir, "Both", &["client=macos", "package=jp_cli"]);
    add_labelled(&dir, "One", &["package=jp_cli"]);
    add_labelled(&dir, "None", &[]);

    let listed = run_command(&dir, Command::List {
        status: None,
        labels: vec!["package=jp_cli".to_owned(), "client=macos".to_owned()],
        query: None,
    })
    .unwrap();

    assert_eq!(
        listed.text,
        format!("{both} Todo         Both [client=macos, package=jp_cli]\n")
    );
}

/// A bare key filters on "carries this key at all", which is how you find
/// everything touching one facet.
#[test]
fn listing_narrows_on_a_bare_key() {
    let dir = Utf8TempDir::new().unwrap();

    add_labelled(&dir, "Client work", &["client=cli"]);
    add_labelled(&dir, "Crate work", &["package=jp_cli"]);

    let listed = run_command(&dir, Command::List {
        status: None,
        labels: vec!["client".to_owned()],
        query: None,
    })
    .unwrap();

    assert_eq!(listed.text.lines().count(), 1, "{}", listed.text);
    assert!(listed.text.contains("Client work"), "{}", listed.text);
}

/// A query matches the title or the description, whatever its case, and a blank
/// one keeps every ticket.
#[test]
fn listing_narrows_on_a_query() {
    let dir = Utf8TempDir::new().unwrap();
    let titled = add_labelled(&dir, "Wrap is off by one", &[]);
    let described = add_labelled(&dir, "Header misaligned", &[]);
    add_labelled(&dir, "Unrelated", &[]);
    run_command(&dir, Command::Edit {
        id: Some(described),
        title: None,
        body: Some("The WRAP width ignores the gutter.".to_owned()),
        status: None,
        labels: vec![],
    })
    .unwrap();
    let listing = |query: &str| {
        run_command(&dir, Command::List {
            status: None,
            labels: vec![],
            query: Some(query.to_owned()),
        })
        .unwrap()
        .text
    };

    assert_eq!(
        listing("wrap"),
        format!(
            "{titled} Todo         Wrap is off by one\n{described} Todo         Header \
             misaligned\n"
        )
    );
    assert_eq!(listing("  ").lines().count(), 3);
}

/// The whole set is written, so labelling is idempotent and a label left off
/// the call is a label removed.
#[test]
fn labelling_replaces_the_whole_set() {
    let dir = Utf8TempDir::new().unwrap();
    let id = add_labelled(&dir, "Replace me", &["client=macos", "package=jp_cli"]);

    let out = run_command(&dir, Command::Label {
        id: Some(id),
        labels: vec!["package=jp_cli".to_owned()],
    })
    .unwrap();
    assert!(out.text.ends_with(": package=jp_cli\n"), "{}", out.text);

    let cleared = run_command(&dir, Command::Label {
        id: Some(id),
        labels: vec![],
    })
    .unwrap();
    assert!(
        cleared.text.ends_with(": labels cleared\n"),
        "{}",
        cleared.text
    );

    let path = store::locate_ticket(dir.path(), id).unwrap();
    let source = std::fs::read_to_string(path).unwrap();
    assert!(!source.contains("- **Label**"), "{source}");
}

/// A typo names the vocabulary rather than landing a label nothing groups by.
#[test]
fn an_unknown_label_is_refused_with_the_known_set() {
    let dir = Utf8TempDir::new().unwrap();

    let error = run_command(&dir, Command::Add {
        title: Some("Typo".to_owned()),
        author: Some("john".to_owned()),
        body: None,
        implements: None,
        labels: vec!["client=mcaos".to_owned()],
    })
    .unwrap_err();

    assert_eq!(
        error,
        "`client=mcaos` is not a known label. Labels you can add: client=cli, client=macos, \
         package=jp_cli, package=jp_config."
    );
    assert!(
        store::list(dir.path()).unwrap().is_empty(),
        "a ticket was filed anyway"
    );
}

#[test]
fn the_vocabulary_listing_names_each_label_and_what_it_covers() {
    let dir = Utf8TempDir::new().unwrap();

    let out = run_command(&dir, Command::Labels).unwrap();

    assert_eq!(
        out.text,
        "client  The client.\n  client=cli\n  client=macos\n\npackage  The crate.\n  \
         package=jp_cli\n  package=jp_config\n  package=jp_legacy  (retired)\n\nA retired label \
         stays on a ticket that carries it, but can't be added.\n"
    );
}

/// The case the active/retired split exists for: an old ticket carries a label
/// the board has since retired, and adding a new one must not force the retired
/// one off first.
#[test]
fn a_retired_label_already_on_a_ticket_can_be_kept() {
    let dir = Utf8TempDir::new().unwrap();

    // Written by hand: `legacy-ui` can no longer be applied through the CLI,
    // which is exactly the situation an old ticket is in.
    let id = add_labelled(&dir, "Old ticket", &[]);
    let path = store::locate_ticket(dir.path(), id).unwrap();
    let source = std::fs::read_to_string(&path).unwrap();
    std::fs::write(
        &path,
        ticket::render::set_repeated_metadata(&source, "Label", &["package=jp_legacy".to_owned()])
            .unwrap(),
    )
    .unwrap();

    let out = run_command(&dir, Command::Label {
        id: Some(id),
        labels: vec!["package=jp_legacy".to_owned(), "client=cli".to_owned()],
    })
    .unwrap();

    assert!(
        out.text.ends_with(": client=cli, package=jp_legacy\n"),
        "{}",
        out.text
    );
}

#[test]
fn a_retired_label_cannot_be_added_fresh() {
    let dir = Utf8TempDir::new().unwrap();
    let id = add_labelled(&dir, "Fresh", &[]);

    let error = run_command(&dir, Command::Label {
        id: Some(id),
        labels: vec!["package=jp_legacy".to_owned()],
    })
    .unwrap_err();

    assert_eq!(
        error,
        "`package=jp_legacy` is retired and can only stay on an item that already carries it. \
         Labels you can add: client=cli, client=macos, package=jp_cli, package=jp_config."
    );
}

#[test]
fn a_board_without_a_vocabulary_says_where_to_define_one() {
    let dir = Utf8TempDir::new().unwrap();

    let out = run_in(dir.path(), Command::Labels).unwrap();

    assert_eq!(
        out.text,
        "This board defines no labels, so a ticket takes any `key` or `key=value` label. List \
         them in `plugins.command.ticket.options.labels` to hold tickets to a fixed set.\n"
    );
}

/// The file the vocabulary lived in before it moved into the options is no
/// longer read, so a board that kept it is told why its labels are gone.
#[test]
fn a_leftover_labels_file_is_pointed_out() {
    let dir = Utf8TempDir::new().unwrap();
    std::fs::write(dir.path().join(".labels.json"), "{}").unwrap();

    let out = run_in(dir.path(), Command::Labels).unwrap();

    assert_eq!(
        out.text,
        format!(
            "This board defines no labels, so a ticket takes any `key` or `key=value` label. List \
             them in `plugins.command.ticket.options.labels` to hold tickets to a fixed set.\n{} \
             is no longer read: move its keys there.\n",
            dir.path().join(".labels.json")
        )
    );
}

/// A vocabulary that doesn't parse fails the commands that label or file a
/// ticket, named by where it is configured, and no others.
/// Filing reads it even with no labels named, since a board can require some.
#[test]
fn a_broken_vocabulary_fails_what_labels_or_files() {
    let dir = Utf8TempDir::new().unwrap();
    let Value::Object(options) = json!({ "labels": { "client": "The client." } }) else {
        unreachable!("options are an object");
    };
    let options = Options::read(&options).unwrap();
    let broken = "`plugins.command.ticket.options.labels`: The label vocabulary is not a map";

    let error = run_with_options(dir.path(), Command::Labels, &options).unwrap_err();
    assert!(error.starts_with(broken), "{error}");

    let error = run_with_options(
        dir.path(),
        Command::Add {
            title: Some("Unlabelled".to_owned()),
            author: Some("john".to_owned()),
            body: None,
            implements: None,
            labels: vec![],
        },
        &options,
    )
    .unwrap_err();
    assert!(error.starts_with(broken), "{error}");

    run_with_options(
        dir.path(),
        Command::List {
            status: None,
            labels: vec![],
            query: None,
        },
        &options,
    )
    .unwrap();
}

/// A board where every ticket carries exactly one `type`.
fn typed() -> Options {
    let Value::Object(options) = json!({ "labels": {
        "type": { "description": "The type", "values": ["bug", "feature"], "min": 1, "max": 1 },
        "client": { "description": "The client.", "values": ["cli"] }
    } }) else {
        unreachable!("options are an object");
    };

    Options::read(&options).unwrap()
}

/// A ticket filed without a label the board requires is refused, naming the key
/// and the values to choose from, and nothing is written.
#[test]
fn adding_a_ticket_without_a_required_label_is_refused() {
    let dir = Utf8TempDir::new().unwrap();
    let add = |labels: &[&str]| {
        run_with_options(
            dir.path(),
            Command::Add {
                title: Some("Header misaligned".to_owned()),
                author: Some("john".to_owned()),
                body: None,
                implements: None,
                labels: labels.iter().map(|label| (*label).to_owned()).collect(),
            },
            &typed(),
        )
    };

    assert_eq!(
        add(&["client=cli"]).unwrap_err(),
        "`type` label missing: add one of type=bug, type=feature."
    );
    assert!(store::list(dir.path()).unwrap().is_empty());

    add(&["client=cli", "type=bug"]).unwrap();
    assert_eq!(store::list(dir.path()).unwrap().len(), 1);
}

/// Composing a ticket the board would refuse would throw away what the user
/// typed, so the labels are checked before the composer opens.
#[test]
fn the_labels_are_checked_before_the_composer_opens() {
    let dir = Utf8TempDir::new().unwrap();

    let error = compose_missing(
        dir.path(),
        &typed(),
        Command::Add {
            title: None,
            author: None,
            body: None,
            implements: None,
            labels: vec![],
        },
        &mut &b""[..],
        &mut Vec::new(),
    )
    .unwrap_err();

    assert_eq!(
        error,
        "`type` label missing: add one of type=bug, type=feature."
    );
}

/// Relabelling replaces the whole set, so dropping the required key is refused
/// and the ticket keeps the labels it had.
#[test]
fn relabelling_cannot_drop_a_required_label() {
    let dir = Utf8TempDir::new().unwrap();
    run_with_options(
        dir.path(),
        Command::Add {
            title: Some("Header misaligned".to_owned()),
            author: Some("john".to_owned()),
            body: None,
            implements: None,
            labels: vec!["type=bug".to_owned()],
        },
        &typed(),
    )
    .unwrap();
    let id = store::list(dir.path()).unwrap()[0].id;

    let error = run_with_options(
        dir.path(),
        Command::Label {
            id: Some(id),
            labels: vec!["client=cli".to_owned()],
        },
        &typed(),
    )
    .unwrap_err();

    assert_eq!(
        error,
        "`type` label missing: add one of type=bug, type=feature."
    );
    let ticket = store::list(dir.path()).unwrap().remove(0).ticket.unwrap();
    assert_eq!(ticket.metadata.labels.to_tokens(), ["type=bug"]);
}

/// An issue imported for the first time needs the labels the board requires,
/// and is refused before anything is fetched from GitHub.
#[test]
fn importing_a_new_issue_without_required_labels_fetches_nothing() {
    let dir = Utf8TempDir::new().unwrap();
    let refused = Err("`type` label missing: add one of type=bug, type=feature.".to_owned());

    let error = import_all(dir.path(), &[123], "dcdpr/jp", &refused).unwrap_err();

    assert_eq!(
        error,
        "`type` label missing: add one of type=bug, type=feature."
    );
}

/// `jp ticket labels` says how many of a bounded key a ticket carries.
#[test]
fn the_label_listing_names_each_bound() {
    let dir = Utf8TempDir::new().unwrap();

    let out = run_with_options(dir.path(), Command::Labels, &typed()).unwrap();

    assert_eq!(
        out.text,
        "client  The client.\n  client=cli\n\ntype  The type (exactly one label per ticket)\n  \
         type=bug\n  type=feature\n\n"
    );
}

/// An unreadable ticket is a warning, not mixed into stdout where it would
/// corrupt `jp -F json`.
#[test]
fn unreadable_tickets_are_warned_about_separately() {
    let dir = Utf8TempDir::new().unwrap();
    run_command(&dir, Command::Add {
        title: Some("Readable".to_owned()),
        author: Some("john".to_owned()),
        body: None,
        implements: None,
        labels: vec![],
    })
    .unwrap();
    std::fs::write(dir.path().join("zzzzzzz-mangled.md"), "no heading here\n").unwrap();

    let listed = run_json(&dir, Command::List {
        status: None,
        labels: vec![],
        query: None,
    })
    .unwrap();

    let rows: Vec<serde_json::Value> = serde_json::from_str(&listed.text).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(listed.warnings.len(), 1);
    assert!(listed.warnings[0].contains("zzzzzzz-mangled.md"));
}

/// `jp -F json` reaches the plugin as the output format, and the listing is one
/// JSON value the host prints as a record of its own.
#[test]
fn a_json_output_format_prints_the_listing_as_json() {
    let root = Utf8TempDir::new().unwrap();
    let mut message = init_message(jp_plugin::PROTOCOL_VERSION, root.path(), &["list"]);
    message.output_format = OutputFormat::Json;

    let messages = exchange(&HostToPlugin::Init(Box::new(message)));

    let prints: Vec<&PrintMessage> = messages
        .iter()
        .filter_map(|message| match message {
            PluginToHost::Print(print) => Some(print),
            _ => None,
        })
        .collect();
    let [print] = prints[..] else {
        panic!("expected one print: {messages:?}");
    };
    assert_eq!(print.format, "json");
    assert_eq!(print.text, "[]\n");
}

/// A warning names something to check now, so it is printed on stderr rather
/// than logged, where only `-v` would show it.
#[test]
fn warnings_are_printed_on_stderr() {
    let root = Utf8TempDir::new().unwrap();
    let dir = root.path().join("docs/ticket");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("zzzzzzz-mangled.md"), "no heading here\n").unwrap();

    let messages = exchange(&init(root.path(), &["list"]));

    let stderr: Vec<&str> = messages
        .iter()
        .filter_map(|message| match message {
            PluginToHost::Print(print) if print.channel == "chrome" => Some(print.text.as_str()),
            _ => None,
        })
        .collect();
    let [warning] = stderr[..] else {
        panic!("expected one warning: {messages:?}");
    };
    assert!(warning.starts_with("Warning: "), "{warning}");
    assert!(warning.contains("zzzzzzz-mangled.md"), "{warning}");
    assert!(
        !messages
            .iter()
            .any(|message| matches!(message, PluginToHost::Log(_))),
        "{messages:?}"
    );
}
