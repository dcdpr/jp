use std::{fs, process::Command};

use camino_tempfile::Utf8TempDir;
use indoc::indoc;
use jp_plugin::message::OutputFormat;
use serde_json::{Map, Value, json};

use super::*;
use crate::{
    access::Access,
    options::{Options, Paths},
};

/// A page with every field, under `W-0000001`, holding `body`.
fn page(id: &str, title: &str, parent: Option<&str>, body: &str) -> String {
    let parent = parent.map_or(String::new(), |parent| format!("- **Parent**: {parent}\n"));
    format!(
        "# {title}\n\n- **Status**: Reference\n- **Authors**: jp\n- **Date**: \
         2026-10-30\n{parent}- **Label**: domain=llm\n- **Summary**: {id}.\n\n{body}\n"
    )
}

/// A workspace holding `pages`, each a file name below `docs/wiki` and its
/// text.
fn workspace(pages: &[(&str, String)]) -> Utf8TempDir {
    let root = Utf8TempDir::new().unwrap();
    let wiki = root.path().join("docs/wiki");
    fs::create_dir_all(&wiki).unwrap();
    fs::write(wiki.join(".epoch"), "2026-08-10\n").unwrap();
    for (name, text) in pages {
        fs::write(wiki.join(name), text).unwrap();
    }

    root
}

fn options() -> Options {
    let Value::Object(map) = json!({
        "labels": { "domain": { "description": "The subsystem", "values": ["llm", "storage"] } }
    }) else {
        unreachable!("a table");
    };

    Options::read(&map).unwrap()
}

fn with_context<T>(root: &Utf8Path, run: impl FnOnce(&Context<'_>) -> T) -> T {
    with_config(root, &Value::Object(Map::new()), run)
}

fn with_config<T>(root: &Utf8Path, config: &Value, run: impl FnOnce(&Context<'_>) -> T) -> T {
    let options = options();
    let ctx = Context {
        paths: Paths::new(root, config, &options).unwrap(),
        config,
        options: &options,
        format: OutputFormat::default(),
        access: Access::Unrestricted,
    };

    run(&ctx)
}

fn id(text: &str) -> PageId {
    text.parse().unwrap()
}

fn read(root: &Utf8Path, name: &str) -> String {
    fs::read_to_string(root.join("docs/wiki").join(name)).unwrap()
}

#[test]
fn a_new_title_moves_the_file_and_the_links_to_it() {
    let root = workspace(&[
        (
            "0000001-a.md",
            page("A", "A", None, "See [B](0000002-b.md#turn)."),
        ),
        (
            "0000002-b.md",
            page("B", "B", Some("W-0000001"), "## Turn\n\nOne cycle."),
        ),
    ]);

    let output = with_context(root.path(), |ctx| {
        edit(ctx, id("W-0000002"), &Changes {
            title: Some("Beta".to_owned()),
            ..Changes::default()
        })
    })
    .unwrap();

    assert_eq!(
        output.text,
        "Edited docs/wiki/0000002-beta.md\n  updated links in docs/wiki/0000001-a.md\n"
    );
    assert!(read(root.path(), "0000002-beta.md").starts_with("# Beta\n"));
    assert!(read(root.path(), "0000001-a.md").contains("See [B](0000002-beta.md#turn)."));
    assert!(!root.path().join("docs/wiki/0000002-b.md").exists());
}

#[test]
fn labels_are_added_and_removed() {
    let root = workspace(&[("0000001-a.md", page("A", "A", None, "Body."))]);

    with_context(root.path(), |ctx| {
        edit(ctx, id("W-0000001"), &Changes {
            add_labels: vec!["domain=storage".to_owned()],
            remove_labels: vec!["domain=llm".to_owned()],
            ..Changes::default()
        })
    })
    .unwrap();

    let source = read(root.path(), "0000001-a.md");
    assert!(
        source.contains("- **Date**: 2026-10-30\n- **Label**: domain=storage\n- **Summary**: A."),
        "{source}"
    );
}

#[test]
fn a_section_is_replaced_in_place() {
    let root = workspace(&[(
        "0000001-a.md",
        page(
            "A",
            "A",
            None,
            "## Turn\n\nOne cycle.\n\n## Thread\n\nTurns.",
        ),
    )]);

    with_context(root.path(), |ctx| {
        edit(ctx, id("W-0000001"), &Changes {
            body: Some("Two cycles.".to_owned()),
            section: Some("Turn".to_owned()),
            ..Changes::default()
        })
    })
    .unwrap();

    assert!(
        read(root.path(), "0000001-a.md")
            .ends_with("## Turn\n\nTwo cycles.\n\n## Thread\n\nTurns.\n")
    );
}

/// An edit is checked for what it brings, and a refusal leaves the page as it
/// was.
#[test]
fn an_edit_that_breaks_a_link_is_refused() {
    let root = workspace(&[("0000001-a.md", page("A", "A", None, "Body."))]);
    let before = read(root.path(), "0000001-a.md");

    let error = with_context(root.path(), |ctx| {
        edit(ctx, id("W-0000001"), &Changes {
            body: Some("See [gone](0000009-gone.md).".to_owned()),
            ..Changes::default()
        })
    })
    .unwrap_err();

    assert_eq!(
        error,
        "W-0000001 is unchanged: Line 9: `0000009-gone.md`: there is no such file."
    );
    assert_eq!(read(root.path(), "0000001-a.md"), before);
}

/// A problem already in the page doesn't stand in the way of an unrelated edit.
#[test]
fn a_problem_already_there_doesnt_stop_an_edit() {
    let root = workspace(&[(
        "0000001-a.md",
        page("A", "A", None, "See [gone](0000009-gone.md)."),
    )]);

    with_context(root.path(), |ctx| {
        edit(ctx, id("W-0000001"), &Changes {
            summary: Some("Still broken.".to_owned()),
            ..Changes::default()
        })
    })
    .unwrap();

    assert!(read(root.path(), "0000001-a.md").contains("- **Summary**: Still broken.\n"));
}

#[test]
fn a_page_below_can_not_be_the_parent() {
    let root = workspace(&[
        ("0000001-a.md", page("A", "A", None, "Body.")),
        ("0000002-b.md", page("B", "B", Some("W-0000001"), "Body.")),
    ]);

    let error = with_context(root.path(), |ctx| {
        edit(ctx, id("W-0000001"), &Changes {
            parent: Some(Some(id("W-0000002"))),
            ..Changes::default()
        })
    })
    .unwrap_err();

    assert_eq!(
        error,
        "W-0000002 sits below W-0000001, so it can't be its parent."
    );
}

#[test]
fn archiving_by_status_refuses_a_page_with_active_children() {
    let root = workspace(&[
        ("0000001-a.md", page("A", "A", None, "Body.")),
        ("0000002-b.md", page("B", "B", Some("W-0000001"), "Body.")),
    ]);

    let error = with_context(root.path(), |ctx| {
        edit(ctx, id("W-0000001"), &Changes {
            status: Some(Status::Archived),
            ..Changes::default()
        })
    })
    .unwrap_err();

    assert!(
        error.contains("W-0000001 has active children: W-0000002."),
        "{error}"
    );
}

/// Run git in `dir`, failing the test when it does.
fn git(dir: &Utf8Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .args(["-c", "user.name=Tester", "-c", "user.email=t@example.com"])
        .args(["-c", "commit.gpgsign=false"])
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    String::from_utf8_lossy(&output.stdout).trim().to_owned()
}

/// A page becoming a Snapshot is pinned before it is checked, so the check sees
/// the permalink rather than refusing the relative link it replaces.
#[test]
fn a_page_becoming_a_snapshot_is_pinned_before_it_is_checked() {
    let root = workspace(&[(
        "0000001-a.md",
        page("A", "A", None, "See [`x`](../../crates/x.rs#L1)."),
    )]);
    fs::create_dir_all(root.path().join("crates")).unwrap();
    fs::write(root.path().join("crates/x.rs"), "fn x() {}\n").unwrap();
    git(root.path(), &["init", "--quiet", "--initial-branch=main"]);
    git(root.path(), &["add", "crates"]);
    git(root.path(), &["commit", "--quiet", "-m", "x"]);
    let commit = git(root.path(), &["rev-parse", "HEAD"]);
    git(root.path(), &[
        "update-ref",
        "refs/remotes/origin/main",
        "HEAD",
    ]);
    git(root.path(), &[
        "symbolic-ref",
        "refs/remotes/origin/HEAD",
        "refs/remotes/origin/main",
    ]);
    let config = json!({
        "plugins": { "command": { "vite": { "options": { "repository": "https://github.com/o/r" } } } }
    });

    let output = with_config(root.path(), &config, |ctx| {
        edit(ctx, id("W-0000001"), &Changes {
            status: Some(Status::Snapshot),
            ..Changes::default()
        })
    })
    .unwrap();

    let permalink = format!("https://github.com/o/r/blob/{commit}/crates/x.rs#L1");
    assert_eq!(
        output.text,
        format!(
            "Edited docs/wiki/0000001-a.md\n  pinned ../../crates/x.rs#L1\n      to {permalink}\n"
        )
    );
    let source = read(root.path(), "0000001-a.md");
    assert!(source.contains("- **Status**: Snapshot\n"), "{source}");
    assert!(
        source.contains(&format!("See [`x`]({permalink}).")),
        "{source}"
    );
}

/// A link that can't be pinned leaves a Snapshot citing source by a relative
/// link, which the check refuses; the refusal says why the pin couldn't.
#[test]
fn a_snapshot_that_cant_be_pinned_is_refused_with_the_reason() {
    let root = workspace(&[(
        "0000001-a.md",
        page("A", "A", None, "See [`x`](../../crates/x.rs#L1)."),
    )]);
    fs::create_dir_all(root.path().join("crates")).unwrap();
    fs::write(root.path().join("crates/x.rs"), "fn x() {}\n").unwrap();
    let before = read(root.path(), "0000001-a.md");

    let error = with_context(root.path(), |ctx| {
        edit(ctx, id("W-0000001"), &Changes {
            status: Some(Status::Snapshot),
            ..Changes::default()
        })
    })
    .unwrap_err();

    assert_eq!(
        error,
        "W-0000001 is unchanged: Line 9: `../../crates/x.rs#L1` cites source by a relative link, \
         which follows the work tree; `jp wiki pin W-0000001` turns it into a permalink. \
         `../../crates/x.rs#L1` couldn't be pinned: there is no repository address to pin to: set \
         `plugins.command.vite.options.repository`, or give git an `origin` remote."
    );
    assert_eq!(read(root.path(), "0000001-a.md"), before);
}

/// An editor that writes each of `edits` in turn, and answers each question
/// with `again`.
struct Scripted {
    edits: Vec<String>,
    again: bool,
    asked: Vec<String>,
}

impl Editing for Scripted {
    fn open(&mut self, path: &Utf8Path) -> Result<bool, String> {
        let edit = self.edits.remove(0);
        fs::write(path, edit).unwrap();
        Ok(true)
    }

    fn again(&mut self, problem: &str) -> bool {
        self.asked.push(problem.to_owned());
        self.again
    }
}

#[test]
fn the_editor_offers_a_broken_edit_back_and_keeps_the_fixed_one() {
    let root = workspace(&[("0000001-a.md", page("A", "A", None, "Body."))]);
    let mut editing = Scripted {
        edits: vec![
            page("A", "A", None, "See [gone](0000009-gone.md)."),
            page("A", "A", None, "Fixed."),
        ],
        again: true,
        asked: vec![],
    };

    let output = with_context(root.path(), |ctx| {
        in_editor(ctx, id("W-0000001"), &mut editing)
    })
    .unwrap();

    assert_eq!(output.text, "Edited docs/wiki/0000001-a.md\n");
    assert_eq!(editing.asked, [
        "W-0000001 can't be saved: Line 9: `0000009-gone.md`: there is no such file."
    ]);
    assert!(read(root.path(), "0000001-a.md").ends_with("Fixed.\n"));
}

#[test]
fn the_editor_undoes_an_edit_given_up_on() {
    let original = page("A", "A", None, "Body.");
    let root = workspace(&[("0000001-a.md", original.clone())]);
    let mut editing = Scripted {
        edits: vec![page("A", "A", None, "See [gone](0000009-gone.md).")],
        again: false,
        asked: vec![],
    };

    let error = with_context(root.path(), |ctx| {
        in_editor(ctx, id("W-0000001"), &mut editing)
    })
    .unwrap_err();

    assert!(error.starts_with("W-0000001 is unchanged:"), "{error}");
    assert_eq!(read(root.path(), "0000001-a.md"), original);
}

#[test]
fn removing_a_bare_key_removes_every_label_under_it() {
    let current = [
        "domain=llm".to_owned(),
        "domain=storage".to_owned(),
        "x".to_owned(),
    ];

    assert_eq!(
        relabel(&current, &["y=1".to_owned()], &["domain".to_owned()]),
        ["x", "y=1"]
    );
    assert_eq!(
        relabel(&current, &["DOMAIN=llm".to_owned()], &[
            "domain=STORAGE".to_owned()
        ]),
        ["domain=llm", "x"]
    );
}

#[test]
fn a_new_body_replaces_everything_below_the_metadata() {
    let text = indoc! {"
        # A

        - **Status**: Reference

        Old body.

        [x]: y
    "};

    assert_eq!(
        replace_body(text, "New body."),
        "# A\n\n- **Status**: Reference\n\nNew body.\n"
    );
}
