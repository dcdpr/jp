use std::{env, fs, io::Cursor, process::Command as Process};

use camino_tempfile::Utf8TempDir;
use clap::Error as ClapError;
use jp_plugin::message::{ComposeResponse, OutputFormat, PathsInfo, WorkspaceInfo};
use serde_json::{Map, json};

use super::*;

fn parse(args: &[&str]) -> Result<Args, ClapError> {
    Args::try_parse_from(iter::once("jp vite").chain(args.iter().copied()))
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

/// Drive the plugin through a conversation: `messages` are what the host says,
/// in order, starting with `init`.
fn exchange(messages: &[HostToPlugin]) -> Vec<PluginToHost> {
    let input: String = messages
        .iter()
        .map(|message| serde_json::to_string(message).unwrap() + "\n")
        .collect();
    let mut output = vec![];
    run(Cursor::new(input.into_bytes()), &mut output).unwrap();

    String::from_utf8(output)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

/// What a command printed for the user, and how it exited.
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

fn composed(text: Option<&str>) -> HostToPlugin {
    HostToPlugin::Composed(ComposeResponse {
        id: None,
        text: text.map(ToOwned::to_owned),
        values: vec![],
    })
}

/// Run git in `dir`, failing the test with its stderr.
fn git(dir: &Utf8Path, args: &[&str]) {
    let output = Process::new("git")
        .current_dir(dir)
        .args(args)
        .output()
        .unwrap_or_else(|error| panic!("git {args:?}: {error}"));

    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

/// A repository holding a site in `docs` with both workflows, everything
/// committed.
///
/// The config is set locally so the machine's identity and signing setup can't
/// reach in and change what the test does.
fn project() -> Utf8TempDir {
    let root = Utf8TempDir::new().unwrap();
    git(root.path(), &["init", "--quiet", "--initial-branch=main"]);
    git(root.path(), &["config", "user.name", "Tester"]);
    git(root.path(), &["config", "user.email", "tester@example.com"]);
    git(root.path(), &["config", "commit.gpgsign", "false"]);

    fs::create_dir_all(root.path().join("docs/rfd")).unwrap();
    fs::create_dir_all(root.path().join("docs/ticket")).unwrap();
    fs::write(root.path().join("docs/package.json"), "{}\n").unwrap();
    fs::write(root.path().join("docs/ticket/.keep"), "").unwrap();
    fs::write(root.path().join("docs/rfd/.keep"), "").unwrap();
    commit(root.path());

    root
}

fn commit(root: &Utf8Path) {
    git(root, &["add", "-A"]);
    git(root, &["commit", "--quiet", "-m", "Commit"]);
}

#[test]
fn cli_definition_is_valid() {
    Args::command().debug_assert();
}

/// Everything after `serve` reaches `vitepress dev`, flags included.
#[test]
fn serve_passes_its_arguments_to_vitepress() {
    for args in [
        ["serve", "--host", "--open", "/rfd/priority"].as_slice(),
        ["serve", "--", "--host", "--open", "/rfd/priority"].as_slice(),
    ] {
        match parse(args).unwrap().command {
            Command::Serve { vitepress } => {
                assert_eq!(vitepress, ["--host", "--open", "/rfd/priority"], "{args:?}");
            }
            other => panic!("expected serve, got {other:?}"),
        }
    }
}

/// `--check` writes nothing, so there is nothing to force or confirm.
#[test]
fn check_takes_neither_force_nor_yes() {
    assert!(parse(&["sync", "--check", "--force"]).is_err());
    assert!(parse(&["sync", "--check", "--yes"]).is_err());
    assert!(parse(&["sync", "--force", "--yes"]).is_ok());
}

/// `jp vite serve` reads the revision out of every plugin's `--version`, its
/// own included.
#[test]
fn the_version_names_the_site_revision() {
    let root = Utf8TempDir::new().unwrap();

    let (text, code, _) = printed(&exchange(&[init(root.path(), &["--version"])]));

    assert_eq!(code, 0);
    assert_eq!(text, format!("jp vite {VERSION}\n"));
    assert_eq!(serve::reported_revision(&text), Some(site::REVISION));
}

#[test]
fn describe_names_the_command() {
    let messages = exchange(&[HostToPlugin::Describe]);

    let [PluginToHost::Describe(describe)] = messages.as_slice() else {
        panic!("expected one describe answer: {messages:?}");
    };
    assert_eq!(describe.name, "vite");
    assert_eq!(describe.manifest.command, ["vite"]);
}

#[test]
fn a_tool_call_is_refused() {
    let root = Utf8TempDir::new().unwrap();
    let mut message = init_message(root.path(), &[]);
    message.tool = Some(
        serde_json::from_value(json!({
            "action": "run",
            "name": "sync",
            "arguments": {},
            "conversation": "test",
        }))
        .unwrap(),
    );

    let (_, code, reason) = printed(&exchange(&[HostToPlugin::Init(Box::new(message))]));

    assert_eq!(
        (code, reason.as_deref()),
        (1, Some("`jp vite` serves no tools."))
    );
}

#[test]
fn init_writes_the_project_files_with_the_answers() {
    let root = Utf8TempDir::new().unwrap();
    fs::create_dir_all(root.path().join("docs/ticket")).unwrap();

    let messages = exchange(&[
        init(root.path(), &["init", "--description", "Plans and work"]),
        composed(Some("Acme")),
    ]);

    let (text, code, reason) = printed(&messages);
    assert_eq!((code, reason), (0, None));
    assert_eq!(
        text,
        "Created docs/package.json\nCreated docs/.vitepress/config.mts\nCreated \
         docs/.vitepress/theme/index.js\nCreated docs/index.md\nThe configuration imports files \
         `jp vite sync` writes, with the RFD and ticket pages: run it next, then `jp vite serve` \
         to see the site.\n"
    );

    let config = fs::read_to_string(root.path().join("docs/.vitepress/config.mts")).unwrap();
    assert!(config.contains("    title: \"Acme\",\n"), "{config}");
    assert!(
        config.contains("    description: \"Plans and work\",\n"),
        "{config}"
    );
    assert!(
        config.contains("            { text: \"Tickets\", link: \"/ticket/\" },\n"),
        "{config}"
    );
    assert!(!config.contains("RFDs"), "{config}");
}

/// Nobody to ask, and no flag: nothing is written rather than a site without a
/// title.
#[test]
fn init_without_a_title_writes_nothing() {
    let root = Utf8TempDir::new().unwrap();

    let messages = exchange(&[init(root.path(), &["init"]), composed(None)]);

    let (_, code, reason) = printed(&messages);
    assert_eq!(code, 1);
    assert_eq!(
        reason.as_deref(),
        Some(
            "Nothing written: `jp vite init` needs an answer to \"The site's title\". Answer it \
             at a terminal, or pass `--title`."
        )
    );
    assert!(!root.path().join("docs").exists());
}

#[test]
fn init_leaves_a_set_up_site_alone() {
    let root = Utf8TempDir::new().unwrap();
    exchange(&[init(root.path(), &[
        "init",
        "--title",
        "A",
        "--description",
        "B",
    ])]);

    let (text, code, _) = printed(&exchange(&[init(root.path(), &["init"])]));

    assert_eq!(code, 0);
    assert_eq!(
        text,
        "docs/ already has its package.json, configuration, theme, and homepage.\n"
    );
}

#[test]
fn sync_writes_the_site_files_and_check_agrees() {
    let root = project();

    let (text, code, reason) = printed(&exchange(&[init(root.path(), &["sync", "--yes"])]));
    assert_eq!((code, reason), (0, None));
    assert!(text.starts_with("`jp vite sync` will change:\n"), "{text}");
    assert!(
        text.contains("  write   docs/.vitepress/jp-vite/config.mjs\n"),
        "{text}"
    );
    assert!(text.contains("  create  docs/rfd/index.md\n"), "{text}");
    assert!(text.contains("  create  docs/ticket/board.md\n"), "{text}");
    assert!(text.ends_with("`git diff` shows what changed.\n"), "{text}");

    assert!(
        root.path()
            .join("docs/.vitepress/jp-vite/settings.json")
            .is_file()
    );
    assert!(
        fs::read_to_string(root.path().join("docs/rfd/priority.md"))
            .unwrap()
            .contains("\n<RfdBoard />\n")
    );

    let (text, code, _) = printed(&exchange(&[init(root.path(), &["sync", "--check"])]));
    assert_eq!(code, 0);
    assert_eq!(
        text,
        "The site's files are the ones `jp vite sync` writes.\n"
    );
}

#[test]
fn check_names_the_files_that_differ() {
    let root = project();
    exchange(&[init(root.path(), &["sync", "--yes"])]);
    fs::write(
        root.path().join("docs/.vitepress/jp-vite/config.mjs"),
        "Edited",
    )
    .unwrap();
    fs::remove_file(root.path().join("docs/ticket/board.md")).unwrap();

    let (_, code, reason) = printed(&exchange(&[init(root.path(), &["sync", "--check"])]));

    assert_eq!(code, 1);
    assert_eq!(
        reason.as_deref(),
        Some(
            "`jp vite sync` would make these changes:\n  write   \
             docs/.vitepress/jp-vite/config.mjs\n  create  docs/ticket/board.md\nRun `jp vite \
             sync` to make them."
        )
    );
}

/// A page is the project's once it exists: sync never rewrites it, and check
/// doesn't compare it.
#[test]
fn a_page_the_project_wrote_is_left_alone() {
    let root = project();
    exchange(&[init(root.path(), &["sync", "--yes"])]);
    commit(root.path());
    fs::write(
        root.path().join("docs/rfd/index.md"),
        "# Our RFDs\n\n<RfdIndex />\n",
    )
    .unwrap();

    let (text, code, _) = printed(&exchange(&[init(root.path(), &["sync", "--check"])]));
    assert_eq!(code, 0);
    assert_eq!(
        text,
        "The site's files are the ones `jp vite sync` writes.\n"
    );

    let (_, code, _) = printed(&exchange(&[init(root.path(), &["sync", "--yes"])]));
    assert_eq!(code, 0);
    assert_eq!(
        fs::read_to_string(root.path().join("docs/rfd/index.md")).unwrap(),
        "# Our RFDs\n\n<RfdIndex />\n"
    );
}

/// "Refuses when an owned file is modified or untracked, since git cannot
/// restore an edit it never recorded."
#[test]
fn sync_refuses_to_overwrite_what_git_has_not_recorded() {
    let root = project();
    exchange(&[init(root.path(), &["sync", "--yes"])]);
    commit(root.path());
    let config = root.path().join("docs/.vitepress/jp-vite/config.mjs");
    fs::write(&config, "Edited").unwrap();
    fs::write(
        root.path().join("docs/.vitepress/jp-vite/stray.mjs"),
        "Stray",
    )
    .unwrap();

    let (_, code, reason) = printed(&exchange(&[init(root.path(), &["sync", "--yes"])]));

    assert_eq!(code, 1);
    assert_eq!(
        reason.as_deref(),
        Some(
            "Git couldn't restore these files, so `jp vite sync` leaves them:\n  \
             docs/.vitepress/jp-vite/config.mjs (changed since the last commit)\n  \
             docs/.vitepress/jp-vite/stray.mjs (not tracked)\nCommit them, or run `jp vite sync \
             --force` to overwrite them anyway."
        )
    );
    assert_eq!(fs::read_to_string(&config).unwrap(), "Edited");

    let (_, code, _) = printed(&exchange(&[init(root.path(), &[
        "sync", "--yes", "--force",
    ])]));
    assert_eq!(code, 0);
    assert_ne!(fs::read_to_string(&config).unwrap(), "Edited");
    assert!(
        !root
            .path()
            .join("docs/.vitepress/jp-vite/stray.mjs")
            .exists()
    );
}

/// A file that doesn't exist yet risks nothing, and the snapshot, the pages,
/// and the loaders of a new install are all new.
#[test]
fn sync_writes_new_files_in_a_dirty_tree() {
    let root = project();
    fs::write(root.path().join("docs/notes.md"), "Uncommitted").unwrap();

    let (_, code, reason) = printed(&exchange(&[init(root.path(), &["sync", "--yes"])]));

    assert_eq!((code, reason), (0, None));
}

#[test]
fn sync_asks_before_writing() {
    let root = project();

    let messages = exchange(&[init(root.path(), &["sync"]), composed(Some("write"))]);

    let asked = messages.iter().find_map(|message| match message {
        PluginToHost::Compose(request) => Some(request),
        _ => None,
    });
    assert!(
        asked.is_some_and(|request| request.message.starts_with("Change ")),
        "{messages:?}"
    );
    assert_eq!(printed(&messages).1, 0);
    assert!(
        root.path()
            .join("docs/.vitepress/jp-vite/config.mjs")
            .is_file()
    );
}

#[test]
fn a_declined_sync_writes_nothing() {
    let root = project();

    let (text, code, _) = printed(&exchange(&[
        init(root.path(), &["sync"]),
        composed(Some("skip")),
    ]));

    assert_eq!(code, 0);
    assert!(text.ends_with("Nothing written.\n"), "{text}");
    assert!(!root.path().join("docs/.vitepress/jp-vite").exists());
}

/// A script with no terminal has to say `--yes`; a run that writes nothing must
/// not look like one that synced.
#[test]
fn a_sync_with_nobody_to_ask_fails() {
    let root = project();

    let (_, code, reason) = printed(&exchange(&[init(root.path(), &["sync"]), composed(None)]));

    assert_eq!(code, 1);
    assert_eq!(
        reason.as_deref(),
        Some(
            "Nothing written: there is no terminal to ask on. Run it again with `--yes` to write \
             without asking."
        )
    );
}

#[test]
fn sync_needs_a_git_work_tree() {
    let root = Utf8TempDir::new().unwrap();
    fs::create_dir_all(root.path().join("docs")).unwrap();

    let (_, code, reason) = printed(&exchange(&[init(root.path(), &["sync", "--yes"])]));

    assert_eq!(code, 1);
    assert_eq!(
        reason.as_deref(),
        Some(
            "`jp vite sync` writes only in a git work tree, where git can restore what it \
             overwrites."
        )
    );
}

#[test]
fn sync_needs_a_site() {
    let root = Utf8TempDir::new().unwrap();

    let (_, code, reason) = printed(&exchange(&[init(root.path(), &["sync", "--check"])]));

    assert_eq!(code, 1);
    assert_eq!(
        reason.as_deref(),
        Some("There is no site in docs: run `jp vite init` to set one up.")
    );
}

/// JP's own configuration, as the workspace layer sets it.
fn jp_config(root: &Utf8Path) -> Value {
    let path = root.join(".jp/config.toml");
    let config: toml::Value = toml::from_str(&fs::read_to_string(&path).unwrap())
        .unwrap_or_else(|error| panic!("{path}: {error}"));

    serde_json::to_value(config).unwrap()
}

/// JP's own site is synced from this binary, like any other project's.
///
/// Fails when a file in the crate's `site/` changed without JP's copy being
/// synced, or JP's configuration changed what the snapshot holds.
/// Run `jp vite sync` to fix it, or, without the plugins installed,
/// `JP_VITE_BLESS=1 cargo test -p jp-vite jp_site_is_synced`, which writes what
/// the test expects.
#[test]
fn jp_site_is_synced() {
    let root = Utf8PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../..");
    let config = jp_config(&root);
    let options = Options::from_config(&config).unwrap();
    let settings = snapshot(&root, &options, &config, None).unwrap();
    let owned_dir = site::owned_dir(&settings);
    let changes = changes(&root, &settings).unwrap();

    if env::var_os("JP_VITE_BLESS").is_some() {
        sync::apply(&root, &changes, &owned_dir).unwrap();
        return;
    }

    assert!(
        changes.is_empty(),
        "JP's site isn't the one this `jp vite` writes:\n{}Run `jp vite sync`, or \
         `JP_VITE_BLESS=1 cargo test -p jp-vite jp_site_is_synced`.",
        listing(&changes)
    );
}
