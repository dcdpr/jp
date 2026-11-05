use std::env;

use camino_tempfile::Utf8TempDir;
use indoc::indoc;
use jp_plugin::message::{ComposeResponse, OutputFormat, PathsInfo, WorkspaceInfo};
use serde_json::{Map, json};

use super::*;

/// The root of JP's own repository, for the tests that read its files.
///
/// Read when the test runs rather than with `env!`: a target directory shared
/// between worktrees can run a binary another worktree compiled, whose baked-in
/// path names that worktree.
pub(crate) fn jp_root() -> Utf8PathBuf {
    let manifest = env::var("CARGO_MANIFEST_DIR")
        .expect("CARGO_MANIFEST_DIR: run tests via `cargo test` or `cargo nextest`");

    Utf8PathBuf::from(manifest).join("../../../..")
}

/// A workspace whose fragments directory, `changelog.d`, exists.
fn workspace() -> Utf8TempDir {
    let root = Utf8TempDir::new().unwrap();
    fs::create_dir(root.path().join("changelog.d")).unwrap();

    root
}

/// The `init` running `args` in the workspace at `root`, as Jane Doe.
fn init_message(root: &Utf8Path, args: &[&str]) -> InitMessage {
    InitMessage {
        version: jp_plugin::PROTOCOL_VERSION,
        workspace: WorkspaceInfo {
            root: root.to_path_buf(),
            storage: root.join(".jp"),
            id: "test".to_owned(),
        },
        paths: PathsInfo::default(),
        config: json!({ "user": { "name": "Jane Doe" } }),
        options: Map::new(),
        args: args.iter().map(|arg| (*arg).to_owned()).collect(),
        log_level: 0,
        output_format: OutputFormat::default(),
        tool: None,
    }
}

/// Run a session the way the host does: `message`, then a scripted reply to
/// each request the plugin makes.
fn session(message: &HostToPlugin, replies: &[HostToPlugin]) -> Vec<PluginToHost> {
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

/// Run `jp release` with `args` in the workspace at `root`.
fn command(root: &Utf8Path, args: &[&str], replies: &[HostToPlugin]) -> Vec<PluginToHost> {
    session(
        &HostToPlugin::Init(Box::new(init_message(root, args))),
        replies,
    )
}

/// The composer's answer: `text`, or none, as when there is no terminal to ask
/// on.
fn composed(text: Option<&str>) -> HostToPlugin {
    HostToPlugin::Composed(ComposeResponse {
        id: None,
        text: text.map(str::to_owned),
        values: vec![],
    })
}

/// Everything the session printed.
fn printed(messages: &[PluginToHost]) -> String {
    messages
        .iter()
        .filter_map(|message| match message {
            PluginToHost::Print(print) => Some(print.text.as_str()),
            _ => None,
        })
        .collect()
}

/// How the session ended.
fn exit(messages: &[PluginToHost]) -> &ExitMessage {
    match messages.last() {
        Some(PluginToHost::Exit(exit)) => exit,
        other => panic!("expected an exit, got {other:?}"),
    }
}

/// What the session asked the composer for.
fn requests(messages: &[PluginToHost]) -> Vec<&ComposeRequest> {
    messages
        .iter()
        .filter_map(|message| match message {
            PluginToHost::Compose(request) => Some(request),
            _ => None,
        })
        .collect()
}

fn read(root: &Utf8TempDir, path: &str) -> String {
    fs::read_to_string(root.path().join(path)).unwrap()
}

/// Run `jp release` with `args` in the workspace at `root`, as Jane Doe, with
/// `format_on_write` set to `format_on_write`.
fn formatting(root: &Utf8Path, args: &[&str], format_on_write: &Value) -> Vec<PluginToHost> {
    let mut message = init_message(root, args);
    message.config = json!({
        "user": { "name": "Jane Doe" },
        "plugins": { "command": { "release": { "options": {
            "format_on_write": format_on_write
        } } } }
    });

    session(&HostToPlugin::Init(Box::new(message)), &[])
}

#[test]
fn the_command_line_is_well_formed() {
    Args::command().debug_assert();
}

#[test]
fn describe_names_the_plugin_and_its_command() {
    let messages = session(&HostToPlugin::Describe, &[]);

    let [PluginToHost::Describe(describe)] = messages.as_slice() else {
        panic!("expected one describe, got {messages:?}");
    };
    assert_eq!(describe.name, "release");
    assert_eq!(describe.manifest.command, ["release"]);
    assert_eq!(describe.manifest.protocol, REQUIRED_PROTOCOL);
}

#[test]
fn init_creates_the_directory_its_highlights_and_the_readme() {
    let root = Utf8TempDir::new().unwrap();

    let messages = command(root.path(), &["init"], &[]);

    assert_eq!(
        printed(&messages),
        "Created changelog.d/\nCreated changelog.d/highlights/\nWrote changelog.d/README.md\n"
    );
    assert_eq!(exit(&messages).code, 0);
    assert_eq!(read(&root, "changelog.d/README.md"), README);
}

/// Running it again adds what is missing, and leaves what is there alone.
#[test]
fn init_leaves_what_is_there() {
    let root = workspace();
    fs::write(root.path().join("changelog.d/README.md"), "Ours.\n").unwrap();

    let messages = command(root.path(), &["init"], &[]);

    assert_eq!(printed(&messages), "Created changelog.d/highlights/\n");
    assert_eq!(read(&root, "changelog.d/README.md"), "Ours.\n");

    let messages = command(root.path(), &["init"], &[]);

    assert_eq!(
        printed(&messages),
        "changelog.d/ already has its `highlights/` and its README.\n"
    );
}

#[test]
fn init_lays_the_readme_out_in_the_workspaces_style() {
    let root = Utf8TempDir::new().unwrap();
    // No width to wrap at, so each sentence takes one line.
    fs::write(root.path().join(".comfort.toml"), "max_width = 0\n").unwrap();

    formatting(root.path(), &["init"], &json!(true));

    let readme = read(&root, "changelog.d/README.md");
    assert!(
        readme.contains(
            "\nEach pull request that changes something users notice adds a fragment to this \
             directory: a short entry that tells them what changed.\n"
        ),
        "{readme}"
    );
}

/// The README a new project starts with is JP's own.
#[test]
fn the_readme_init_writes_is_the_one_jp_uses() {
    let ours = fs::read_to_string(jp_root().join("changelog.d/README.md")).unwrap();

    assert_eq!(README, ours);
}

/// The README's table names each type with what it is used for, as the type
/// picker does.
#[test]
fn the_readme_describes_every_type() {
    for type_ in Type::ALL {
        let row = README
            .lines()
            .find(|line| line.starts_with(&format!("| `{}`", type_.id())))
            .unwrap_or_else(|| panic!("the README has no row for `{type_}`"));

        assert!(row.contains(type_.description()), "{row}");
    }
}

/// Whatever `init` writes, `check` passes.
#[test]
fn a_directory_init_set_up_passes_check() {
    let root = Utf8TempDir::new().unwrap();
    command(root.path(), &["init"], &[]);

    let messages = command(root.path(), &["check"], &[]);

    assert_eq!(
        printed(&messages),
        "Checked 0 fragments and 0 highlights: no problems.\n"
    );
    assert_eq!(exit(&messages).code, 0);
}

#[test]
fn add_writes_a_fragment_from_its_flags() {
    let root = workspace();

    let messages = command(
        root.path(),
        &["add", "--type", "fixed", "--body", "Crashes are fixed."],
        &[],
    );

    assert_eq!(
        printed(&messages),
        "Wrote changelog.d/crashes-are-fixed.md\n"
    );
    assert_eq!(exit(&messages).code, 0);
    assert_eq!(read(&root, "changelog.d/crashes-are-fixed.md"), indoc! {"
        ---
        type: fixed
        ---
        Crashes are fixed.
    "});
}

#[test]
fn add_lays_the_fragment_out_in_the_workspaces_style() {
    let root = workspace();
    fs::write(root.path().join(".comfort.toml"), "max_width = 80\n").unwrap();

    let messages = formatting(
        root.path(),
        &[
            "add",
            "--type",
            "fixed",
            "--slug",
            "retry",
            "--body",
            "Retries wait. They give up after three attempts.",
        ],
        &json!(true),
    );

    assert_eq!(printed(&messages), "Wrote changelog.d/retry.md\n");
    assert_eq!(read(&root, "changelog.d/retry.md"), indoc! {"
        ---
        type: fixed
        ---
        Retries wait.
        They give up after three attempts.
    "});
}

/// A setting the plugin can't read fails every command, not only the ones that
/// write.
#[test]
fn a_format_on_write_that_is_not_true_or_false_fails_the_command() {
    let root = workspace();

    let messages = formatting(root.path(), &["check"], &json!("yes"));

    let exit = exit(&messages);
    assert_eq!(exit.code, 1);
    assert_eq!(
        exit.reason.as_deref(),
        Some("`plugins.command.release.options.format_on_write` must be `true` or `false`.")
    );
}

#[test]
fn add_asks_for_the_type_and_the_entry() {
    let root = workspace();

    let messages = command(root.path(), &["add", "--slug", "crashes"], &[
        composed(Some("fixed")),
        composed(Some("Crashes are fixed.")),
    ]);

    let requests = requests(&messages);
    let ComposeMode::Select { options, default } = &requests[0].mode else {
        panic!("expected a picker, got {:?}", requests[0]);
    };
    let values: Vec<&str> = options.iter().map(|option| option.value.as_str()).collect();
    assert_eq!(values, [
        "added",
        "changed",
        "deprecated",
        "removed",
        "fixed",
        "security",
        "performance"
    ]);
    assert_eq!(
        options[0].label,
        "added        A new capability the user can now reach."
    );
    assert_eq!(*default, None);
    assert_eq!(requests[1].message, "New `fixed` fragment");
    assert_eq!(requests[1].mode, ComposeMode::Buffer { initial_text: None });

    assert_eq!(printed(&messages), "Wrote changelog.d/crashes.md\n");
    assert_eq!(read(&root, "changelog.d/crashes.md"), indoc! {"
        ---
        type: fixed
        ---
        Crashes are fixed.
    "});
}

/// An entry the format refuses opens the composer again on what was written,
/// saying why, rather than throwing it away.
#[test]
fn add_offers_an_entry_the_format_refuses_back_to_fix() {
    let root = workspace();
    let broken = "Entry.\n\n```text\nBREAKING CHANGE: in an example\n```";

    let messages = command(root.path(), &["add", "--type", "changed"], &[
        composed(Some(broken)),
        composed(Some("Entry, now without the example.")),
    ]);

    let requests = requests(&messages);
    assert_eq!(requests[1].mode, ComposeMode::Buffer {
        initial_text: Some(broken.to_owned())
    });
    assert_eq!(
        requests[1].help.as_deref(),
        Some(
            "Not written: line 7 starts with `BREAKING CHANGE:` inside a code block; to show the \
             token, indent the line or put the token in inline code."
        )
    );
    assert_eq!(
        printed(&messages),
        "Wrote changelog.d/entry-now-without-the-example.md\n"
    );
}

#[test]
fn add_without_a_terminal_says_what_to_pass() {
    let root = workspace();

    let messages = command(root.path(), &["add", "--type", "fixed"], &[composed(None)]);

    let exit = exit(&messages);
    assert_eq!(exit.code, 1);
    assert_eq!(
        exit.reason.as_deref(),
        Some("Nothing composed; run it again with what it asked for given as flags.")
    );
}

/// The directory and a named slug are checked before the user is asked for
/// anything, so a mistake there doesn't cost what they typed.
#[test]
fn add_checks_the_directory_and_the_slug_before_asking() {
    let root = Utf8TempDir::new().unwrap();

    let messages = command(root.path(), &["add"], &[]);

    assert!(requests(&messages).is_empty());
    assert_eq!(
        exit(&messages).reason.as_deref(),
        Some("There is no fragments directory at changelog.d; create it with `jp release init`.")
    );

    let root = workspace();
    let messages = command(root.path(), &["add", "--slug", "README"], &[]);

    assert!(requests(&messages).is_empty());
    assert_eq!(
        exit(&messages).reason.as_deref(),
        Some(
            "`README` is reserved for the fragments directory's documentation, `README.md` at its \
             top level, in any letter case; choose another slug."
        )
    );
}

#[test]
fn add_writes_a_highlight_from_its_flags() {
    let root = workspace();

    let messages = command(
        root.path(),
        &[
            "add",
            "--highlight",
            "--title",
            "Tool calls fan out",
            "--author",
            "@JeanMertz",
            "--body",
            "The assistant calls one tool on many inputs at once.",
        ],
        &[],
    );

    assert_eq!(
        printed(&messages),
        "Wrote changelog.d/highlights/tool-calls-fan-out.md\n"
    );
    assert_eq!(
        read(&root, "changelog.d/highlights/tool-calls-fan-out.md"),
        indoc! {r#"
            ---
            title: "Tool calls fan out"
            authors: ["@JeanMertz"]
            ---
            The assistant calls one tool on many inputs at once.
        "#}
    );
}

/// The credits default to the user's own name, which they can replace with
/// handles, several separated by commas.
#[test]
fn add_asks_for_a_highlights_title_credits_and_prose() {
    let root = workspace();

    let messages = command(root.path(), &["add", "--highlight"], &[
        composed(Some("Tool calls fan out")),
        composed(Some("@JeanMertz, Jane Doe")),
        composed(Some("The assistant calls one tool on many inputs at once.")),
    ]);

    let requests = requests(&messages);
    assert_eq!(requests[0].mode, ComposeMode::Line { default: None });
    assert_eq!(requests[1].mode, ComposeMode::Line {
        default: Some("Jane Doe".to_owned())
    });
    assert_eq!(requests[2].message, "Highlight: Tool calls fan out");
    assert_eq!(
        read(&root, "changelog.d/highlights/tool-calls-fan-out.md"),
        indoc! {r#"
            ---
            title: "Tool calls fan out"
            authors: ["@JeanMertz", "Jane Doe"]
            ---
            The assistant calls one tool on many inputs at once.
        "#}
    );
}

#[test]
fn a_highlight_needs_a_title_and_someone_to_credit() {
    let root = workspace();

    let messages = command(root.path(), &["add", "--highlight"], &[composed(Some(" "))]);

    assert_eq!(
        exit(&messages).reason.as_deref(),
        Some("A highlight needs a title.")
    );

    let messages = command(
        root.path(),
        &["add", "--highlight", "--title", "Fan out", "--author", " "],
        &[],
    );

    assert_eq!(
        exit(&messages).reason.as_deref(),
        Some("A highlight credits at least one author.")
    );
}

/// A highlight has no type, and a fragment no title or authors.
#[test]
fn the_flags_of_one_format_are_refused_for_the_other() {
    let root = workspace();

    for args in [
        &["add", "--highlight", "--type", "fixed"][..],
        &["add", "--title", "Fan out"],
        &["add", "--author", "@JeanMertz"],
    ] {
        let messages = command(root.path(), args, &[]);

        assert_eq!(exit(&messages).code, 1, "{args:?}");
    }
}

#[test]
fn an_unknown_type_is_refused_with_the_seven() {
    let root = workspace();

    let messages = command(root.path(), &["add", "--type", "feat"], &[]);

    let reason = exit(&messages).reason.clone().unwrap_or_default();
    assert!(
        reason.contains(
            "`feat` is not a changelog type; use one of: added, changed, deprecated, removed, \
             fixed, security, performance"
        ),
        "{reason}"
    );
}

#[test]
fn check_names_each_problem_from_the_workspace_root() {
    let root = workspace();
    fs::write(root.path().join("changelog.d/notes.txt"), "Notes.\n").unwrap();

    let messages = command(root.path(), &["check"], &[]);

    assert_eq!(printed(&messages), indoc! {"
        changelog.d/notes.txt: not a fragment, which is named `<slug>.md`; rename it, or move it out of the fragments directory

        Checked 0 fragments and 0 highlights: 1 problem.
    "});
    // A check that finds problems reports them by its exit code, which is not
    // a failure of the run.
    let exit = exit(&messages);
    assert_eq!(exit.code, 1);
    assert!(exit.expected);
}

#[test]
fn check_without_a_fragments_directory_says_how_to_make_one() {
    let root = Utf8TempDir::new().unwrap();

    let messages = command(root.path(), &["check"], &[]);

    assert_eq!(
        exit(&messages).reason.as_deref(),
        Some("There is no fragments directory at changelog.d; create it with `jp release init`.")
    );
}

#[test]
fn the_fragments_directory_can_be_configured() {
    let root = Utf8TempDir::new().unwrap();
    let mut message = init_message(root.path(), &["init"]);
    message.options = Map::from_iter([("fragments_dir".to_owned(), json!("docs/changes"))]);

    let messages = session(&HostToPlugin::Init(Box::new(message)), &[]);

    assert_eq!(
        printed(&messages),
        "Created docs/changes/\nCreated docs/changes/highlights/\nWrote docs/changes/README.md\n"
    );
}

#[test]
fn tools_init_writes_the_definition_once() {
    let root = Utf8TempDir::new().unwrap();
    let dir = root.path().join(".jp/mcp/tools/release");
    let args = ["tools", "init", ".jp/mcp/tools/release"];

    let messages = command(root.path(), &args, &[]);

    assert_eq!(
        printed(&messages),
        format!(
            "Wrote {}\nLoad the directory from the workspace configuration for the tools to \
             appear.\n",
            dir.join("add.toml")
        )
    );
    assert_eq!(
        fs::read_to_string(dir.join("add.toml")).unwrap(),
        include_str!("../tools/add.toml")
    );

    let messages = command(root.path(), &args, &[]);

    assert_eq!(exit(&messages).code, 1);
}
