use std::{fs, process::Command};

use camino::Utf8Path;

use super::*;
use crate::{
    cmd::{
        draft::{self, Draft},
        tests::Workspace,
    },
    tests::jp_root,
};

fn categories(list: &[&str]) -> Vec<String> {
    list.iter().map(|category| (*category).to_owned()).collect()
}

fn empty() -> Workspace {
    let workspace = Workspace::new();
    fs::remove_dir_all(&workspace.paths.rfds).unwrap();

    workspace
}

fn git(dir: &Utf8Path, args: &[&str]) {
    let output = Command::new("git")
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

fn commit(workspace: &Workspace) {
    let root = &workspace.paths.root;
    if !root.join(".git").exists() {
        git(root, &["init", "--quiet", "--initial-branch=main"]);
        git(root, &["config", "user.name", "Tester"]);
        git(root, &["config", "user.email", "tester@example.com"]);
        git(root, &["config", "commit.gpgsign", "false"]);
    }
    git(root, &["add", "-A"]);
    git(root, &["commit", "--quiet", "-m", "Commit"]);
}

#[test]
fn init_sets_up_the_rfd_directory() {
    let workspace = empty();

    let output = init(
        &workspace.paths,
        &categories(&["design", "decision", "guide", "process"]),
        false,
    )
    .unwrap();

    assert_eq!(
        output.text,
        "Created docs/rfd/\nCreated docs/rfd/drafts/\nWrote \
         docs/rfd/000-design-template.md\nWrote docs/rfd/000-decision-template.md\nWrote \
         docs/rfd/000-guide-template.md\nWrote docs/rfd/000-process-template.md\nWrote \
         docs/rfd/.priority.json\n"
    );
    assert_eq!(
        workspace.read("docs/rfd/.priority.json"),
        Priority::default().render()
    );
    assert!(!workspace.exists("docs/rfd/001-jp-rfd-process.md"));
}

#[test]
fn the_default_templates_are_the_ones_jp_uses() {
    let jp = jp_root().join("docs/rfd");

    for (category, template) in TEMPLATES {
        let ours = fs::read_to_string(jp.join(format!("000-{category}-template.md"))).unwrap();

        assert_eq!(template, ours, "{category}");
    }
}

#[test]
fn a_category_of_the_projects_own_gets_a_template_naming_it() {
    let workspace = empty();

    init(&workspace.paths, &categories(&["post-mortem"]), false).unwrap();

    let template = workspace.read("docs/rfd/000-post-mortem-template.md");
    assert!(template.contains("# RFD NNN: TITLE\n"), "{template}");
    assert!(
        template.contains("- **Category**: Post-mortem\n"),
        "{template}"
    );
}

#[test]
fn init_adds_what_is_missing() {
    let workspace = empty();
    init(&workspace.paths, &categories(&["design"]), false).unwrap();

    let output = init(&workspace.paths, &categories(&["design", "guide"]), false).unwrap();

    assert_eq!(output.text, "Wrote docs/rfd/000-guide-template.md\n");

    let output = init(&workspace.paths, &categories(&["design", "guide"]), false).unwrap();

    assert_eq!(
        output.text,
        "docs/rfd/ already has its drafts directory, its templates, and its priority board.\n"
    );
}

#[test]
fn init_overwrites_an_edited_template_outside_git_only_when_forced() {
    let workspace = empty();
    init(&workspace.paths, &categories(&["design"]), false).unwrap();
    workspace.write("docs/rfd/000-design-template.md", "Our own.\n");

    let error = init(&workspace.paths, &categories(&["design", "guide"]), false).unwrap_err();

    assert_eq!(
        error,
        "Git couldn't restore these files, so `jp rfd init` leaves them:\n  \
         docs/rfd/000-design-template.md (not in a git work tree)\nRun `jp rfd init --force` to \
         overwrite them anyway."
    );
    assert_eq!(
        workspace.read("docs/rfd/000-design-template.md"),
        "Our own.\n"
    );
    assert!(!workspace.exists("docs/rfd/000-guide-template.md"));

    let output = init(&workspace.paths, &categories(&["design", "guide"]), true).unwrap();

    assert_eq!(
        output.text,
        "Overwrote docs/rfd/000-design-template.md\nWrote docs/rfd/000-guide-template.md\n"
    );
    assert_eq!(
        workspace.read("docs/rfd/000-design-template.md"),
        TEMPLATES[0].1
    );
}

#[test]
fn init_overwrites_a_template_git_can_restore() {
    let workspace = empty();
    init(&workspace.paths, &categories(&["design", "guide"]), false).unwrap();
    workspace.write("docs/rfd/000-design-template.md", "Our own.\n");
    workspace.board("{ \"ranked\": true }\n");
    commit(&workspace);
    workspace.write("docs/rfd/000-guide-template.md", "Edited since.\n");

    let error = init(&workspace.paths, &categories(&["design", "guide"]), false).unwrap_err();

    assert_eq!(
        error,
        "Git couldn't restore these files, so `jp rfd init` leaves them:\n  \
         docs/rfd/000-guide-template.md (changed since the last commit)\nCommit them, or run `jp \
         rfd init --force` to overwrite them anyway."
    );

    let output = init(&workspace.paths, &categories(&["design"]), false).unwrap();

    assert_eq!(output.text, "Overwrote docs/rfd/000-design-template.md\n");
    assert_eq!(
        workspace.read("docs/rfd/000-design-template.md"),
        TEMPLATES[0].1
    );
    assert_eq!(
        workspace.read("docs/rfd/.priority.json"),
        "{ \"ranked\": true }\n"
    );
}

#[test]
fn a_draft_can_be_written_right_after_init() {
    let workspace = empty();
    let categories = categories(&["design", "post-mortem"]);
    init(&workspace.paths, &categories, false).unwrap();

    for (category, title) in [("design", "First Idea"), ("post-mortem", "The Outage")] {
        draft::draft(&workspace.paths, &categories, &Draft {
            category,
            title,
            author: "Jane Doe <jane@example.com>",
            date: "2026-10-15",
        })
        .unwrap();
    }

    let outage = workspace.read("docs/rfd/drafts/D02-the-outage.md");
    assert!(outage.contains("# RFD D02: The Outage\n"), "{outage}");
    assert!(outage.contains("- **Category**: Post-mortem\n"), "{outage}");
    assert!(
        outage.contains("- **Authors**: Jane Doe <jane@example.com>\n- **Date**: 2026-10-15\n"),
        "{outage}"
    );
}
