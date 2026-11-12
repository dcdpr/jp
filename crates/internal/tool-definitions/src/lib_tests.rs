#[cfg(unix)]
use std::os::unix::fs::symlink;
use std::process::Command;

use camino_tempfile::Utf8TempDir;
use serde_json::json;

use super::*;

const NOTES: Definitions = Definitions {
    plugin: "notes",
    files: &[("create.toml", "# create\n"), ("list.toml", "# list\n")],
    offers: "categories",
};

const SYNCED_NOTES: &str = "#\n# Mirrors the categories.\n";

fn categories() -> Vec<String> {
    vec!["design".to_owned(), "guide".to_owned()]
}

fn categorized(source: &str) -> Value {
    json!({ "source": source, "parameters": { "category": { "type": "string" } } })
}

fn sync(dir: &Utf8Path, config: &Value) -> Result<String, String> {
    sync_with(dir, config, false)
}

fn sync_with(dir: &Utf8Path, config: &Value, force: bool) -> Result<String, String> {
    let categories = categories();

    NOTES.sync(
        dir,
        config,
        SYNCED_NOTES,
        |tool| match tool {
            "create" | "list" => vec![Mirror::one_of("category", &categories)],
            _ => vec![],
        },
        force,
    )
}

/// Run git in `dir` without the user's or the system's git configuration, which
/// can ask for a signature on every commit.
fn git(dir: &Utf8Path, args: &[&str]) {
    let output = Command::new("git")
        .current_dir(dir)
        .args(args)
        .env("GIT_CONFIG_GLOBAL", "")
        .env("GIT_CONFIG_SYSTEM", "")
        .output()
        .unwrap_or_else(|error| panic!("git {args:?}: {error}"));

    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn repository() -> Utf8TempDir {
    let dir = Utf8TempDir::new().unwrap();
    git(dir.path(), &["init", "--quiet", "--initial-branch=main"]);
    git(dir.path(), &["config", "user.name", "Tester"]);
    git(dir.path(), &["config", "user.email", "tester@example.com"]);

    dir
}

fn commit(dir: &Utf8Path) {
    git(dir, &["add", "-A"]);
    git(dir, &["commit", "--quiet", "-m", "Commit"]);
}

#[test]
fn init_writes_one_file_per_definition() {
    let dir = Utf8TempDir::new().unwrap();
    let target = dir.path().join("tools/notes");

    let text = NOTES.init(&target, false).unwrap();

    assert_eq!(
        fs::read_to_string(target.join("create.toml")).unwrap(),
        "# create\n"
    );
    assert_eq!(
        fs::read_to_string(target.join("list.toml")).unwrap(),
        "# list\n"
    );
    assert_eq!(
        text,
        format!(
            "Wrote {}\nWrote {}\nLoad the directory from the workspace configuration, then run \
             `jp notes tools sync {target}` to offer the categories.\n",
            target.join("create.toml"),
            target.join("list.toml")
        )
    );
}

#[test]
fn init_refuses_to_overwrite_outside_git_and_writes_nothing() {
    let dir = Utf8TempDir::new().unwrap();
    fs::write(dir.path().join("list.toml"), "# mine\n").unwrap();

    let error = NOTES.init(dir.path(), false).unwrap_err();

    assert_eq!(
        error,
        format!(
            "Git couldn't restore these files, so `jp notes tools init {}` leaves them:\n  {} \
             (not in a git work tree)\nRun `jp notes tools init {} --force` to overwrite them \
             anyway.",
            dir.path(),
            dir.path().join("list.toml"),
            dir.path()
        )
    );
    assert_eq!(
        fs::read_to_string(dir.path().join("list.toml")).unwrap(),
        "# mine\n"
    );
    assert!(!dir.path().join("create.toml").exists());
}

#[test]
fn init_leaves_a_file_as_shipped_alone() {
    let dir = Utf8TempDir::new().unwrap();
    let create = dir.path().join("create.toml");
    let list = dir.path().join("list.toml");
    fs::write(&create, "# create\n").unwrap();

    let text = NOTES.init(dir.path(), false).unwrap();

    assert_eq!(
        text,
        format!(
            "Unchanged {create}\nWrote {list}\nLoad the directory from the workspace \
             configuration, then run `jp notes tools sync {}` to offer the categories.\n",
            dir.path()
        )
    );
    assert_eq!(fs::read_to_string(&list).unwrap(), "# list\n");
}

#[test]
fn init_overwrites_what_git_can_restore() {
    let dir = repository();
    let list = dir.path().join("list.toml");
    fs::write(&list, "# mine\n").unwrap();
    commit(dir.path());

    let text = NOTES.init(dir.path(), false).unwrap();

    assert_eq!(
        text,
        format!(
            "Wrote {}\nOverwrote {list}\nLoad the directory from the workspace configuration, \
             then run `jp notes tools sync {}` to offer the categories.\n",
            dir.path().join("create.toml"),
            dir.path()
        )
    );
    assert_eq!(fs::read_to_string(&list).unwrap(), "# list\n");
}

#[test]
fn init_refuses_an_edit_git_has_not_recorded() {
    let dir = repository();
    let list = dir.path().join("list.toml");
    fs::write(&list, "# mine\n").unwrap();
    commit(dir.path());
    fs::write(&list, "# mine, edited\n").unwrap();

    let error = NOTES.init(dir.path(), false).unwrap_err();

    assert_eq!(
        error,
        format!(
            "Git couldn't restore these files, so `jp notes tools init {}` leaves them:\n  {list} \
             (changed since the last commit)\nCommit them, or run `jp notes tools init {} \
             --force` to overwrite them anyway.",
            dir.path(),
            dir.path()
        )
    );
    assert_eq!(fs::read_to_string(&list).unwrap(), "# mine, edited\n");
    assert!(!dir.path().join("create.toml").exists());
}

#[cfg(unix)]
#[test]
fn init_refuses_to_write_through_a_committed_symlink() {
    let dir = repository();
    let shared = Utf8TempDir::new().unwrap();
    let target = shared.path().join("list.toml");
    fs::write(&target, "# shared\n").unwrap();
    let list = dir.path().join("list.toml");
    symlink(&target, &list).unwrap();
    commit(dir.path());

    let error = NOTES.init(dir.path(), false).unwrap_err();

    assert_eq!(
        error,
        format!(
            "Git couldn't restore these files, so `jp notes tools init {}` leaves them:\n  {list} \
             (reached through a symlink)\nRun `jp notes tools init {} --force` to overwrite them \
             anyway.",
            dir.path(),
            dir.path()
        )
    );
    assert_eq!(fs::read_to_string(&target).unwrap(), "# shared\n");
    assert!(!dir.path().join("create.toml").exists());
}

#[test]
fn init_refuses_an_edit_hidden_by_assume_unchanged() {
    let dir = repository();
    let list = dir.path().join("list.toml");
    fs::write(&list, "# mine\n").unwrap();
    commit(dir.path());
    git(dir.path(), &[
        "update-index",
        "--assume-unchanged",
        "list.toml",
    ]);
    fs::write(&list, "# mine, customized\n").unwrap();

    let error = NOTES.init(dir.path(), false).unwrap_err();

    assert_eq!(
        error,
        format!(
            "Git couldn't restore these files, so `jp notes tools init {}` leaves them:\n  {list} \
             (marked assume-unchanged or skip-worktree)\nRun `jp notes tools init {} --force` to \
             overwrite them anyway.",
            dir.path(),
            dir.path()
        )
    );
    assert_eq!(fs::read_to_string(&list).unwrap(), "# mine, customized\n");
    assert!(!dir.path().join("create.toml").exists());
}

#[test]
fn init_with_force_overwrites_a_file_that_differs() {
    let dir = Utf8TempDir::new().unwrap();
    let create = dir.path().join("create.toml");
    let list = dir.path().join("list.toml");
    fs::write(&create, "# create\n").unwrap();
    fs::write(&list, "# mine\n").unwrap();

    let text = NOTES.init(dir.path(), true).unwrap();

    assert_eq!(
        text,
        format!(
            "Unchanged {create}\nOverwrote {list}\nLoad the directory from the workspace \
             configuration, then run `jp notes tools sync {}` to offer the categories.\n",
            dir.path()
        )
    );
    assert_eq!(fs::read_to_string(&list).unwrap(), "# list\n");
}

#[test]
fn sync_writes_what_each_tool_takes_found_by_source() {
    let dir = Utf8TempDir::new().unwrap();
    let config = json!({ "conversation": { "tools": {
        "*": { "run": "ask" },
        "notes_list": categorized("plugin.command.notes.list"),
        "board": categorized("plugin.command.notes.list"),
        "notes_show": { "source": "plugin.command.notes.show" },
        "rfd_list": categorized("plugin.command.rfd.list"),
        "notes_create": categorized("local"),
    }}});

    let text = sync(dir.path(), &config).unwrap();

    let path = dir.path().join(SYNCED_FILE);
    assert_eq!(
        text,
        format!("Wrote {path}, with categories for board, notes_list\n")
    );
    assert_eq!(
        fs::read_to_string(&path).unwrap(),
        format!(
            "# Generated by `jp notes tools \
             sync`.\n{SYNCED_NOTES}\n[conversation.tools.board.parameters.category]\nenum = [\n  \
             \"design\",\n  \
             \"guide\",\n]\n\n[conversation.tools.notes_list.parameters.category]\nenum = [\n  \
             \"design\",\n  \"guide\",\n]\n"
        )
    );
}

#[test]
fn sync_leaves_out_a_parameter_a_tool_doesnt_declare() {
    let dir = Utf8TempDir::new().unwrap();
    let config = json!({ "conversation": { "tools": {
        "notes_list": categorized("plugin.command.notes.list"),
        "notes_create": {
            "source": "plugin.command.notes.create",
            "parameters": { "category": { "enum": ["design"] } }
        },
    }}});

    let text = sync(dir.path(), &config).unwrap();

    let path = dir.path().join(SYNCED_FILE);
    assert_eq!(
        text,
        format!(
            "Wrote {path}, with categories for notes_list\nLeft out notes_create (no `category` \
             parameter): the values go into that parameter, which their definitions don't \
             declare.\n"
        )
    );
    assert!(!fs::read_to_string(&path).unwrap().contains("notes_create"));
}

#[test]
fn sync_rewrites_its_file_whole() {
    let dir = repository();
    let configured = json!({ "conversation": { "tools": {
        "notes_list": categorized("plugin.command.notes.list"),
    }}});

    sync(dir.path(), &configured).unwrap();
    commit(dir.path());
    let text = sync(dir.path(), &json!({})).unwrap();

    let path = dir.path().join(SYNCED_FILE);
    assert_eq!(
        text,
        format!("Wrote {path}; no configured tool takes categories from `jp notes`.\n")
    );
    assert_eq!(
        fs::read_to_string(&path).unwrap(),
        format!("# Generated by `jp notes tools sync`.\n{SYNCED_NOTES}")
    );
}

#[test]
fn sync_leaves_an_unchanged_file_untouched() {
    let dir = Utf8TempDir::new().unwrap();
    let path = dir.path().join(SYNCED_FILE);
    let config = json!({ "conversation": { "tools": {
        "notes_list": categorized("plugin.command.notes.list"),
    }}});
    sync(dir.path(), &config).unwrap();
    // Make the file read-only so any write attempt fails the test.
    let mut permissions = fs::metadata(&path).unwrap().permissions();
    permissions.set_readonly(true);
    fs::set_permissions(&path, permissions).unwrap();

    let text = sync(dir.path(), &config).unwrap();

    assert_eq!(
        text,
        format!("Unchanged {path}, with categories for notes_list\n")
    );
}

#[test]
fn sync_refuses_to_overwrite_outside_git_unless_forced() {
    let dir = Utf8TempDir::new().unwrap();
    let path = dir.path().join(SYNCED_FILE);
    let configured = json!({ "conversation": { "tools": {
        "notes_list": categorized("plugin.command.notes.list"),
    }}});
    sync(dir.path(), &configured).unwrap();
    let before = fs::read_to_string(&path).unwrap();

    let error = sync(dir.path(), &json!({})).unwrap_err();

    assert_eq!(
        error,
        format!(
            "Git couldn't restore these files, so `jp notes tools sync {}` leaves them:\n  {path} \
             (not in a git work tree)\nRun `jp notes tools sync {} --force` to overwrite them \
             anyway.",
            dir.path(),
            dir.path()
        )
    );
    assert_eq!(fs::read_to_string(&path).unwrap(), before);

    sync_with(dir.path(), &json!({}), true).unwrap();
    assert_eq!(
        fs::read_to_string(&path).unwrap(),
        format!("# Generated by `jp notes tools sync`.\n{SYNCED_NOTES}")
    );
}

#[test]
fn sync_leaves_a_file_it_did_not_write_alone() {
    let dir = repository();
    let path = dir.path().join(SYNCED_FILE);
    fs::write(&path, "# Generated by `jp rfd tools sync`.\n").unwrap();
    commit(dir.path());

    let error = sync(dir.path(), &json!({})).unwrap_err();

    assert_eq!(
        error,
        format!(
            "{path} was not written by `jp notes tools sync`, so it is left alone. Move it aside, \
             or run `jp notes tools sync {} --force` to overwrite it.",
            dir.path()
        )
    );
    assert_eq!(
        fs::read_to_string(&path).unwrap(),
        "# Generated by `jp rfd tools sync`.\n"
    );

    sync_with(dir.path(), &json!({}), true).unwrap();
    assert_eq!(
        fs::read_to_string(&path).unwrap(),
        format!("# Generated by `jp notes tools sync`.\n{SYNCED_NOTES}")
    );
}

#[test]
fn sync_reports_a_synced_file_it_cannot_read() {
    let dir = Utf8TempDir::new().unwrap();
    let path = dir.path().join(SYNCED_FILE);
    fs::create_dir(&path).unwrap();

    let error = sync(dir.path(), &json!({})).unwrap_err();

    assert!(error.starts_with(&format!("{path}: ")), "{error}");
    assert!(path.is_dir());
}

#[test]
fn sync_needs_an_existing_directory() {
    let dir = Utf8TempDir::new().unwrap();
    let missing = dir.path().join("nowhere");

    assert_eq!(
        sync(&missing, &json!({})).unwrap_err(),
        format!("{missing} is not a directory.")
    );
}

#[test]
fn a_tool_named_by_the_plugin_alone_is_known_by_its_key() {
    let config = json!({ "conversation": { "tools": {
        "list": { "source": "plugin.command.notes" },
        "notes_show": { "source": "plugin.command.notes.show" },
        "notebook": { "source": "plugin.command.notebook.list" },
    }}});

    assert_eq!(NOTES.served_tools(&config), [
        ("list".to_owned(), "list".to_owned()),
        ("notes_show".to_owned(), "show".to_owned()),
    ]);
}

#[test]
fn the_host_sends_a_declared_parameter_with_its_type() {
    let partial: jp_config::PartialAppConfig = toml::from_str(
        r#"
        [conversation.tools.notes_list]
        source = "plugin.command.notes.list"

        [conversation.tools.notes_list.parameters.category]
        type = "string"

        [conversation.tools.notes_create]
        source = "plugin.command.notes.create"

        [conversation.tools.notes_create.parameters.category]
        enum = ["design"]
        "#,
    )
    .unwrap();
    let config = serde_json::to_value(&partial).unwrap();

    assert!(declares(&config, "notes_list", "category"), "{config:#}");
    assert!(!declares(&config, "notes_create", "category"), "{config:#}");
}

#[test]
fn keys_and_values_are_written_as_toml() {
    assert_eq!(toml_key("ticket_create"), "ticket_create");
    assert_eq!(toml_key("my-tool"), "my-tool");
    assert_eq!(toml_key("my tool"), "\"my tool\"");
    assert_eq!(toml_key("a.b"), "\"a.b\"");
    assert_eq!(
        toml_string("say \"hi\" \\ there"),
        r#""say \"hi\" \\ there""#
    );
    assert_eq!(toml_string("tab\there"), r#""tab\u0009here""#);
}

#[test]
fn the_synced_file_parses_with_a_quoted_key() {
    let dir = Utf8TempDir::new().unwrap();
    let config = json!({ "conversation": { "tools": {
        "my tool": categorized("plugin.command.notes.list"),
    }}});

    sync(dir.path(), &config).unwrap();

    let source = fs::read_to_string(dir.path().join(SYNCED_FILE)).unwrap();
    let value: toml::Value = toml::from_str(&source).unwrap();
    assert_eq!(
        value["conversation"]["tools"]["my tool"]["parameters"]["category"]["enum"]
            .as_array()
            .map(Vec::len),
        Some(2)
    );
}
