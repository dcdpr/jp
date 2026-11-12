use std::fs;

use camino_tempfile::Utf8TempDir;
use jp_plugin::message::{OutputFormat, PathsInfo, WorkspaceInfo};
use jp_tool::FsRule;
use serde_json::json;

use super::*;

/// A workspace whose fragments directory, `changelog.d`, exists.
fn workspace() -> Utf8TempDir {
    let root = Utf8TempDir::new().unwrap();
    fs::create_dir(root.path().join("changelog.d")).unwrap();

    root
}

/// The workspace a call runs in, as the host describes it in `init`.
fn init(root: &Utf8TempDir) -> InitMessage {
    InitMessage {
        version: jp_plugin::PROTOCOL_VERSION,
        workspace: WorkspaceInfo {
            root: root.path().to_path_buf(),
            storage: root.path().join(".jp"),
            id: "test".to_owned(),
        },
        paths: PathsInfo::default(),
        config: json!({}),
        options: Map::new(),
        args: vec![],
        log_level: 0,
        output_format: OutputFormat::Text,
        tool: None,
    }
}

/// A call to `name` asking for `action`, with `arguments`.
fn call(action: ToolAction, name: &str, arguments: Value) -> ToolCall {
    let Value::Object(arguments) = arguments else {
        panic!("arguments are an object");
    };

    ToolCall {
        action,
        name: name.to_owned(),
        arguments,
        answers: Map::new(),
        options: Map::new(),
        access: None,
        conversation: "test".to_owned(),
    }
}

fn add(root: &Utf8TempDir, arguments: Value) -> Outcome {
    run(
        &init(root),
        &call(ToolAction::Run, "add", arguments),
        &Options::default(),
    )
}

fn preview_add(root: &Utf8TempDir, arguments: Value) -> Outcome {
    run(
        &init(root),
        &call(ToolAction::FormatArguments, "add", arguments),
        &Options::default(),
    )
}

fn content(outcome: Outcome) -> String {
    match outcome {
        Outcome::Success { content } => content,
        other => panic!("expected success, got: {other:?}"),
    }
}

fn error_message(outcome: Outcome) -> String {
    match outcome {
        Outcome::Error { message, .. } => message,
        other => panic!("expected an error, got: {other:?}"),
    }
}

/// A preview as a terminal shows it, with the ANSI styling removed.
fn strip_ansi(rendered: &str) -> String {
    String::from_utf8(strip_ansi_escapes::strip(rendered)).unwrap()
}

#[test]
fn add_writes_a_fragment() {
    let root = workspace();

    let outcome = add(
        &root,
        json!({ "type": "fixed", "body": "Crashes are fixed." }),
    );

    assert_eq!(
        content(outcome),
        "Wrote changelog.d/crashes-are-fixed.fixed.md"
    );
    assert_eq!(
        fs::read_to_string(root.path().join("changelog.d/crashes-are-fixed.fixed.md")).unwrap(),
        "Crashes are fixed.\n"
    );
}

/// The tool lays a fragment out as `jp release fixed` does: as the workspace's
/// `format_on_write` says.
#[test]
fn add_lays_the_fragment_out_as_the_workspace_formats_it() {
    let root = workspace();
    fs::write(root.path().join(".comfort.toml"), "max_width = 80\n").unwrap();
    let mut init = init(&root);
    init.config = json!({
        "plugins": { "command": { "release": { "options": { "format_on_write": true } } } }
    });

    let outcome = run(
        &init,
        &call(
            ToolAction::Run,
            "add",
            json!({
                "type": "fixed",
                "slug": "retry",
                "body": "Retries wait. They give up after three attempts."
            }),
        ),
        &Options::default(),
    );

    assert_eq!(content(outcome), "Wrote changelog.d/retry.fixed.md");
    assert_eq!(
        fs::read_to_string(root.path().join("changelog.d/retry.fixed.md")).unwrap(),
        "Retries wait.\nThey give up after three attempts.\n"
    );
}

#[test]
fn a_format_on_write_that_is_not_true_or_false_refuses_the_call() {
    let root = workspace();
    let mut init = init(&root);
    init.config = json!({
        "plugins": { "command": { "release": { "options": { "format_on_write": "yes" } } } }
    });

    let message = error_message(run(
        &init,
        &call(
            ToolAction::Run,
            "add",
            json!({ "type": "fixed", "body": "Crashes are fixed." }),
        ),
        &Options::default(),
    ));

    assert_eq!(
        message,
        "`plugins.command.release.options.format_on_write` must be `true` or `false`."
    );
}

#[test]
fn add_names_the_file_by_the_slug_it_is_given() {
    let root = workspace();

    let outcome = add(
        &root,
        json!({ "type": "fixed", "body": "Crashes are fixed.", "slug": "retry-idle-turns" }),
    );

    assert_eq!(
        content(outcome),
        "Wrote changelog.d/retry-idle-turns.fixed.md"
    );
}

/// The preview shows the file whole, where it goes, and writes nothing.
#[test]
fn a_preview_shows_the_file_and_writes_nothing() {
    let root = workspace();

    let preview = strip_ansi(&content(preview_add(
        &root,
        json!({ "type": "fixed", "body": "Crashes are fixed." }),
    )));

    for expected in [
        "changelog.d/crashes-are-fixed.fixed.md",
        "Crashes are fixed.",
    ] {
        assert!(preview.contains(expected), "{expected} in:\n{preview}");
    }
    assert!(
        !root
            .path()
            .join("changelog.d/crashes-are-fixed.fixed.md")
            .exists()
    );
}

/// A call that cannot land fails its preview, before it is put to the user.
#[test]
fn a_preview_refuses_an_entry_the_format_refuses() {
    let root = workspace();

    let message = error_message(preview_add(
        &root,
        json!({
            "type": "changed",
            "body": "Entry.\n\nBREAKING CHANGE: What broke"
        }),
    ));

    assert_eq!(
        message,
        "the upgrade section on line 3 has no body: say what changed and how to upgrade in the \
         paragraphs after its title"
    );
}

#[test]
fn an_unknown_type_is_refused_with_the_seven() {
    let root = workspace();

    let message = error_message(add(&root, json!({ "type": "feat", "body": "Entry." })));

    assert_eq!(
        message,
        "`feat` is not a changelog type; use one of: added, changed, deprecated, removed, fixed, \
         security, performance"
    );
}

#[test]
fn a_missing_argument_is_named() {
    let root = workspace();

    let message = error_message(add(&root, json!({ "type": "fixed" })));

    assert_eq!(message, "Missing argument `body`.");
}

#[test]
fn a_tool_this_plugin_does_not_serve_is_refused() {
    let root = workspace();

    let message = error_message(run(
        &init(&root),
        &call(ToolAction::Run, "list", json!({})),
        &Options::default(),
    ));

    assert_eq!(
        message,
        "`jp release` serves no tool named `list`. It serves: add."
    );
}

/// The host compiles the call's access grants and the plugin enforces them: a
/// policy that only lets the tool read refuses the write before anything lands.
#[test]
fn a_write_the_access_policy_denies_is_refused() {
    let root = workspace();
    let read_only = AccessPolicy {
        fs: vec![FsRule::new("").with_read(true)],
        ..AccessPolicy::default()
    };
    let mut call = call(
        ToolAction::Run,
        "add",
        json!({ "type": "fixed", "body": "Crashes are fixed." }),
    );
    call.access = Some(serde_json::to_value(&read_only).unwrap());

    let message = error_message(run(&init(&root), &call, &Options::default()));

    assert!(
        message.starts_with("`add` can't reach the fragments directory: "),
        "{message}"
    );
    assert!(
        !root
            .path()
            .join("changelog.d/crashes-are-fixed.fixed.md")
            .exists()
    );
}

/// A tool reaches only the workspace, so a fragments directory configured
/// outside it is refused whatever the policy says.
#[test]
fn a_fragments_directory_outside_the_workspace_is_refused() {
    let root = workspace();
    let elsewhere = Utf8TempDir::new().unwrap();
    let mut options = Options::default();
    options.notes.fragments_dir = elsewhere.path().to_path_buf();

    let message = error_message(run(
        &init(&root),
        &call(
            ToolAction::Run,
            "add",
            json!({ "type": "fixed", "body": "Entry." }),
        ),
        &options,
    ));

    assert_eq!(
        message,
        format!(
            "The fragments directory {} is outside the workspace, which is all a tool reaches.",
            elsewhere.path()
        )
    );
}

/// A fence around the file has to be longer than any run of backticks in it, or
/// a code block in the entry would end the preview early.
#[test]
fn the_longest_backtick_run_is_found() {
    assert_eq!(longest_backtick_run("no ticks"), 0);
    assert_eq!(longest_backtick_run("`a` and ```b``` and ````c````"), 4);
}
