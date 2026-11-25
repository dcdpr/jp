use std::{
    collections::VecDeque,
    fs::{self, File},
    io::{self, BufReader, Cursor, Read},
    net::{SocketAddr, TcpListener},
    sync::{Arc, Mutex},
    thread,
};

use camino::{Utf8Path, Utf8PathBuf};
use camino_tempfile::Utf8TempDir;
use jp_plugin::message::{
    CreatedResponse, ErrorResponse, InterruptAction, OutputFormat, PathsInfo,
    QueryCompleteResponse, QueryOutcome, QueryRequest, ToolAction, WorkspaceInfo,
};
use pretty_assertions::assert_eq;
use serde_json::{Map, Value, json};

use super::*;
use crate::{
    anchor::{Anchor, Target},
    model::{Content, NewComment, Role},
    pass::NewPass,
    store::{Serving, Store, now},
    test_support::{CONVERSATION, Written, http},
};

/// What the file under review holds when a test starts.
const SOURCE: &str = "# Title\n\nFirst line.\nSecond line.\n";

/// One thing the host does in a session: send the plugin a message, or act
/// while the turn runs, before it sends the next one.
enum Step {
    Send(HostToPlugin),
    Act(Box<dyn FnOnce() + Send>),
}

/// The host's side of a session, played as the plugin reads it.
struct Script {
    steps: VecDeque<Step>,

    /// What is left of the message being read.
    pending: Vec<u8>,
}

impl Read for Script {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        while self.pending.is_empty() {
            match self.steps.pop_front() {
                None => return Ok(0),
                Some(Step::Act(act)) => act(),
                Some(Step::Send(message)) => {
                    self.pending = (serde_json::to_string(&message).unwrap() + "\n").into_bytes();
                }
            }
        }

        let count = self.pending.len().min(buf.len());
        buf[..count].copy_from_slice(&self.pending[..count]);
        self.pending.drain(..count);

        Ok(count)
    }
}

/// The messages the plugin sends in a session the host plays as `steps`.
fn session(steps: Vec<Step>) -> Vec<PluginToHost> {
    let script = Script {
        steps: steps.into(),
        pending: vec![],
    };
    let output = Written::default();
    run(BufReader::new(script), output.clone()).unwrap();

    output.messages()
}

/// What `messages` printed, and how the session ended.
fn ran(messages: &[PluginToHost]) -> Ran {
    let mut ran = Ran {
        text: String::new(),
        chrome: String::new(),
        code: u8::MAX,
        reason: None,
    };
    for message in messages {
        match message {
            PluginToHost::Print(print) if print.channel == "content" => {
                ran.text.push_str(&print.text);
            }
            PluginToHost::Print(print) => ran.chrome.push_str(&print.text),
            PluginToHost::Exit(exit) => (ran.code, ran.reason) = (exit.code, exit.reason.clone()),
            _ => {}
        }
    }

    ran
}

/// The query `messages` asked the host to run.
fn query_of(messages: &[PluginToHost]) -> &QueryRequest {
    messages
        .iter()
        .find_map(|message| match message {
            PluginToHost::Query(query) => Some(query),
            _ => None,
        })
        .expect("the plugin asks for a turn")
}

fn init_step(root: &Utf8Path, args: &[&str]) -> Step {
    Step::Send(HostToPlugin::Init(Box::new(init_message(
        root,
        args,
        OutputFormat::Text,
        Map::new(),
    ))))
}

fn created() -> Step {
    Step::Send(HostToPlugin::Created(CreatedResponse {
        id: Some("pass".to_owned()),
        conversation: CONVERSATION.to_owned(),
    }))
}

fn completed(outcome: QueryOutcome) -> Step {
    answered(outcome, None)
}

/// The turn ends, `answer` being the last thing the assistant said in it.
fn answered(outcome: QueryOutcome, answer: Option<&str>) -> Step {
    Step::Send(HostToPlugin::QueryComplete(QueryCompleteResponse {
        id: Some("pass".to_owned()),
        conversation: CONVERSATION.to_owned(),
        outcome,
        data: None,
        answer: answer.map(ToOwned::to_owned),
    }))
}

/// A tool call the assistant makes from [`CONVERSATION`], answered the way the
/// host has the plugin answer it, in a session of its own.
fn tool_call(root: &Utf8Path, name: &str, arguments: Value) -> Vec<PluginToHost> {
    let Value::Object(arguments) = arguments else {
        panic!("arguments are an object");
    };
    let mut init = init_message(root, &[], OutputFormat::Text, Map::new());
    init.tool = Some(ToolCall {
        action: ToolAction::Run,
        name: name.to_owned(),
        arguments,
        answers: Map::new(),
        options: Map::new(),
        access: None,
        conversation: CONVERSATION.to_owned(),
    });

    exchange(&HostToPlugin::Init(Box::new(init)))
}

/// A review of `docs/a.md` with the draft comment `#1` on `First`, returning
/// its id.
fn commented(root: &Utf8Path) -> String {
    let id = start(root);
    let ran = run_args(root, &[
        "comment",
        &id,
        "--at",
        "docs/a.md:3:1-3:6",
        "Name it.",
    ]);
    assert_eq!((ran.code, ran.reason), (0, None));

    id
}

/// The review `id` as stored.
fn stored(root: &Utf8Path, id: &str) -> Value {
    serde_json::from_str(&fs::read_to_string(review_file(root, id)).unwrap()).unwrap()
}

/// What one plugin session printed, and how it ended.
struct Ran {
    text: String,
    chrome: String,
    code: u8,
    reason: Option<String>,
}

/// A workspace holding `docs/a.md`.
fn workspace() -> Utf8TempDir {
    let root = Utf8TempDir::new().unwrap();
    fs::create_dir_all(root.path().join("docs")).unwrap();
    fs::write(root.path().join("docs/a.md"), SOURCE).unwrap();

    root
}

fn init_message(
    root: &Utf8Path,
    args: &[&str],
    format: OutputFormat,
    options: Map<String, Value>,
) -> InitMessage {
    InitMessage {
        version: jp_plugin::PROTOCOL_VERSION,
        workspace: WorkspaceInfo {
            root: root.to_path_buf(),
            storage: root.join(".jp"),
            id: "test".to_owned(),
        },
        paths: PathsInfo {
            user_workspace: Some(root.join(".user")),
            ..PathsInfo::default()
        },
        config: json!({ "user": { "name": "Jane Doe <jane@example.com>" } }),
        options,
        args: args.iter().map(|arg| (*arg).to_owned()).collect(),
        log_level: 0,
        output_format: format,
        tool: None,
    }
}

/// A browser that records the pages it is asked to open.
#[derive(Default)]
struct Opened(Mutex<Vec<String>>);

impl Browser for Opened {
    fn open(&self, url: &str) -> Result<(), String> {
        self.0.lock().unwrap().push(url.to_owned());
        Ok(())
    }
}

impl Opened {
    fn urls(&self) -> Vec<String> {
        self.0.lock().unwrap().clone()
    }
}

/// Run `args` the way the host does, printing text, opening pages in `browser`.
fn run_opening(root: &Utf8Path, args: &[&str], browser: &Opened) -> Ran {
    run_opening_as(root, args, OutputFormat::Text, browser)
}

fn run_opening_as(root: &Utf8Path, args: &[&str], format: OutputFormat, browser: &Opened) -> Ran {
    let init = HostToPlugin::Init(Box::new(init_message(root, args, format, Map::new())));
    let input = serde_json::to_string(&init).unwrap() + "\n";
    let output = Written::default();
    run_with(Cursor::new(input.into_bytes()), output.clone(), browser).unwrap();

    ran(&output.messages())
}

/// The messages the plugin sends in answer to `message`.
fn exchange(message: &HostToPlugin) -> Vec<PluginToHost> {
    let input = serde_json::to_string(message).unwrap() + "\n";
    let output = Written::default();
    run(Cursor::new(input.into_bytes()), output.clone()).unwrap();

    output.messages()
}

fn run_init(init: InitMessage) -> Ran {
    ran(&exchange(&HostToPlugin::Init(Box::new(init))))
}

/// Run `args` the way the host does, printing text.
fn run_args(root: &Utf8Path, args: &[&str]) -> Ran {
    run_init(init_message(root, args, OutputFormat::Text, Map::new()))
}

/// Run `args` the way the host does under `jp -F json`, returning the JSON
/// printed.
fn run_json(root: &Utf8Path, args: &[&str]) -> Value {
    let ran = run_init(init_message(root, args, OutputFormat::Json, Map::new()));
    assert_eq!((ran.code, ran.reason), (0, None));

    serde_json::from_str(&ran.text).unwrap()
}

/// Start a review of `docs/a.md`, returning its id.
fn start(root: &Utf8Path) -> String {
    let ran = run_args(root, &["start", "docs/a.md"]);
    assert_eq!((ran.code, ran.reason), (0, None));

    ran.text.split_once(':').unwrap().0.to_owned()
}

/// The `review.json` of the review `id`.
fn review_file(root: &Utf8Path, id: &str) -> Utf8PathBuf {
    root.join(".user/review")
        .join(id.trim_start_matches("R-"))
        .join("review.json")
}

#[test]
fn cli_definition_is_valid() {
    Args::command().debug_assert();
}

#[test]
fn the_version_is_the_plugin_version() {
    let root = Utf8TempDir::new().unwrap();

    let ran = run_args(root.path(), &["--version"]);

    assert_eq!(ran.code, 0);
    assert_eq!(
        ran.text,
        format!("jp review {}\n", env!("CARGO_PKG_VERSION"))
    );
}

#[test]
fn describe_names_the_command() {
    match exchange(&HostToPlugin::Describe).as_slice() {
        [PluginToHost::Describe(describe)] => {
            assert_eq!(describe.name, "review");
            assert_eq!(describe.manifest.command, ["review"]);
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
    let mut init = init_message(root.path(), &["ls"], OutputFormat::Text, Map::new());
    init.version = REQUIRED_PROTOCOL - 1;

    let ran = run_init(init);

    assert_eq!(ran.code, 1);
    assert!(
        ran.reason
            .as_deref()
            .is_some_and(|reason| reason.contains("needs `jp` protocol 15")),
        "{:?}",
        ran.reason
    );
}

#[test]
fn a_tool_call_is_answered_with_its_outcome() {
    let root = workspace();

    let messages = tool_call(root.path(), "resolve", json!({}));

    match messages.as_slice() {
        [
            PluginToHost::Ready(_),
            PluginToHost::ToolOutcome(outcome),
            PluginToHost::Exit(exit),
        ] => {
            assert_eq!(
                outcome.outcome,
                json!({
                    "type": "error",
                    "message": "`jp review` serves no tool named `resolve`. It serves: comment, \
                                reply.",
                    "trace": [],
                    "transient": true
                })
            );
            assert_eq!(exit.code, 0);
        }
        other => panic!("expected an outcome, got {other:?}"),
    }
}

/// The assistant can't fix the configuration, so options that don't read end
/// the call as a failed plugin run rather than as an outcome.
#[test]
fn a_tool_call_under_unreadable_options_fails_the_run() {
    let root = workspace();
    let mut init = init_message(
        root.path(),
        &[],
        OutputFormat::Text,
        Map::from_iter([("max_chars".to_owned(), json!(0))]),
    );
    init.tool = Some(ToolCall {
        action: ToolAction::Run,
        name: "reply".to_owned(),
        arguments: Map::new(),
        answers: Map::new(),
        options: Map::new(),
        access: None,
        conversation: CONVERSATION.to_owned(),
    });

    let ran = run_init(init);

    assert_eq!(
        (ran.code, ran.reason.as_deref()),
        (
            1,
            Some("`plugins.command.review.options.max_chars` must allow at least one character.")
        )
    );
}

#[test]
fn without_user_local_storage_the_store_has_to_be_configured() {
    let root = workspace();
    let mut init = init_message(root.path(), &["ls"], OutputFormat::Text, Map::new());
    init.paths.user_workspace = None;

    let ran = run_init(init);

    assert_eq!(
        (ran.code, ran.reason.as_deref()),
        (
            1,
            Some(
                "This workspace has no user-local storage to keep reviews in; set \
                 `plugins.command.review.options.dir` to a directory for them."
            )
        )
    );
}

#[test]
fn a_configured_store_is_inside_the_workspace() {
    let root = workspace();
    let options = Map::from_iter([("dir".to_owned(), json!(".jp/reviews"))]);

    let ran = run_init(init_message(
        root.path(),
        &["start", "docs/a.md"],
        OutputFormat::Text,
        options,
    ));

    assert_eq!((ran.code, ran.reason), (0, None));
    let id = ran.text.split_once(':').unwrap().0;
    assert!(
        root.path()
            .join(".jp/reviews")
            .join(id.trim_start_matches("R-"))
            .join("review.json")
            .is_file()
    );
}

#[test]
fn a_review_starts_with_its_files_as_its_first_version() {
    let root = workspace();

    let ran = run_args(root.path(), &["start", "docs/a.md", "docs/./a.md"]);

    assert_eq!((ran.code, ran.reason), (0, None));
    let id = ran.text.split_once(':').unwrap().0.to_owned();
    assert_eq!(ran.text, format!("{id}: 1 file, version 1\n"));
    assert!(id.starts_with("R-"), "{id}");

    let listed = run_args(root.path(), &["ls"]).text;
    assert!(listed.starts_with(&format!("{id}  open    ")), "{listed}");
    assert!(listed.ends_with("  docs/a.md\n"), "{listed}");

    assert_eq!(
        run_args(root.path(), &["show", &id]).text,
        format!(
            "# {id}: docs/a.md\n\n- **Status**: open\n- **Author**: Jane Doe \
             <jane@example.com>\n- **Files**: docs/a.md\n- **Versions**: 1\n- **Showing**: \
             version 1\n"
        )
    );
}

#[test]
fn a_review_of_a_missing_file_is_refused() {
    let root = workspace();

    let ran = run_args(root.path(), &["start", "docs/missing.md"]);

    assert_eq!(
        (ran.code, ran.reason.as_deref()),
        (1, Some("No file at docs/missing.md."))
    );
}

#[test]
fn an_unknown_review_is_named() {
    let root = workspace();

    let ran = run_args(root.path(), &["show", "R-0000000"]);

    assert_eq!(
        (ran.code, ran.reason.as_deref()),
        (1, Some("No review R-0000000."))
    );
}

#[test]
fn listing_an_empty_store_says_so() {
    let root = workspace();

    let ran = run_args(root.path(), &["ls"]);

    assert_eq!(
        (ran.text.as_str(), ran.chrome.as_str()),
        ("", "No reviews.\n")
    );
}

/// A comment written against one version is shown where its lines moved in a
/// later one, and where it was written in the earlier one.
#[test]
fn a_comment_follows_its_lines_into_a_later_version() {
    let root = workspace();
    let id = start(root.path());

    let ran = run_args(root.path(), &[
        "comment",
        &id,
        "--at",
        "docs/a.md:3:1-3:6",
        "Name it.",
    ]);
    assert_eq!(ran.text, format!("{id}#1\n"));

    fs::write(
        root.path().join("docs/a.md"),
        "# Title\n\nIntro.\n\nFirst line.\nSecond line.\n",
    )
    .unwrap();
    let ran = run_args(root.path(), &[
        "comment",
        &id,
        "--at",
        "docs/a.md:3",
        "Why?",
    ]);
    assert_eq!(ran.text, format!("{id}#2\n"));

    let shown = run_args(root.path(), &["show", &id]).text;
    assert!(shown.contains("- **Versions**: 1, 2\n"), "{shown}");
    assert!(
        shown.contains(&format!(
            "## {id}#1 (draft)\n\nOn docs/a.md:5:1-5:6, by Jane Doe <jane@example.com>, "
        )),
        "{shown}"
    );
    assert!(shown.contains("\n\n> First\n\nName it.\n"), "{shown}");

    let earlier = run_args(root.path(), &["show", &id, "--version", "1"]).text;
    assert!(
        earlier.contains("On docs/a.md:3:1-3:6, by Jane Doe"),
        "{earlier}"
    );
    assert!(
        earlier.contains("On docs/a.md:3 in version 2, outdated in version 1, by Jane Doe"),
        "{earlier}"
    );
}

#[test]
fn a_comment_on_a_changed_line_is_outdated() {
    let root = workspace();
    let id = start(root.path());
    run_args(root.path(), &[
        "comment",
        &id,
        "--at",
        "docs/a.md:4",
        "Too long.",
    ]);
    fs::write(
        root.path().join("docs/a.md"),
        "# Title\n\nFirst line.\nSecond line, edited.\n",
    )
    .unwrap();
    run_args(root.path(), &[
        "comment",
        &id,
        "--at",
        "docs/a.md:1",
        "Title?",
    ]);

    let shown = run_args(root.path(), &["show", &id]).text;
    assert!(
        shown.contains("On docs/a.md:4 in version 1, outdated since version 2, by Jane Doe"),
        "{shown}"
    );
    assert!(
        shown.contains("\n\n> Second line.\n\nToo long.\n"),
        "{shown}"
    );

    let detail = run_json(root.path(), &["show", &id]);
    assert_eq!(detail["version"], 2);
    assert_eq!(
        detail["comments"][0]["anchor"],
        json!({
            "version": 1,
            "path": "docs/a.md",
            "range": "4",
            "outdated": true,
            "resolved": { "version": 1, "range": "4", "bytes": [21, 33] }
        })
    );
    assert_eq!(detail["comments"][0]["excerpt"], "Second line.");
    assert_eq!(
        detail["comments"][1]["anchor"],
        json!({
            "version": 2,
            "path": "docs/a.md",
            "range": "1",
            "projected": { "version": 2, "range": "1", "bytes": [0, 7] },
            "outdated": false
        })
    );
}

/// A comment outdated now may have followed its lines through later versions;
/// it is shown where it last stood.
#[test]
fn an_outdated_comment_names_the_last_version_its_text_stood_in() {
    let root = workspace();
    let id = start(root.path());
    let comment_at = |at: &str| {
        let ran = run_args(root.path(), &["comment", &id, "--at", at, "Noted."]);
        assert_eq!((ran.code, ran.reason), (0, None));
    };
    comment_at("docs/a.md:4");
    fs::write(
        root.path().join("docs/a.md"),
        "# Heading\n\nFirst line.\nSecond line.\n",
    )
    .unwrap();
    comment_at("docs/a.md:1");
    fs::write(
        root.path().join("docs/a.md"),
        "# Heading\n\nFirst line.\nSecond line, edited.\n",
    )
    .unwrap();
    comment_at("docs/a.md:1");

    let shown = run_args(root.path(), &["show", &id]).text;
    let detail = run_json(root.path(), &["show", &id]);

    assert!(
        shown.contains("On docs/a.md:4 in version 1, outdated since version 3, by "),
        "{shown}"
    );
    assert_eq!(detail["version"], 3);
    assert_eq!(
        detail["comments"][0]["anchor"]["resolved"],
        json!({ "version": 2, "range": "4", "bytes": [23, 35] })
    );
}

#[test]
fn diff_shows_what_changed_between_two_versions() {
    let root = workspace();
    let id = start(root.path());
    fs::write(
        root.path().join("docs/a.md"),
        "# Title\n\nFirst line.\nSecond line, edited.\n",
    )
    .unwrap();
    run_args(root.path(), &[
        "comment",
        &id,
        "--at",
        "docs/a.md:1",
        "Title?",
    ]);

    assert_eq!(
        run_args(root.path(), &["diff", &id]).text,
        "--- v1/docs/a.md\n+++ v2/docs/a.md\n@@ -1,4 +1,4 @@\n # Title\n \n First line.\n-Second \
         line.\n+Second line, edited.\n"
    );

    let unchanged = run_args(root.path(), &["diff", &id, "--from", "2"]);
    assert_eq!(
        (unchanged.text.as_str(), unchanged.chrome.as_str()),
        ("", "Nothing changed from version 2 to version 2.\n")
    );

    let missing = run_args(root.path(), &["diff", &id, "--to", "3"]);
    assert_eq!(
        missing.reason,
        Some(format!("{id} has no version 3; its latest is 2."))
    );

    let changes = run_json(root.path(), &["diff", &id]);
    assert_eq!(changes["from"], 1);
    assert_eq!(changes["to"], 2);
    assert_eq!(changes["files"][0]["path"], "docs/a.md");
    assert_eq!(changes["files"][0]["change"], "modified");
}

#[test]
fn a_suggestion_replaces_a_range() {
    let root = workspace();
    let id = start(root.path());

    let refused = run_args(root.path(), &[
        "comment",
        &id,
        "--at",
        "docs/a.md",
        "--replace",
        "x",
    ]);
    assert_eq!(
        refused.reason.as_deref(),
        Some("A suggestion replaces a range of a file, and the comment is on none.")
    );

    let ran = run_args(root.path(), &[
        "comment",
        &id,
        "--at",
        "docs/a.md:4",
        "--replace",
        "Second.",
    ]);
    assert_eq!(ran.text, format!("{id}#1\n"));

    let shown = run_args(root.path(), &["show", &id]).text;
    assert!(
        shown.contains(&format!("## {id}#1 (draft suggestion)\n")),
        "{shown}"
    );
    assert!(
        shown.contains("\n```diff\n-Second line.\n+Second.\n```\n"),
        "{shown}"
    );
}

#[test]
fn a_place_outside_the_review_is_refused() {
    let root = workspace();
    let id = start(root.path());

    let outside = run_args(root.path(), &["comment", &id, "--at", "docs/a.md:9", "x"]);
    assert_eq!(
        outside.reason.as_deref(),
        Some("docs/a.md:9 lies outside docs/a.md as version 1 holds it.")
    );

    let untracked = run_args(root.path(), &["comment", &id, "--at", "docs/b.md:1", "x"]);
    assert_eq!(
        untracked.reason.as_deref(),
        Some("`docs/b.md:1` names no file in the review, which tracks docs/a.md.")
    );
}

#[test]
fn a_draft_is_edited_withdrawn_and_its_number_retired() {
    let root = workspace();
    let id = start(root.path());
    run_args(root.path(), &["comment", &id, "One."]);
    run_args(root.path(), &["comment", &id, "Two."]);

    let edited = run_args(root.path(), &["edit", &format!("{id}#1"), "One, better."]);
    assert_eq!(edited.text, format!("Edited {id}#1.\n"));
    let withdrawn = run_args(root.path(), &["withdraw", &format!("{id}#2")]);
    assert_eq!(withdrawn.text, format!("Withdrew {id}#2.\n"));
    let third = run_args(root.path(), &["comment", &id, "Three."]);
    assert_eq!(third.text, format!("{id}#3\n"));

    let shown = run_args(root.path(), &["show", &id]).text;
    assert!(
        shown.contains(&format!("## {id}#1 (draft)\n\nOn the review, by Jane Doe")),
        "{shown}"
    );
    assert!(shown.contains("\n\nOne, better.\n"), "{shown}");
    assert!(!shown.contains(&format!("{id}#2")), "{shown}");
}

#[test]
fn editing_a_comment_moves_it_to_where_the_files_are_now() {
    let root = workspace();
    let id = start(root.path());
    run_args(root.path(), &["comment", &id, "--at", "docs/a.md:3", "x"]);
    fs::write(root.path().join("docs/a.md"), "# Title\n\nNew.\n").unwrap();

    let edited = run_args(root.path(), &[
        "edit",
        &format!("{id}#1"),
        "--at",
        "docs/a.md:3",
    ]);

    assert_eq!(edited.text, format!("Edited {id}#1.\n"));
    let comment = run_json(root.path(), &["show", &id])["comments"][0].clone();
    assert_eq!(comment["anchor"]["version"], 2);
    assert_eq!(comment["excerpt"], "New.");
}

#[test]
fn a_draft_takes_no_reply_and_is_not_resolved() {
    let root = workspace();
    let id = start(root.path());
    run_args(root.path(), &["comment", &id, "One."]);
    let comment = format!("{id}#1");

    let replied = run_args(root.path(), &["reply", &comment, "Sure?"]);
    assert_eq!(
        replied.reason,
        Some(format!("{comment} is a draft, not yet sent."))
    );
    let resolved = run_args(root.path(), &["resolve", &comment]);
    assert_eq!(
        resolved.reason,
        Some(format!("{comment} is a draft, not yet sent."))
    );

    let accepted = run_args(root.path(), &["resolve", "--accepted", &id]);
    assert_eq!(
        (accepted.code, accepted.chrome),
        (
            0,
            format!("No open comment in {id} ends with the assistant accepting it.\n")
        )
    );
}

/// Comments go to the assistant in passes, which this plugin doesn't run yet,
/// so the test writes what a pass leaves behind into `review.json` itself.
#[test]
fn a_sent_comment_takes_replies_and_is_resolved() {
    let root = workspace();
    let id = start(root.path());
    run_args(root.path(), &["comment", &id, "One."]);
    run_args(root.path(), &["comment", &id, "Two."]);
    let path = review_file(root.path(), &id);
    let mut stored: Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
    for comment in stored["comments"].as_array_mut().unwrap() {
        comment["pass"] = json!(1);
        comment["replies"] = json!([{
            "role": "assistant",
            "author": "jp",
            "created_at": "2026-11-17T10:05:00Z",
            "body": "Fixed.",
            "verdict": "accept"
        }]);
    }
    fs::write(&path, serde_json::to_string(&stored).unwrap()).unwrap();
    let second = format!("{id}#2");

    let replied = run_args(root.path(), &["reply", &second, "Not quite."]);
    assert_eq!(replied.text, format!("Drafted a reply to {second}.\n"));
    let edited = run_args(root.path(), &["edit", &second, "Almost."]);
    assert_eq!(
        edited.text,
        format!("Edited the draft reply to {second}.\n")
    );

    let resolved = run_args(root.path(), &["resolve", "--accepted", &id]);
    assert_eq!(resolved.text, format!("Resolved {id}#1.\n"));

    let withdrawn = run_args(root.path(), &["withdraw", &second]);
    assert_eq!(
        withdrawn.text,
        format!("Withdrew the draft reply to {second}.\n")
    );
    let resolved = run_args(root.path(), &["resolve", &second, &format!("{id}#1")]);
    assert_eq!(
        resolved.text,
        format!("Resolved {second}.\n{id}#1 is resolved already.\n")
    );

    let shown = run_args(root.path(), &["show", &id]).text;
    assert!(
        shown.contains(&format!("## {id}#1 (resolved)\n")),
        "{shown}"
    );
    assert!(
        shown.contains("\n### jp, accept, 2026-11-17T10:05:00Z\n\nFixed.\n"),
        "{shown}"
    );
}

/// The pass sent comment 1 and its turn left it unanswered, so the reviewer
/// wrote in it last.
#[test]
fn a_resolved_comment_is_opened_again() {
    let root = workspace();
    let id = commented(root.path());
    session(vec![
        init_step(root.path(), &["submit", &id]),
        created(),
        completed(QueryOutcome::Finished),
    ]);
    run_args(root.path(), &["comment", &id, "Rename it."]);
    let (first, draft) = (format!("{id}#1"), format!("{id}#2"));
    run_args(root.path(), &["resolve", &first]);

    assert_eq!(
        run_args(root.path(), &["reopen", &first]).text,
        format!("Reopened {first}.\n")
    );
    assert_eq!(
        run_args(root.path(), &["reopen", &first]).text,
        format!("{first} is open already.\n")
    );
    assert_eq!(
        run_args(root.path(), &["reopen", &draft]).reason,
        Some(format!("{draft} is a draft, not yet sent."))
    );

    let reopened = run_json(root.path(), &["show", &id])["comments"][0].clone();
    assert_eq!(reopened["status"], "open");
    assert_eq!(reopened.get("resolved_at"), None);

    run_args(root.path(), &["resolve", &first]);
    run_args(root.path(), &["close", &id]);
    assert_eq!(
        run_args(root.path(), &["reopen", &first]).reason,
        Some(format!("{id} is closed."))
    );
}

#[test]
fn a_closed_review_takes_no_changes() {
    let root = workspace();
    let id = start(root.path());

    assert_eq!(
        run_args(root.path(), &["close", &id]).text,
        format!("Closed {id}.\n")
    );
    assert_eq!(
        run_args(root.path(), &["close", &id]).text,
        format!("{id} is closed already.\n")
    );

    let refused = run_args(root.path(), &["comment", &id, "Late."]);
    assert_eq!(refused.reason, Some(format!("{id} is closed.")));
    let listed = run_args(root.path(), &["ls"]).text;
    assert!(listed.starts_with(&format!("{id}  closed  ")), "{listed}");
}

#[test]
fn a_closed_review_is_opened_again() {
    let root = workspace();
    let id = start(root.path());
    run_args(root.path(), &["close", &id]);

    assert_eq!(
        run_args(root.path(), &["reopen", &id]).text,
        format!("Reopened {id}.\n")
    );
    assert_eq!(
        run_args(root.path(), &["reopen", &id]).text,
        format!("{id} is open already.\n")
    );

    let commented = run_args(root.path(), &["comment", &id, "Not done yet."]);
    assert_eq!(commented.reason, None);
    let listed = run_args(root.path(), &["ls"]).text;
    assert!(listed.starts_with(&format!("{id}  open  ")), "{listed}");
}

#[test]
fn a_passage_is_approved_and_a_file_read_to_a_line() {
    let root = workspace();
    let id = start(root.path());

    assert_eq!(
        run_args(root.path(), &["approve", &id, "--at", "docs/a.md:3"]).text,
        "Approved docs/a.md:3, as approval 1.\n"
    );
    assert_eq!(
        run_args(root.path(), &["read-until", &id, "--at", "docs/a.md:3"]).text,
        "Read docs/a.md through line 3.\n"
    );

    let shown = run_args(root.path(), &["show", &id]).text;
    assert!(
        shown.contains(
            "\n## Read so far\n\n- docs/a.md through line 3\n\n## Approved\n\n- 1: docs/a.md:3\n"
        ),
        "{shown}"
    );

    assert_eq!(
        run_args(root.path(), &["unapprove", &id, "1"]).text,
        "Took back approval 1.\n"
    );
    assert_eq!(
        run_args(root.path(), &["unapprove", &id, "1"]).reason,
        Some(format!("{id} has no approval 1."))
    );
    let clear = ["read-until", &id, "--at", "docs/a.md", "--clear"];
    assert_eq!(
        run_args(root.path(), &clear).text,
        "Forgot how far docs/a.md was read.\n"
    );
    assert_eq!(
        run_args(root.path(), &clear).text,
        format!("{id} doesn't say how far docs/a.md was read.\n")
    );
}

#[test]
fn approving_and_reading_need_lines_to_name() {
    let root = workspace();
    let id = start(root.path());

    assert_eq!(
        run_args(root.path(), &["approve", &id, "--at", "docs/a.md"]).reason,
        Some("An approval is of a range of a file: lines, or a span within them.".to_owned())
    );
    assert_eq!(
        run_args(root.path(), &["read-until", &id, "--at", "docs/a.md"]).reason,
        Some(
            "`docs/a.md` names no line; name the last line read, as in `docs/a.md:120`.".to_owned()
        )
    );

    run_args(root.path(), &["close", &id]);
    assert_eq!(
        run_args(root.path(), &["approve", &id, "--at", "docs/a.md:3"]).reason,
        Some(format!("{id} is closed."))
    );
    assert_eq!(
        run_args(root.path(), &["read-until", &id, "--at", "docs/a.md:3"]).reason,
        Some(format!("{id} is closed."))
    );
}

/// Closing is the reviewer's call, but comments left behind are easy to miss.
#[test]
fn closing_with_comments_left_open_or_unsent_warns_about_them() {
    let root = workspace();
    let id = commented(root.path());
    session(vec![
        init_step(root.path(), &["submit", &id]),
        created(),
        completed(QueryOutcome::Finished),
    ]);
    run_args(root.path(), &["comment", &id, "Rename it."]);

    let closed = run_args(root.path(), &["close", &id]);
    let again = run_args(root.path(), &["close", &id]);

    assert_eq!(closed.text, format!("Closed {id}.\n"));
    assert_eq!(
        closed.chrome,
        "Warning: Closed with comments #1 still open and #2 never sent.\n"
    );
    assert_eq!(again.chrome, "");
}

#[test]
fn every_command_prints_json_under_a_json_format() {
    let root = workspace();

    let started = run_json(root.path(), &[
        "start",
        "docs/a.md",
        "--title",
        "First pass",
    ]);
    let id = started["id"].as_str().unwrap().to_owned();
    assert_eq!(started["title"], "First pass");
    assert_eq!(started["status"], "open");
    assert_eq!(started["files"], json!(["docs/a.md"]));
    assert_eq!(started["latest"], 1);
    assert_eq!(
        started["comments"],
        json!({ "draft": 0, "open": 0, "resolved": 0 })
    );

    let comment = run_json(root.path(), &[
        "comment",
        &id,
        "--at",
        "docs/a.md:3:1-3:6",
        "Name it.",
    ]);
    assert_eq!(comment["id"], format!("{id}#1"));
    assert_eq!(comment["status"], "draft");
    assert_eq!(comment["role"], "reviewer");
    assert_eq!(comment["excerpt"], "First");
    assert_eq!(
        comment["anchor"]["projected"],
        json!({ "version": 1, "range": "3:1-3:6", "bytes": [9, 14] })
    );

    let listed = run_json(root.path(), &["ls"]);
    assert_eq!(listed[0]["id"], id);
    assert_eq!(listed[0]["comments"]["draft"], 1);

    let withdrawn = run_json(root.path(), &["withdraw", &format!("{id}#1")]);
    assert_eq!(
        withdrawn,
        json!({ "id": format!("{id}#1"), "withdrawn": "comment" })
    );

    let closed = run_json(root.path(), &["close", &id]);
    assert_eq!(closed["status"], "closed");
}

/// Version numbers don't depend on what the command printed: with `base` set,
/// the working tree is always version 2.
#[cfg(unix)]
#[test]
fn a_base_command_that_prints_nothing_still_records_version_1() {
    let root = workspace();
    let options = Map::from_iter([("base".to_owned(), json!(["false"]))]);

    let ran = run_init(init_message(
        root.path(),
        &["start", "docs/a.md"],
        OutputFormat::Text,
        options,
    ));

    assert_eq!((ran.code, ran.reason), (0, None));
    let id = ran.text.split_once(':').unwrap().0.to_owned();
    assert_eq!(
        ran.text,
        format!("{id}: 1 file, version 2 (version 1 is the base)\n")
    );
    assert!(
        run_args(root.path(), &["diff", &id])
            .text
            .starts_with("--- /dev/null\n+++ v2/docs/a.md\n")
    );
}

#[cfg(unix)]
#[test]
fn the_base_flags_win_over_the_option() {
    let root = workspace();
    fs::write(root.path().join("docs/a.md.orig"), "# Title\n").unwrap();
    let options = Map::from_iter([("base".to_owned(), json!(["false"]))]);
    let start = |args: &[&str]| {
        run_init(init_message(
            root.path(),
            args,
            OutputFormat::Text,
            options.clone(),
        ))
    };

    let based = start(&["start", "docs/a.md", "--base", "cat '{path}.orig'"]);
    let unbased = start(&["start", "docs/a.md", "--no-base"]);
    let unclosed = start(&["start", "docs/a.md", "--base", "cat '{path}"]);

    let id = based.text.split_once(':').unwrap().0.to_owned();
    assert_eq!(
        based.text,
        format!("{id}: 1 file, version 2 (version 1 is the base)\n")
    );
    assert_eq!(
        run_args(root.path(), &["diff", &id]).text,
        "--- v1/docs/a.md\n+++ v2/docs/a.md\n@@ -1 +1,4 @@\n # Title\n+\n+First line.\n+Second \
         line.\n"
    );
    let unbased_id = unbased.text.split_once(':').unwrap().0;
    assert_eq!(unbased.text, format!("{unbased_id}: 1 file, version 1\n"));
    assert!(
        unclosed
            .reason
            .is_some_and(|reason| reason.contains("a quote in it isn't closed"))
    );
}

#[cfg(unix)]
#[test]
fn the_base_is_recorded_before_the_working_tree() {
    let root = workspace();
    fs::write(
        root.path().join("docs/a.md.orig"),
        "# Title\n\nFirst line.\n",
    )
    .unwrap();
    fs::write(root.path().join("docs/new.md"), "new\n").unwrap();
    let options = Map::from_iter([("base".to_owned(), json!(["cat", "{path}.orig"]))]);

    let ran = run_init(init_message(
        root.path(),
        &["start", "docs/a.md", "docs/new.md"],
        OutputFormat::Text,
        options,
    ));

    assert_eq!((ran.code, ran.reason), (0, None));
    let id = ran.text.split_once(':').unwrap().0.to_owned();
    assert_eq!(
        ran.text,
        format!("{id}: 2 files, version 2 (version 1 is the base)\n")
    );
    // `docs/new.md` has no `.orig`, so it has no base, and reads as added.
    assert_eq!(
        run_args(root.path(), &["diff", &id]).text,
        "--- v1/docs/a.md\n+++ v2/docs/a.md\n@@ -1,3 +1,4 @@\n # Title\n \n First line.\n+Second \
         line.\n--- /dev/null\n+++ v2/docs/new.md\n@@ -0,0 +1 @@\n+new\n"
    );
}

/// The whole loop from the terminal: the pass goes to the host as a turn on a
/// new conversation, the assistant's tool call lands in the store while the
/// turn runs and is printed, and its edit is recorded as the next version.
#[test]
fn a_pass_runs_a_turn_and_prints_what_the_assistant_writes() {
    let root = workspace();
    let id = commented(root.path());
    let (path, review) = (root.path().to_owned(), id.clone());

    let messages = session(vec![
        init_step(root.path(), &["submit", &id, "-m", " First half only. "]),
        created(),
        Step::Act(Box::new(move || {
            fs::write(
                path.join("docs/a.md"),
                "# Title\n\nThe first line.\nSecond line.\n",
            )
            .unwrap();
            let answered = tool_call(
                &path,
                "reply",
                json!({ "comment": format!("{review}#1"), "verdict": "accept", "body": "Fixed." }),
            );
            assert!(
                matches!(answered.get(1), Some(PluginToHost::ToolOutcome(outcome))
                    if outcome.outcome["type"] == "success"),
                "{answered:?}"
            );
        })),
        completed(QueryOutcome::Finished),
    ]);

    let query = query_of(&messages);
    assert!(query.new);
    assert_eq!(query.title, Some(format!("Review {id}: docs/a.md")));
    assert_eq!(query.cfg, [
        r#"plugins.command.review.options.assistant:="assistant""#,
        "plugins.command.review.options.max_chars:=600",
    ]);
    assert!(
        query.content.starts_with(&format!(
            "# Review {id}, pass 1\n\nFirst half only.\n\n## docs/a.md (version 1)\n"
        )),
        "{}",
        query.content
    );
    let ran = ran(&messages);
    assert_eq!((ran.code, ran.reason), (0, None));
    assert_eq!(
        ran.text,
        format!(
            "Pass 1 on a new conversation, {CONVERSATION}.\n#1  accept   Fixed.\nVersion 2: 1 \
             file changed.\n"
        )
    );

    let shown = run_args(root.path(), &["show", &id]).text;
    assert!(
        shown.contains(&format!(
            "- **Conversation**: {CONVERSATION}\n- **Passes**: 1 completed\n"
        )),
        "{shown}"
    );
    assert!(shown.contains(&format!("## {id}#1 (open)\n")), "{shown}");
}

/// `diff` compares the latest two versions, which after the reviewer's own
/// edits no longer hold what the pass changed; `--pass` still does.
#[test]
fn diff_pass_shows_what_a_pass_changed() {
    let root = workspace();
    let id = commented(root.path());
    let path = root.path().to_owned();
    session(vec![
        init_step(root.path(), &["submit", &id, "-m", "Go."]),
        created(),
        Step::Act(Box::new(move || {
            fs::write(
                path.join("docs/a.md"),
                "# Title\n\nThe first line.\nSecond line.\n",
            )
            .unwrap();
        })),
        completed(QueryOutcome::Finished),
    ]);
    fs::write(
        root.path().join("docs/a.md"),
        "# Title\n\nThe first line.\nThe second line.\n",
    )
    .unwrap();
    // Commenting on the edited file records it as version 3.
    let commented = run_args(root.path(), &[
        "comment",
        &id,
        "--at",
        "docs/a.md:4",
        "Better.",
    ]);
    assert_eq!((commented.code, commented.reason), (0, None));

    let by_pass = "--- v1/docs/a.md\n+++ v2/docs/a.md\n@@ -1,4 +1,4 @@\n # Title\n \n-First \
                   line.\n+The first line.\n Second line.\n";
    assert_eq!(
        run_args(root.path(), &["diff", &id, "--pass", "last"]).text,
        by_pass
    );
    assert_eq!(
        run_args(root.path(), &["diff", &id, "--pass", "1"]).text,
        by_pass
    );
    assert!(
        run_args(root.path(), &["diff", &id])
            .text
            .starts_with("--- v2/docs/a.md\n+++ v3/docs/a.md\n")
    );
    assert_eq!(
        run_args(root.path(), &["diff", &id, "--pass", "2"]).reason,
        Some(format!("{id} has no pass 2."))
    );
    assert!(
        run_args(root.path(), &["diff", &id, "--pass", "1", "--from", "1"])
            .reason
            .is_some_and(|reason| reason.contains("cannot be used with"))
    );
    assert!(
        run_args(root.path(), &["diff", &id, "--pass", "first"])
            .reason
            .is_some_and(|reason| reason.contains("write a pass's number, such as `2`, or `last`"))
    );
}

#[test]
fn a_pass_that_changed_nothing_says_so() {
    let root = workspace();
    let id = commented(root.path());
    session(vec![
        init_step(root.path(), &["submit", &id]),
        created(),
        completed(QueryOutcome::Finished),
    ]);

    let ran = run_args(root.path(), &["diff", &id, "--pass", "last"]);

    assert_eq!((ran.text.as_str(), ran.code), ("", 0));
    assert_eq!(ran.chrome, "Pass 1 changed nothing in the files.\n");
}

/// The assistant's tool calls find the review where `--dir` put it only because
/// the pass hands the directory to the turn.
#[test]
fn dir_on_the_command_line_stores_reviews_there_and_goes_with_each_pass() {
    let root = workspace();

    let started = run_args(root.path(), &["--dir", "elsewhere", "start", "docs/a.md"]);
    assert_eq!((started.code, started.reason.clone()), (0, None));
    let id = started.text.split_once(':').unwrap().0.to_owned();
    let messages = session(vec![
        init_step(root.path(), &[
            "--dir",
            "elsewhere",
            "submit",
            &id,
            "-m",
            "Go.",
        ]),
        created(),
        completed(QueryOutcome::Finished),
    ]);

    assert!(
        root.path()
            .join("elsewhere")
            .join(id.trim_start_matches("R-"))
            .join("review.json")
            .is_file()
    );
    assert!(!review_file(root.path(), &id).exists());
    assert!(
        query_of(&messages)
            .cfg
            .contains(&r#"plugins.command.review.options.dir:="elsewhere""#.to_owned()),
        "{:?}",
        query_of(&messages).cfg
    );
    assert!(
        run_args(root.path(), &["--dir", " ", "ls"])
            .reason
            .is_some_and(|reason| reason.contains("'--dir <DIR>': it is empty"))
    );
}

/// The turn ran, so how it ended is recorded, with why its files weren't.
#[test]
fn a_pass_whose_files_cant_be_read_after_the_turn_still_records_its_outcome() {
    let root = workspace();
    let id = commented(root.path());
    let path = root.path().to_owned();

    let messages = session(vec![
        init_step(root.path(), &["submit", &id]),
        created(),
        Step::Act(Box::new(move || {
            fs::write(path.join("docs/a.md"), [0xff, 0xfe]).unwrap();
        })),
        completed(QueryOutcome::Finished),
    ]);

    let ran = ran(&messages);
    let unread = format!(
        "The files couldn't be recorded after the turn: {} is not UTF-8 text, which a review \
         can't comment on.",
        root.path().join("docs/a.md")
    );
    assert_eq!((ran.code, ran.reason), (0, None));
    assert!(
        ran.chrome.contains(&format!("Warning: {unread}\n")),
        "{}",
        ran.chrome
    );
    let ended = &stored(root.path(), &id)["passes"][0]["ended"];
    assert_eq!(ended["outcome"], "completed");
    assert_eq!(ended["version"], Value::Null);
    assert_eq!(ended["error"], unread);
}

/// The files went into the conversation once, so the next pass sends what
/// changed since; the first pass left `#1` unanswered, so it goes again, with
/// the reviewer's reply after it.
#[test]
fn a_later_pass_continues_the_conversation_with_what_changed() {
    let root = workspace();
    let id = commented(root.path());
    session(vec![
        init_step(root.path(), &["submit", &id]),
        created(),
        completed(QueryOutcome::Finished),
    ]);
    fs::write(
        root.path().join("docs/a.md"),
        "# Title\n\nFirst line.\nSecond line, edited.\n",
    )
    .unwrap();
    assert_eq!(
        run_args(root.path(), &["reply", &format!("{id}#1"), "Still vague."]).code,
        0
    );

    let messages = session(vec![
        init_step(root.path(), &["submit", &id]),
        completed(QueryOutcome::Finished),
    ]);

    let query = query_of(&messages);
    assert_eq!(
        (query.new, query.conversation.as_str()),
        (false, CONVERSATION)
    );
    assert_eq!(query.title, None);
    assert!(
        query.content.contains("## Changes since pass 1 was sent\n"),
        "{}",
        query.content
    );
    assert!(
        query.content.ends_with(&format!(
            "### {id}#1 at docs/a.md:3:1-3:6\n\n> First\n\nName it.\n\n#### Reply\n\nStill \
             vague.\n"
        )),
        "{}",
        query.content
    );
    assert!(
        ran(&messages)
            .text
            .starts_with(&format!("Pass 2 on {CONVERSATION}.\n")),
        "{}",
        ran(&messages).text
    );
}

/// The version recorded after a turn also holds what the reviewer saved while
/// it ran, which the assistant never read, so the next pass diffs from what the
/// assistant was sent.
#[test]
fn an_edit_saved_during_a_turn_goes_with_the_next_pass() {
    let root = workspace();
    fs::write(root.path().join("docs/b.md"), "Draft.\n").unwrap();
    let started = run_args(root.path(), &["start", "docs/a.md", "docs/b.md"]);
    let id = started.text.split_once(':').unwrap().0.to_owned();
    let path = root.path().to_owned();
    session(vec![
        init_step(root.path(), &["submit", &id, "-m", "Go."]),
        created(),
        Step::Act(Box::new(move || {
            // The assistant edits one file while the reviewer saves the other.
            fs::write(
                path.join("docs/a.md"),
                "# Title\n\nThe first line.\nSecond line.\n",
            )
            .unwrap();
            fs::write(path.join("docs/b.md"), "Draft, reworded.\n").unwrap();
        })),
        completed(QueryOutcome::Finished),
    ]);

    let messages = session(vec![
        init_step(root.path(), &["submit", &id, "-m", "Go on."]),
        completed(QueryOutcome::Finished),
    ]);

    let content = &query_of(&messages).content;
    assert!(
        content.contains(
            "--- v1/docs/b.md\n+++ v2/docs/b.md\n@@ -1 +1 @@\n-Draft.\n+Draft, reworded.\n"
        ),
        "{content}"
    );
    assert!(content.contains("+The first line.\n"), "{content}");
}

#[test]
fn a_stopped_pass_leaves_what_it_did_not_answer_for_the_next() {
    let root = workspace();
    let id = commented(root.path());

    let messages = session(vec![
        init_step(root.path(), &["submit", &id]),
        created(),
        completed(QueryOutcome::Stopped),
    ]);

    let ran = ran(&messages);
    assert_eq!((ran.code, ran.reason), (0, None));
    assert_eq!(
        ran.text,
        format!(
            "Pass 1 on a new conversation, {CONVERSATION}.\nPass 1 stopped.\nNo file changed.\nNo \
             reply to #1; the next submit sends it again.\n"
        )
    );
    assert_eq!(
        stored(root.path(), &id)["passes"][0]["ended"]["outcome"],
        "stopped"
    );
}

#[test]
fn a_failed_pass_ends_the_command_with_the_hosts_error() {
    let root = workspace();
    let id = commented(root.path());

    let messages = session(vec![
        init_step(root.path(), &["submit", &id]),
        Step::Send(HostToPlugin::Error(ErrorResponse {
            id: Some("pass".to_owned()),
            request: Some("query".to_owned()),
            message: "the provider is down".to_owned(),
        })),
    ]);

    let ran = ran(&messages);
    assert_eq!(
        (ran.code, ran.reason.as_deref()),
        (1, Some("Pass 1 failed: the provider is down"))
    );
    let review = stored(root.path(), &id);
    assert_eq!(review["passes"][0]["ended"]["outcome"], "failed");
    assert_eq!(
        review["passes"][0]["ended"]["error"],
        "the provider is down"
    );
    assert_eq!(review.get("conversation"), None);
}

#[test]
fn a_pass_whose_host_goes_away_is_interrupted() {
    let root = workspace();
    let id = commented(root.path());

    let messages = session(vec![init_step(root.path(), &["submit", &id]), created()]);

    let ran = ran(&messages);
    assert_eq!(
        (ran.code, ran.reason.as_deref()),
        (
            1,
            Some(
                "Pass 1 was interrupted: The host stopped responding. Are `jp` and `jp-review` \
                 the same build?"
            )
        )
    );
    assert_eq!(
        stored(root.path(), &id)["passes"][0]["ended"]["outcome"],
        "interrupted"
    );
}

/// The host is shutting down around the turn, so the plugin asks it to stop the
/// turn and still waits to learn how it ended.
#[test]
fn a_shutdown_stops_the_turn() {
    let root = workspace();
    let id = commented(root.path());

    let messages = session(vec![
        init_step(root.path(), &["submit", &id]),
        created(),
        Step::Send(HostToPlugin::Shutdown),
        completed(QueryOutcome::Stopped),
    ]);

    let stops: Vec<_> = messages
        .iter()
        .filter_map(|message| match message {
            PluginToHost::Interrupt(interrupt) => Some(interrupt),
            _ => None,
        })
        .collect();
    assert_eq!(stops.len(), 1);
    assert_eq!(
        (stops[0].conversation.as_str(), stops[0].action),
        (CONVERSATION, InterruptAction::Stop)
    );
    assert_eq!(ran(&messages).code, 0);
}

/// A conversation named with `--id` holds the files' context already, so the
/// pass runs there, and the review keeps to it once the turn ran.
#[test]
fn a_pass_runs_in_the_conversation_id_names() {
    let root = workspace();
    let id = commented(root.path());

    let messages = session(vec![
        init_step(root.path(), &["submit", &id, "--id", "17629384710"]),
        completed(QueryOutcome::Finished),
    ]);

    let query = query_of(&messages);
    assert_eq!(
        (query.new, query.conversation.as_str()),
        (false, CONVERSATION)
    );
    assert!(
        ran(&messages)
            .text
            .starts_with(&format!("Pass 1 on {CONVERSATION}.\n"))
    );
    assert_eq!(stored(root.path(), &id)["conversation"], CONVERSATION);

    let refused = run_args(root.path(), &["submit", &id, "--id", "jp-c1", "-m", "Go."]);
    assert_eq!(
        refused.reason,
        Some(format!(
            "{id} runs its passes in {CONVERSATION}; leave out `--id` to continue there."
        ))
    );
}

#[test]
fn the_cfg_option_and_flags_go_with_the_turn() {
    let root = workspace();
    let id = commented(root.path());
    let options = Map::from_iter([("cfg".to_owned(), json!(["skill/review"]))]);

    let messages = session(vec![
        Step::Send(HostToPlugin::Init(Box::new(init_message(
            root.path(),
            &["submit", &id, "--cfg", "assistant.model.id=opus"],
            OutputFormat::Text,
            options,
        )))),
        created(),
        completed(QueryOutcome::Finished),
    ]);

    assert_eq!(query_of(&messages).cfg, [
        r#"plugins.command.review.options.assistant:="assistant""#,
        "plugins.command.review.options.max_chars:=600",
        "skill/review",
        "assistant.model.id=opus"
    ]);
}

/// `jp --cfg plugins.command.review.options.dir=DIR` sets the store the way the
/// option does, and reaches the plugin only as its options, while a new
/// conversation reads the config files instead.
#[test]
fn a_store_the_options_name_goes_with_each_pass() {
    let root = workspace();
    let options = Map::from_iter([("dir".to_owned(), json!("elsewhere"))]);
    let run_with =
        |args: &[&str]| init_message(root.path(), args, OutputFormat::Text, options.clone());
    let started = run_init(run_with(&["start", "docs/a.md"]));
    let id = started.text.split_once(':').unwrap().0.to_owned();

    let messages = session(vec![
        Step::Send(HostToPlugin::Init(Box::new(run_with(&[
            "submit", &id, "-m", "Go.",
        ])))),
        created(),
        completed(QueryOutcome::Finished),
    ]);

    assert_eq!(query_of(&messages).cfg, [
        r#"plugins.command.review.options.assistant:="assistant""#,
        "plugins.command.review.options.max_chars:=600",
        r#"plugins.command.review.options.dir:="elsewhere""#,
    ]);
}

/// A conversation keeps the configuration it was created with, so a limit
/// lowered after the first pass reaches the next one's tools only by going with
/// it.
#[test]
fn a_later_pass_forwards_the_options_as_they_are_now() {
    let root = workspace();
    let id = commented(root.path());
    session(vec![
        init_step(root.path(), &["submit", &id]),
        created(),
        completed(QueryOutcome::Finished),
    ]);
    let lowered = Map::from_iter([
        ("assistant".to_owned(), json!("jp")),
        ("max_chars".to_owned(), json!(20)),
    ]);

    let messages = session(vec![
        Step::Send(HostToPlugin::Init(Box::new(init_message(
            root.path(),
            &["submit", &id, "-m", "Go on."],
            OutputFormat::Text,
            lowered,
        )))),
        completed(QueryOutcome::Finished),
    ]);

    let query = query_of(&messages);
    assert_eq!(query.conversation, CONVERSATION);
    assert_eq!(query.cfg, [
        r#"plugins.command.review.options.assistant:="jp""#,
        "plugins.command.review.options.max_chars:=20",
    ]);
}

#[test]
fn a_pass_with_nothing_to_send_is_refused() {
    let root = workspace();
    let id = start(root.path());

    let ran = run_args(root.path(), &["submit", &id]);

    assert_eq!(
        ran.reason,
        Some(format!(
            "{id} has nothing to send: no draft, and no comment waiting on the assistant. Pass \
             `-m` to send a message on its own."
        ))
    );
    assert_eq!(stored(root.path(), &id).get("passes"), None);
}

/// A review runs one pass at a time: another process holding `.pass` is running
/// one.
#[test]
fn a_running_pass_refuses_another() {
    let root = workspace();
    let id = commented(root.path());
    let held = File::create(review_file(root.path(), &id).with_file_name(".pass")).unwrap();
    held.lock().unwrap();

    let ran = run_args(root.path(), &["submit", &id]);

    assert_eq!(
        ran.reason,
        Some(format!(
            "A pass of {id} is running; wait for it to end, or stop it where it runs."
        ))
    );
    assert_eq!(stored(root.path(), &id).get("passes"), None);
}

/// A pass without an outcome whose `.pass` lock nobody holds was left by a
/// process that is gone, which the next command to read the review records.
#[test]
fn a_pass_left_by_a_gone_process_is_recorded_as_interrupted() {
    let root = workspace();
    let id = commented(root.path());
    let review_id: ReviewId = id.parse().unwrap();
    Store::new(root.path().join(".user/review"))
        .update(review_id, |review, _| {
            review
                .submit(NewPass {
                    author: "Jane".to_owned(),
                    created_at: now(),
                    version: 1,
                    only: vec![],
                    message: None,
                    conversation: None,
                })
                .map_err(|refusal| refusal.message(review_id))
        })
        .unwrap();

    let shown = run_args(root.path(), &["show", &id]).text;

    assert!(shown.contains("- **Passes**: 1 interrupted\n"), "{shown}");
}

/// The host reports the last thing the assistant said in the turn, which the
/// pass keeps and `show` prints under it.
#[test]
fn a_pass_keeps_the_closing_message_and_show_prints_it() {
    let root = workspace();
    let id = commented(root.path());

    let messages = session(vec![
        init_step(root.path(), &["submit", &id]),
        created(),
        answered(
            QueryOutcome::Finished,
            Some("Renamed the first line.\n\nNothing else changed.\n"),
        ),
    ]);

    assert_eq!(ran(&messages).code, 0);
    assert_eq!(
        stored(root.path(), &id)["passes"][0]["ended"]["answer"],
        "Renamed the first line.\n\nNothing else changed.\n"
    );
    let shown = run_args(root.path(), &["show", &id]).text;
    assert!(
        shown.contains(
            "- **Passes**: 1 completed\n\n## Pass 1 (completed)\n\nRenamed the first \
             line.\n\nNothing else changed.\n\n## "
        ),
        "{shown}"
    );
}

/// A host older than protocol 16 sends no closing message, and the pass still
/// records how it ended.
#[test]
fn a_pass_without_a_closing_message_shows_none() {
    let root = workspace();
    let id = commented(root.path());

    session(vec![
        init_step(root.path(), &["submit", &id]),
        created(),
        completed(QueryOutcome::Finished),
    ]);

    let ended = &stored(root.path(), &id)["passes"][0]["ended"];
    assert_eq!(
        (ended["outcome"].clone(), ended["answer"].clone()),
        (json!("completed"), Value::Null)
    );
    let shown = run_args(root.path(), &["show", &id]).text;
    assert!(!shown.contains("## Pass 1"), "{shown}");
}

#[test]
fn submit_prints_one_record_under_a_json_format() {
    let root = workspace();
    let id = commented(root.path());
    let (path, review) = (root.path().to_owned(), id.clone());

    let messages = session(vec![
        Step::Send(HostToPlugin::Init(Box::new(init_message(
            root.path(),
            &["submit", &id],
            OutputFormat::Json,
            Map::new(),
        )))),
        created(),
        Step::Act(Box::new(move || {
            tool_call(
                &path,
                "reply",
                json!({ "comment": format!("{review}#1"), "verdict": "defer", "body": "T-02wt0kx." }),
            );
            tool_call(
                &path,
                "comment",
                json!({ "review": review, "body": "The title is vague." }),
            );
        })),
        completed(QueryOutcome::Finished),
    ]);

    let ran = ran(&messages);
    assert_eq!((ran.code, ran.reason), (0, None));
    let record: Value = serde_json::from_str(&ran.text).unwrap();
    assert_eq!(record["review"], id);
    assert_eq!(record["number"], 1);
    assert_eq!(record["conversation"], CONVERSATION);
    assert_eq!(record["ended"]["outcome"], "completed");
    assert_eq!(record["replies"][0]["comment"], format!("{id}#1"));
    assert_eq!(record["replies"][0]["verdict"], "defer");
    assert_eq!(record["opened"], json!([format!("{id}#2")]));
    assert_eq!(record["unanswered"], json!([]));
    assert_eq!(record["changed"], json!([]));
}

#[test]
fn init_and_tools_init_write_into_the_workspace() {
    let root = workspace();

    let config = run_args(root.path(), &["init", ".jp/config/skill"]);
    let tools = run_args(root.path(), &["tools", "init", ".jp/mcp/tools/review"]);

    assert_eq!((config.code, tools.code), (0, 0));
    assert!(root.path().join(".jp/config/skill/review.toml").is_file());
    assert!(
        root.path()
            .join(".jp/mcp/tools/review/reply.toml")
            .is_file()
    );
    assert!(
        root.path()
            .join(".jp/mcp/tools/review/comment.toml")
            .is_file()
    );
}

/// Writing files into the workspace needs neither the options nor the store, so
/// a workspace with neither still gets them.
#[test]
fn init_works_without_a_store() {
    let root = workspace();
    let mut init = init_message(
        root.path(),
        &["init", "config"],
        OutputFormat::Text,
        Map::new(),
    );
    init.paths.user_workspace = None;

    let ran = run_init(init);

    assert_eq!((ran.code, ran.reason), (0, None));
    assert!(root.path().join("config/review.toml").is_file());
}

/// A comment's thread reads the assistant's verdict, and what it opened, the
/// way the reviewer reads the rest.
#[test]
fn show_reads_the_assistants_replies_and_comments() {
    let root = workspace();
    let id = commented(root.path());
    let review_id: ReviewId = id.parse().unwrap();
    Store::new(root.path().join(".user/review"))
        .update(review_id, |review, _| {
            review
                .add_comment(NewComment {
                    role: Role::Assistant,
                    author: "jp".to_owned(),
                    created_at: now(),
                    content: Content {
                        anchor: Anchor {
                            version: 1,
                            target: Target::Review,
                        },
                        body: "The title is vague.".to_owned(),
                        suggestion: None,
                    },
                })
                .map_err(|refusal| refusal.message(review_id))
        })
        .unwrap();

    let shown = run_args(root.path(), &["show", &id]).text;

    assert!(
        shown.contains(&format!("## {id}#2 (open)\n\nOn the review, by jp, ")),
        "{shown}"
    );
}

/// `jp review serve`, as the host runs it: it says where it listens, answers
/// over HTTP, the names `--allowed-host` gives included, and exits when the
/// host shuts it down.
#[test]
fn serve_answers_until_the_host_shuts_it_down() {
    let root = workspace();
    let init = HostToPlugin::Init(Box::new(init_message(
        root.path(),
        &["serve", "--port", "0", "--allowed-host", "Box.local"],
        OutputFormat::Text,
        Map::new(),
    )));
    let (plugin_in, mut to_plugin) = io::pipe().unwrap();
    let (from_plugin, plugin_out) = io::pipe().unwrap();
    writeln!(to_plugin, "{}", serde_json::to_string(&init).unwrap()).unwrap();
    let plugin = thread::spawn(move || run(BufReader::new(plugin_in), plugin_out));

    let mut from_plugin = BufReader::new(from_plugin);
    let mut next = || -> PluginToHost {
        let mut line = String::new();
        from_plugin.read_line(&mut line).unwrap();
        serde_json::from_str(&line).unwrap()
    };
    assert!(matches!(next(), PluginToHost::Ready(_)));
    let PluginToHost::Print(serving) = next() else {
        panic!("the plugin says where it listens");
    };
    let address: SocketAddr = serving
        .text
        .trim()
        .strip_prefix("Serving reviews at http://")
        .unwrap()
        .parse()
        .unwrap();

    let listed = http(address, "GET", "/api/reviews", &[], "");
    let allowed = http(address, "GET", "/api/reviews", &[("Host", "box.local")], "");
    let unknown = http(
        address,
        "GET",
        "/api/reviews",
        &[("Host", "evil.example")],
        "",
    );
    writeln!(
        to_plugin,
        "{}",
        serde_json::to_string(&HostToPlugin::Shutdown).unwrap()
    )
    .unwrap();
    let exit = loop {
        if let PluginToHost::Exit(exit) = next() {
            break exit;
        }
    };

    // The workspace is named after its root directory, a temporary one here.
    let empty = json!({ "workspace": root.path().file_name().unwrap(), "reviews": [] });
    assert_eq!(listed, (200, empty.clone()));
    assert_eq!(allowed, (200, empty));
    assert_eq!(unknown.0, 403);
    assert_eq!((exit.code, exit.reason), (0, None));
    assert_eq!(plugin.join().unwrap(), Ok(()));
}

/// `jp review open` and `start --open` find the server through the store, and
/// only while it runs; a second server for the same reviews is refused.
#[test]
fn serve_opens_the_list_and_tells_open_where_it_listens() {
    let root = workspace();
    let id = start(root.path());
    let none = run_opening(root.path(), &["open"], &Opened::default());
    assert_eq!(
        (none.code, none.reason.as_deref()),
        (
            1,
            Some(
                "No `jp review serve` runs for this workspace's reviews. Start one with `jp \
                 review serve --open`."
            )
        )
    );

    let init = HostToPlugin::Init(Box::new(init_message(
        root.path(),
        &["serve", "--port", "0", "--open"],
        OutputFormat::Text,
        Map::new(),
    )));
    let (plugin_in, mut to_plugin) = io::pipe().unwrap();
    let (from_plugin, plugin_out) = io::pipe().unwrap();
    writeln!(to_plugin, "{}", serde_json::to_string(&init).unwrap()).unwrap();
    let served = Arc::new(Opened::default());
    let serving = Arc::clone(&served);
    let plugin = thread::spawn(move || run_with(BufReader::new(plugin_in), plugin_out, &*serving));
    let mut from_plugin = BufReader::new(from_plugin);
    let mut next = || -> PluginToHost {
        let mut line = String::new();
        from_plugin.read_line(&mut line).unwrap();
        serde_json::from_str(&line).unwrap()
    };
    assert!(matches!(next(), PluginToHost::Ready(_)));
    let PluginToHost::Print(announced) = next() else {
        panic!("the plugin says where it listens");
    };
    let base = announced
        .text
        .trim()
        .strip_prefix("Serving reviews at ")
        .unwrap()
        .to_owned();

    let opened = Opened::default();
    let review = run_opening(root.path(), &["open", &id], &opened);
    let started = run_opening(root.path(), &["start", "docs/a.md", "--open"], &opened);
    let missing = run_opening(root.path(), &["open", "R-0000000"], &opened);
    let second = run_args(root.path(), &["serve", "--port", "0"]);

    writeln!(
        to_plugin,
        "{}",
        serde_json::to_string(&HostToPlugin::Shutdown).unwrap()
    )
    .unwrap();
    while !matches!(next(), PluginToHost::Exit(_)) {}
    assert_eq!(plugin.join().unwrap(), Ok(()));
    let after = run_opening(root.path(), &["start", "docs/a.md", "--open"], &opened);

    let new = started.text.split_once(':').unwrap().0;
    assert_eq!(served.urls(), [format!("{base}/")]);
    assert_eq!(review.text, format!("{base}/reviews/{id}\n"));
    assert_eq!(
        started.chrome,
        format!("Opened {base}/reviews/{new}.\n"),
        "the review is started, then opened"
    );
    assert_eq!(opened.urls(), [
        format!("{base}/reviews/{id}"),
        format!("{base}/reviews/{new}")
    ]);
    assert_eq!(missing.reason.as_deref(), Some("No review R-0000000."));
    assert_eq!(
        (second.code, second.reason),
        (
            1,
            Some(format!(
                "`jp review serve` serves these reviews already, at {base}. Open it with `jp \
                 review open`."
            ))
        )
    );
    assert_eq!(after.code, 0, "the review is started without a server");
    assert!(
        after
            .chrome
            .starts_with("Warning: No `jp review serve` runs for this workspace's reviews."),
        "{}",
        after.chrome
    );
}

/// The server acquires the store lock before binding its listener and writing
/// its address file.
#[test]
fn open_while_the_server_starts_asks_to_try_again() {
    let root = workspace();
    let store = Store::new(root.path().join(".user/review"));
    let Ok(Serving::Claimed(_lock)) = store.claim_server() else {
        panic!("nothing else serves the reviews");
    };
    let opened = Opened::default();

    let ran = run_opening(root.path(), &["open"], &opened);

    assert_eq!(
        (ran.code, ran.reason.as_deref()),
        (
            1,
            Some("`jp review serve` is starting, and hasn't said where it listens yet; try again.")
        )
    );
    assert_eq!(opened.urls(), Vec::<String>::new());
}

#[test]
fn open_names_the_review_as_json() {
    let root = workspace();
    let id = start(root.path());
    let store = Store::new(root.path().join(".user/review"));
    let Ok(Serving::Claimed(_lock)) = store.claim_server() else {
        panic!("nothing else serves the reviews");
    };
    store.serving_at("http://127.0.0.1:3001").unwrap();
    let opened = Opened::default();

    let ran = run_opening_as(root.path(), &["open", &id], OutputFormat::Json, &opened);

    assert_eq!(
        serde_json::from_str::<Value>(&ran.text).unwrap(),
        json!({ "url": format!("http://127.0.0.1:3001/reviews/{id}") })
    );
    assert_eq!(opened.urls(), [format!(
        "http://127.0.0.1:3001/reviews/{id}"
    )]);
}

#[test]
fn serve_fails_on_a_port_another_server_holds() {
    let root = workspace();
    let held = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = held.local_addr().unwrap().port().to_string();

    let ran = run_args(root.path(), &["serve", "--port", &port]);

    assert_eq!(ran.code, 1);
    let reason = ran.reason.unwrap();
    assert!(
        reason.starts_with(&format!("Failed to listen on 127.0.0.1:{port}: ")),
        "{reason}"
    );
}

/// Without `--bind` and `--port`, the server listens where the options say; a
/// port another server holds shows which one it tried.
#[test]
fn serve_listens_where_the_options_say_unless_a_flag_says_otherwise() {
    let root = workspace();
    let held = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = held.local_addr().unwrap().port();
    let options = |value: Value| match value {
        Value::Object(map) => map,
        _ => unreachable!(),
    };
    let configured = options(json!({ "bind": "127.0.0.1", "port": port }));
    let elsewhere = options(json!({ "bind": "127.0.0.1", "port": 1 }));

    let from_options = run_init(init_message(
        root.path(),
        &["serve"],
        OutputFormat::Text,
        configured,
    ));
    let from_flag = run_init(init_message(
        root.path(),
        &["serve", "--port", &port.to_string()],
        OutputFormat::Text,
        elsewhere,
    ));

    let tried = format!("Failed to listen on 127.0.0.1:{port}: ");
    assert_eq!(from_options.code, 1);
    assert!(
        from_options.reason.as_deref().unwrap().starts_with(&tried),
        "{:?}",
        from_options.reason
    );
    assert_eq!(from_flag.code, 1);
    assert!(
        from_flag.reason.as_deref().unwrap().starts_with(&tried),
        "{:?}",
        from_flag.reason
    );
}
