use std::env;

use camino_tempfile::Utf8TempDir;
use indoc::indoc;
use jp_plugin::message::{ComposeResponse, PathsInfo, WorkspaceInfo};
use serde_json::{Map, json};

use super::*;
use crate::repository_testing::Fixture;

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

/// A workspace in a git repository on `main`, its fragments directory in place.
///
/// Git runs without the machine's configuration, so a signing setup or a hook
/// can't change what a test commits.
struct Repository {
    root: Utf8TempDir,

    /// Holds the empty file git reads as its global configuration.
    home: Utf8TempDir,
}

impl Repository {
    /// A repository whose `origin` is at `origin`.
    fn new(origin: &str) -> Self {
        let home = Utf8TempDir::new().unwrap();
        fs::write(home.path().join("gitconfig"), "").unwrap();
        let repository = Self {
            root: workspace(),
            home,
        };

        repository.git(&["init", "--quiet", "--initial-branch=main"]);
        repository.git(&["remote", "add", "origin", origin]);

        repository
    }

    fn path(&self) -> &Utf8Path {
        self.root.path()
    }

    /// Write `contents` to `path`, below the workspace root.
    fn write(&self, path: &str, contents: &str) {
        let path = self.path().join(path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, contents).unwrap();
    }

    /// Commit every change in the work tree with `message`.
    fn commit(&self, message: &str) {
        self.git(&["add", "--all"]);
        self.git(&["commit", "--quiet", "--message", message]);
    }

    fn git(&self, args: &[&str]) {
        let output = Process::new("git")
            .current_dir(self.path())
            .args(args)
            .env("GIT_CONFIG_GLOBAL", self.home.path().join("gitconfig"))
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_AUTHOR_NAME", "Tester")
            .env("GIT_AUTHOR_EMAIL", "tester@example.com")
            .env("GIT_COMMITTER_NAME", "Tester")
            .env("GIT_COMMITTER_EMAIL", "tester@example.com")
            .output()
            .unwrap_or_else(|error| panic!("git {args:?}: {error}"));

        assert!(
            output.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

/// Each print of the session, as its channel, its format, and its text.
fn prints(messages: &[PluginToHost]) -> Vec<(&str, &str, &str)> {
    messages
        .iter()
        .filter_map(|message| match message {
            PluginToHost::Print(print) => Some((
                print.channel.as_str(),
                print.format.as_str(),
                print.text.as_str(),
            )),
            _ => None,
        })
        .collect()
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
fn render_prints_the_unreleased_fragments_as_release_notes() {
    let repository = Repository::new("https://github.com/dcdpr/jp");
    repository.write(
        "changelog.d/retry-idle-turns.md",
        "---\ntype: fixed\n---\nIdle turns are retried.\n",
    );
    repository.commit("fix(llm): Retry idle turns (#1207)");

    let messages = command(repository.path(), &["render"], &[]);

    assert_eq!(prints(&messages), [("content", "plain", indoc! {"
        1 entry is unreleased: Fixed (1).

        ## Changelog

        ### Fixed

        - Idle turns are retried.
          ([#1207](https://github.com/dcdpr/jp/pull/1207))
    "})]);
    assert_eq!(exit(&messages).code, 0);
}

/// A self-hosted GitLab can't be told from its address, so the project names
/// its forge.
#[test]
fn render_credits_the_configured_forge_under_the_configured_headings() {
    let repository = Repository::new("git@git.example.com:group/project.git");
    repository.write(
        "changelog.d/retry-idle-turns.md",
        "---\ntype: fixed\n---\nIdle turns are retried.\n",
    );
    repository.commit("Retry idle turns\n\nSee merge request group/project!42");
    let mut message = init_message(repository.path(), &["render"]);
    message.options = Map::from_iter([
        ("forge".to_owned(), json!("gitlab")),
        ("headings".to_owned(), json!({ "fixed": "Bug fixes" })),
    ]);

    let messages = session(&HostToPlugin::Init(Box::new(message)), &[]);

    assert_eq!(printed(&messages), indoc! {"
        1 entry is unreleased: Bug fixes (1).

        ## Changelog

        ### Bug fixes

        - Idle turns are retried.
          ([!42](https://git.example.com/group/project/-/merge_requests/42))
    "});
}

#[test]
fn render_prints_a_releases_upgrade_guide() {
    let repository = Repository::new("https://github.com/dcdpr/jp");
    repository.write("changelog.d/0.3.0/plugin-manifest-routing.md", indoc! {"
            ---
            type: changed
            change_requests: [1216]
            ---
            JP finds a command plugin by its manifest.

            BREAKING CHANGE: Command plugins are found by their manifest

            Rebuild the plugin.
        "});
    repository.commit("chore(release): Cut 0.3.0 (#1250)");

    let messages = command(
        repository.path(),
        &["render", "0.3.0", "--page", "upgrade"],
        &[],
    );

    assert_eq!(printed(&messages), indoc! {"
        ## Command plugins are found by their manifest

        [#1216](https://github.com/dcdpr/jp/pull/1216)

        Rebuild the plugin.
    "});
}

/// An upgrade guide without sections is nothing to print, which a reader at a
/// terminal is told on stderr.
#[test]
fn render_says_when_there_is_no_upgrade_guide() {
    let repository = Repository::new("https://github.com/dcdpr/jp");
    repository.write(
        "changelog.d/0.3.0/retry-idle-turns.md",
        "---\ntype: fixed\n---\nIdle turns are retried.\n",
    );
    repository.commit("chore(release): Cut 0.3.0 (#1250)");

    let messages = command(
        repository.path(),
        &["render", "v0.3.0", "--page", "upgrade"],
        &[],
    );

    assert_eq!(prints(&messages), [(
        "chrome",
        "plain",
        "0.3.0 has no upgrade guide: none of its entries has an upgrade section.\n"
    )]);
    assert_eq!(exit(&messages).code, 0);
}

/// JSON is printed as JSON, so `jp` renders it as such, and it is the default
/// when `jp` itself prints JSON.
#[test]
fn render_prints_json_when_asked_or_when_jp_prints_json() {
    let repository = Repository::new("https://github.com/dcdpr/jp");
    repository.write(
        "changelog.d/retry-idle-turns.md",
        "---\ntype: fixed\n---\nIdle turns are retried.\n",
    );
    repository.commit("fix(llm): Retry idle turns (#1207)");
    let mut message = init_message(repository.path(), &["render"]);
    message.output_format = OutputFormat::Json;

    for messages in [
        command(repository.path(), &["render", "--format", "json"], &[]),
        session(&HostToPlugin::Init(Box::new(message)), &[]),
    ] {
        let [("content", "json", text)] = prints(&messages)[..] else {
            panic!("expected one JSON print, got {messages:?}");
        };
        let release: Value = serde_json::from_str(text).unwrap();
        assert_eq!(release["version"], Value::Null);
        assert_eq!(release["entries"][0]["credits"][0]["label"], "#1207");
    }
}

#[test]
fn render_refuses_a_flag_that_does_not_apply() {
    let root = workspace();

    let messages = command(
        root.path(),
        &["render", "--format", "json", "--page", "upgrade"],
        &[],
    );

    let exit = exit(&messages);
    assert_eq!(exit.code, 1);
    assert_eq!(
        exit.reason.as_deref(),
        Some(
            "`--page` names a Markdown document; JSON holds the whole release, so drop it or add \
             `--format markdown`."
        )
    );
}

#[test]
fn render_names_what_breaks_the_format_from_the_workspace_root() {
    let root = workspace();
    fs::create_dir(root.path().join("changelog.d/0.3.0")).unwrap();
    fs::write(root.path().join("changelog.d/0.3.0/notes.txt"), "Notes.\n").unwrap();

    let messages = command(root.path(), &["render", "0.3.0"], &[]);

    assert_eq!(
        exit(&messages).reason.as_deref(),
        Some(
            "Fix these first, which `jp release check` reports too:\nchangelog.d/0.3.0/notes.txt: \
             not a fragment, which is named `<slug>.md`; rename it, or move it out of the \
             fragments directory"
        )
    );
}

#[test]
fn render_of_a_release_that_is_not_there_says_so() {
    let root = workspace();

    let messages = command(root.path(), &["render", "0.4.0"], &[]);

    assert_eq!(
        exit(&messages).reason.as_deref(),
        Some("changelog.d has no release 0.4.0: no directory 0.4.0/")
    );
}

#[test]
fn render_without_a_fragments_directory_says_how_to_make_one() {
    let root = Utf8TempDir::new().unwrap();

    let messages = command(root.path(), &["render"], &[]);

    assert_eq!(
        exit(&messages).reason.as_deref(),
        Some("There is no fragments directory at changelog.d; create it with `jp release init`.")
    );
}

#[test]
fn making_a_release_needs_a_git_repository() {
    let root = workspace();

    for args in [
        &["cut"][..],
        &["finalize", "0.1.0"],
        &["pick", "--continue"],
        &["due"],
    ] {
        let messages = command(root.path(), args, &[]);

        let reason = exit(&messages).reason.clone().unwrap_or_default();
        assert!(
            reason.starts_with(&format!(
                "The workspace at {} isn't in a git repository",
                root.path()
            )),
            "{args:?}: {reason}"
        );
    }
}

#[test]
fn pick_takes_a_version_and_commits_or_continue() {
    let root = workspace();

    for args in [
        &["pick"][..],
        &["pick", "0.1.1"],
        &["pick", "--continue", "0.1.1", "4e1f0a2"],
        &["pick", "v0.1", "4e1f0a2"],
    ] {
        let messages = command(root.path(), args, &[]);

        assert_eq!(exit(&messages).code, 1, "{args:?}");
    }
}

#[test]
fn continue_without_a_stopped_pick_has_nothing_to_do() {
    let fixture = Fixture::new();

    let messages = command(fixture.root(), &["pick", "--continue"], &[]);

    assert_eq!(
        exit(&messages).reason.as_deref(),
        Some("No pick is stopped, so there is nothing to continue.")
    );
}

#[test]
fn due_says_whether_the_next_release_is_due_and_exits_1_when_it_is_not() {
    let fixture = Fixture::new();

    let messages = command(fixture.root(), &["due"], &[]);

    assert_eq!(prints(&messages), [(
        "content",
        "plain",
        "No release has been cut yet, so the first one is due.\n"
    )]);
    assert_eq!(exit(&messages).code, 0);

    fixture.fragment("retry", "fixed", "Retried.");
    fixture.commit("fix: Retry (#1)");
    fixture.push();
    let shell = fixture.shell();
    let options = Options::default();
    let pipeline = Pipeline::new(&shell, fixture.root(), &options, None).unwrap();
    cut::cut(&pipeline, None).unwrap();

    let messages = command(fixture.root(), &["due"], &[]);

    assert_eq!(
        printed(&messages),
        "0.1.0 is in preparation, so the next release is due once it is finalized.\n"
    );
    // An answer, not a failure: `jp` reports no error for it.
    let exit = exit(&messages);
    assert_eq!(exit.code, 1);
    assert!(exit.expected);
    assert_eq!(exit.reason, None);
}

/// A script reads the answer from the JSON, and so can tell a release that
/// isn't due from a `jp` that failed.
#[test]
fn due_prints_json_when_jp_prints_json() {
    let fixture = Fixture::new();
    let mut message = init_message(fixture.root(), &["due"]);
    message.output_format = OutputFormat::Json;

    let messages = session(&HostToPlugin::Init(Box::new(message)), &[]);

    let [("content", "json", text)] = prints(&messages)[..] else {
        panic!("expected one JSON print, got {messages:?}");
    };
    assert_eq!(
        serde_json::from_str::<Value>(text).unwrap(),
        json!({
            "due": true,
            "reason": "No release has been cut yet, so the first one is due."
        })
    );
}

#[test]
fn tools_init_writes_the_definition_and_leaves_it_as_shipped() {
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

    assert_eq!(
        printed(&messages),
        format!(
            "Unchanged {}\nLoad the directory from the workspace configuration for the tools to \
             appear.\n",
            dir.join("add.toml")
        )
    );
}

/// A changed definition outside git is left alone unless `--force` is passed.
#[test]
fn tools_init_overwrites_a_changed_definition_only_with_force() {
    let root = Utf8TempDir::new().unwrap();
    let dir = root.path().join(".jp/mcp/tools/release");
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("add.toml"), "# mine\n").unwrap();

    let messages = command(root.path(), &["tools", "init", ".jp/mcp/tools/release"], &[
    ]);

    assert_eq!(exit(&messages).code, 1);
    assert_eq!(
        fs::read_to_string(dir.join("add.toml")).unwrap(),
        "# mine\n"
    );

    command(
        root.path(),
        &["tools", "init", ".jp/mcp/tools/release", "--force"],
        &[],
    );

    assert_eq!(
        fs::read_to_string(dir.join("add.toml")).unwrap(),
        include_str!("../tools/add.toml")
    );
}
