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
        &serde_json::json!({
          "user": {
            "name": "tester"
          }
        }),
    )
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
        config: serde_json::json!({
          "user": {
            "name": "tester"
          }
        }),
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
        } => {
            assert_eq!(kind, Some(Kind::Bug));
            assert_eq!(title.as_deref(), Some("Tool call header misaligned"));
            assert_eq!(author, None);
            assert_eq!(body, None);
            assert_eq!(implements, None);
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
            Command::Show { id, json } => {
                assert_eq!(id, Some("T-02wt0kx".parse().unwrap()), "{args:?}");
                assert!(!json);
            }
            other => panic!("expected show for {args:?}, got {other:?}"),
        }
    }

    match parse(&["T-02wt0kx", "--json"]).unwrap().command {
        Command::Show { json, .. } => assert!(json),
        other => panic!("expected show, got {other:?}"),
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
        "add", "comment", "close", "show", "promote", "import", "list",
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
        author: Some("John Doe".to_owned()),
        body: Some("The header renders one column left of the body.".to_owned()),
        implements: None,
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
        kind: None,
        json: false,
    })
    .unwrap();
    assert_eq!(
        listed.text,
        format!("{id} Todo         Bug      Tool call header misaligned\n")
    );

    let closed = run_command(&dir, Command::Close { id: Some(id) }).unwrap();
    assert!(closed.text.contains("Todo -> Done"), "{}", closed.text);

    let json = run_command(&dir, Command::List {
        status: Some(Status::Done),
        kind: None,
        json: true,
    })
    .unwrap();
    let rows: Vec<serde_json::Value> = serde_json::from_str(&json.text).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["id"], id.to_string());
    assert_eq!(rows[0]["status"], "Done");
    assert_eq!(rows[0]["comments"], 1);
}

/// A branch cut before the id change carries tickets the new parser skips
/// silently, so migration has to find them by filename and fix what names them.
#[test]
fn migrate_converts_legacy_tickets_and_the_board() {
    let dir = Utf8TempDir::new().unwrap();
    std::fs::write(
        dir.path().join("0005-old-ticket.md"),
        "# T0005: Old ticket\n\n- **Status**: Todo\n- **Kind**: Bug\n- **Authors**: john\n- \
         **Date**: 2026-08-05\n\nBody.\n",
    )
    .unwrap();
    std::fs::write(
        dir.path().join(".board.json"),
        "{\"todo\":[\"T0005\"],\"in_progress\":[],\"done\":[]}\n",
    )
    .unwrap();

    let output = run_command(&dir, Command::Migrate).unwrap();

    let id = store::list(dir.path()).unwrap()[0].id;
    assert!(!dir.path().join("0005-old-ticket.md").exists());
    assert!(
        dir.path()
            .join(format!("{}old-ticket.md", id.file_prefix()))
            .exists()
    );

    // The legacy heading carried the id too; it lives only in the filename now.
    let converted = dir
        .path()
        .join(format!("{}old-ticket.md", id.file_prefix()));
    let source = std::fs::read_to_string(&converted).unwrap();
    assert!(source.starts_with("# Old ticket\n"), "{source}");

    let board = std::fs::read_to_string(dir.path().join(".board.json")).unwrap();
    assert_eq!(
        board,
        format!("{{\"todo\":[\"{id}\"],\"in_progress\":[],\"done\":[]}}\n")
    );

    assert!(
        output.text.starts_with(&format!("T0005 -> {id} at ")),
        "{}",
        output.text
    );
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
        "# Winner\n\n- **Status**: Todo\n- **Kind**: Bug\n- **Authors**: john\n- **Date**: \
         2026-08-14\n\nBody.\n",
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
        "# Loser\n\n- **Status**: Todo\n- **Kind**: Bug\n- **Authors**: jane\n- **Date**: \
         2026-08-14\n\nOther body.\n",
    )
    .unwrap();
    std::fs::write(root.join("BRANCH.md"), "Fixes T-02wt0kx on this branch.\n").unwrap();
    commit(root, "Add the losing ticket", Some("2026-08-14T12:00:00Z"));

    let output = execute(
        &dir,
        Command::Refresh {
            path: losing.clone(),
            base: "main".to_owned(),
        },
        &serde_json::json!({}),
    )
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
        "# Loser\n\n- **Status**: Todo\n- **Kind**: Bug\n- **Authors**: jane\n- **Date**: \
         2026-08-14\n\nBody.\n\n## Comments\n\n-----\n\n- **From**: john\n- **Date**: \
         2026-08-14T10:00:00Z\n\nFirst.\n\n-----\n\n- **From**: jp\n- **Date**: \
         2026-08-14T11:00:00Z\n- **Re**: #1\n\nSecond.\n",
    )
    .unwrap();
    let before = std::fs::read_to_string(&losing).unwrap();
    commit(root, "Add the losing ticket", None);

    let output = execute(
        &dir,
        Command::Refresh {
            path: losing,
            base: "main".to_owned(),
        },
        &serde_json::json!({}),
    )
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
        "# Loser\n\n- **Status**: Todo\n- **Kind**: Bug\n- **Authors**: jane\n- **Date**: \
         2026-08-14\n\nProbably a duplicate of T-02wt0kx.\n",
    )
    .unwrap();
    commit(root, "Add the losing ticket", None);

    let output = execute(
        &dir,
        Command::Refresh {
            path: losing,
            base: "main".to_owned(),
        },
        &serde_json::json!({}),
    )
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
        "# Filed earlier\n\n- **Status**: Todo\n- **Kind**: Bug\n- **Authors**: john\n- **Date**: \
         2026-08-14\n\nBody.\n",
    )
    .unwrap();
    // 2026-08-14T00:00:00Z is 345,600 seconds past the epoch: bucket 69,120.
    commit(root, "File it", Some("2026-08-14T00:00:00Z"));

    execute(
        &dir,
        Command::Refresh {
            path: Utf8PathBuf::from("docs/ticket/02wt0kx-filed-earlier.md"),
            base: "main".to_owned(),
        },
        &serde_json::json!({}),
    )
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
        "# Loser\n\n- **Status**: Todo\n- **Kind**: Bug\n- **Authors**: jane\n- **Date**: \
         2026-08-14\n\nBody.\n",
    )
    .unwrap();
    // The branch touches the file for an unrelated reason.
    std::fs::write(
        root.join("SHARED.md"),
        "Tracked in T-02wt0kx.\nAlso worth reading.\n",
    )
    .unwrap();
    commit(root, "Add the losing ticket", None);

    let output = execute(
        &dir,
        Command::Refresh {
            path: losing,
            base: "main".to_owned(),
        },
        &serde_json::json!({}),
    )
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
        "# Filed earlier\n\n- **Status**: Todo\n- **Kind**: Bug\n- **Authors**: john\n- **Date**: \
         2026-08-14\n\nBody.\n",
    )
    .unwrap();
    // 2026-08-14T00:00:00Z is 345,600 seconds past the epoch: bucket 69,120.
    commit(root, "File it", Some("2026-08-14T00:00:00Z"));

    execute(
        &dir,
        Command::Refresh {
            path,
            base: "main".to_owned(),
        },
        &serde_json::json!({}),
    )
    .unwrap();

    let fresh = store::list(&dir).unwrap()[0].id;
    assert_eq!(fresh.bucket(), 69_120);
}

/// The scan walks whatever is in the ticket directory, so a name whose fourth
/// byte falls mid-character must not take the command down.
#[test]
fn migrate_skips_a_non_ascii_filename_without_panicking() {
    let dir = Utf8TempDir::new().unwrap();
    std::fs::write(dir.path().join("a\u{e9}\u{e9}.md"), "not a ticket\n").unwrap();

    let output = run_command(&dir, Command::Migrate).unwrap();

    assert_eq!(output.text, "No tickets to migrate.\n");
}

/// `12345-foo.md` is not a pre-RFD-102 ticket: the id is five digits, so the
/// fifth byte is not the separator.
#[test]
fn migrate_ignores_a_filename_that_only_starts_with_digits() {
    let dir = Utf8TempDir::new().unwrap();
    std::fs::write(dir.path().join("12345-foo.md"), "not a ticket\n").unwrap();

    let output = run_command(&dir, Command::Migrate).unwrap();

    assert_eq!(output.text, "No tickets to migrate.\n");
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

/// A legacy reply named the ticket it lives on.
/// The cross-file rewrite would turn that into `T-<new>#1` and leave the
/// migrated ticket naming itself, so the conversion has to reach the `Re` field
/// first.
#[test]
fn migrate_converts_a_legacy_reply_to_the_positional_form() {
    let dir = Utf8TempDir::new().unwrap();
    std::fs::write(
        dir.path().join("0005-old-ticket.md"),
        "# T0005: Old ticket\n\n- **Status**: Todo\n- **Kind**: Bug\n- **Authors**: john\n- \
         **Date**: 2026-08-05\n\nBody.\n\n## Comments\n\n-----\n\n- **From**: john\n- **Date**: \
         2026-08-05T10:00:00Z\n\nFirst.\n\n-----\n\n- **From**: jp\n- **Date**: \
         2026-08-05T11:00:00Z\n- **Re**: T0005#1\n\nSecond.\n",
    )
    .unwrap();

    run_command(&dir, Command::Migrate).unwrap();

    let entry = store::list(dir.path()).unwrap().pop().unwrap();
    let ticket = entry.ticket.unwrap();
    assert_eq!(ticket.title, "Old ticket");
    assert_eq!(ticket.comments[1].re.as_deref(), Some("#1"));

    // The whole point: nothing inside the file names the ticket any more.
    let source = std::fs::read_to_string(&entry.path).unwrap();
    assert!(!source.contains(&entry.id.to_string()), "{source}");
    assert!(!source.contains("T0005"), "{source}");
}

/// `just ticket-promote` reads `.id` out of this, so the field has to survive
/// the document losing its own.
#[test]
fn show_json_carries_the_id() {
    let dir = Utf8TempDir::new().unwrap();
    run_command(&dir, Command::Add {
        kind: Some(Kind::Bug),
        title: Some("Tool call header misaligned".to_owned()),
        author: Some("john".to_owned()),
        body: Some("The header renders one column left.".to_owned()),
        implements: None,
    })
    .unwrap();
    let id = store::list(dir.path()).unwrap()[0].id;

    let out = run_command(&dir, Command::Show {
        id: Some(id),
        json: true,
    })
    .unwrap();

    let detail: serde_json::Value = serde_json::from_str(&out.text).unwrap();
    assert_eq!(detail["id"], id.to_string());
    assert_eq!(detail["title"], "Tool call header misaligned");
    assert_eq!(detail["description"], "The header renders one column left.");
}

#[test]
fn migrate_of_a_converted_directory_does_nothing() {
    let dir = Utf8TempDir::new().unwrap();
    run_command(&dir, Command::Add {
        kind: Some(Kind::Chore),
        title: Some("Already current".to_owned()),
        author: Some("john".to_owned()),
        body: None,
        implements: None,
    })
    .unwrap();

    let output = run_command(&dir, Command::Migrate).unwrap();

    assert_eq!(output.text, "No tickets to migrate.\n");
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
            PluginToHost::Ready,
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

#[test]
fn a_bad_argument_exits_non_zero_with_a_reason() {
    let dir = Utf8TempDir::new().unwrap();
    let messages = exchange(&init(dir.path(), &["close", "not-an-id"]));

    match messages.as_slice() {
        [PluginToHost::Ready, PluginToHost::Exit(exit)] => {
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
        author: Some("john".to_owned()),
        body: None,
        implements: None,
    })
    .unwrap();
    std::fs::write(dir.path().join("zzzzzzz-mangled.md"), "no heading here\n").unwrap();

    let listed = run_command(&dir, Command::List {
        status: None,
        kind: None,
        json: true,
    })
    .unwrap();

    let rows: Vec<serde_json::Value> = serde_json::from_str(&listed.text).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(listed.warnings.len(), 1);
    assert!(listed.warnings[0].contains("zzzzzzz-mangled.md"));
}
