use std::fs;

use camino::Utf8PathBuf;

use super::*;
use crate::cmd::{
    draft::{self, Draft},
    tests::Workspace,
};

fn categories(list: &[&str]) -> Vec<String> {
    list.iter().map(|category| (*category).to_owned()).collect()
}

fn empty() -> Workspace {
    let workspace = Workspace::new();
    fs::remove_dir_all(&workspace.paths.rfds).unwrap();

    workspace
}

#[test]
fn init_sets_up_the_rfd_directory() {
    let workspace = empty();

    let output = init(
        &workspace.paths,
        &categories(&["design", "decision", "guide", "process"]),
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

/// The templates a new project starts with are JP's own.
#[test]
fn the_default_templates_are_the_ones_jp_uses() {
    let jp = Utf8PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../../docs/rfd");

    for (category, template) in TEMPLATES {
        let ours = fs::read_to_string(jp.join(format!("000-{category}-template.md"))).unwrap();

        assert_eq!(template, ours, "{category}");
    }
}

#[test]
fn a_category_of_the_projects_own_gets_a_template_naming_it() {
    let workspace = empty();

    init(&workspace.paths, &categories(&["post-mortem"])).unwrap();

    let template = workspace.read("docs/rfd/000-post-mortem-template.md");
    assert!(template.contains("# RFD NNN: TITLE\n"), "{template}");
    assert!(
        template.contains("- **Category**: Post-mortem\n"),
        "{template}"
    );
}

/// Running it again adds what is missing, such as the template for a category
/// added since, and leaves what is there alone.
#[test]
fn init_only_adds_what_is_missing() {
    let workspace = empty();
    init(&workspace.paths, &categories(&["design"])).unwrap();
    workspace.write("docs/rfd/000-design-template.md", "Our own.\n");

    let output = init(&workspace.paths, &categories(&["design", "guide"])).unwrap();

    assert_eq!(output.text, "Wrote docs/rfd/000-guide-template.md\n");
    assert_eq!(
        workspace.read("docs/rfd/000-design-template.md"),
        "Our own.\n"
    );

    let output = init(&workspace.paths, &categories(&["design", "guide"])).unwrap();

    assert_eq!(
        output.text,
        "docs/rfd/ already has its drafts directory, its templates, and its priority board.\n"
    );
}

/// What `init` writes is what `draft` reads.
#[test]
fn a_draft_can_be_written_right_after_init() {
    let workspace = empty();
    let categories = categories(&["design", "post-mortem"]);
    init(&workspace.paths, &categories).unwrap();

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
