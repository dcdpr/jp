use std::{env, fs};

use camino_tempfile::Utf8TempDir;
use doc::epoch::EPOCH_FILE;
use indoc::indoc;
use jp_tool::FsRule;
use serde_json::{Value, json};

use super::*;

/// A workspace with an epoch, an RFD of each status that matters, a ticket, a
/// source file, and `pages`, each a file name below `docs/wiki` and its text.
fn workspace(pages: &[(&str, &str)]) -> Utf8TempDir {
    let root = Utf8TempDir::new().unwrap();
    for (path, text) in [
        ("docs/wiki/.epoch", "2026-08-10\n"),
        (
            "docs/rfd/100-tickets.md",
            "# RFD 100: Tickets\n\n- **Status**: Implemented\n\n## Labels\n",
        ),
        (
            "docs/rfd/033-old.md",
            "# RFD 033: Old\n\n- **Status**: Superseded\n",
        ),
        (
            "docs/ticket/02wt0kx-a-ticket.md",
            "# A ticket\n\n- **Status**: Todo\n\nSee W-0000001.\n",
        ),
        (
            "crates/x/src/lib.rs",
            "fn one() {}\nfn by_rfd() {}\nfn three() {}\n",
        ),
    ] {
        write(root.path(), path, text);
    }
    for (name, text) in pages {
        write(root.path(), &format!("docs/wiki/{name}"), text);
    }

    root
}

fn write(root: &Utf8Path, path: &str, text: &str) {
    let path = root.join(path);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, text).unwrap();
}

/// Check the workspace at `root`, as `jp wiki check` would.
fn run(root: &Utf8Path) -> Report {
    run_under(&Access::Unrestricted, root)
}

/// Check the workspace at `root` with what `access` lets the check read.
fn run_under(access: &Access, root: &Utf8Path) -> Report {
    let options = Options::default();
    let paths = Paths::new(root, &json!({}), &options).unwrap();
    let wiki = Wiki::load(&paths.wiki, access).unwrap();
    let library = Library::load(&paths, access);

    check(&Context {
        paths: &paths,
        options: &options,
        wiki: &wiki,
        library: &library,
        repository: None,
        access,
    })
}

fn messages(findings: &[Finding]) -> Vec<String> {
    findings
        .iter()
        .map(|finding| match finding.line {
            Some(line) => format!("{}:{line}: {}", finding.path, finding.message),
            None => format!("{}: {}", finding.path, finding.message),
        })
        .collect()
}

/// The root of JP's own repository, whose wiki is checked like any other.
///
/// Read when the test runs rather than with `env!`: a target directory shared
/// between worktrees can run a binary another worktree compiled, whose baked-in
/// path names that worktree.
/// Normalized, since the check compares it with the paths links resolve to.
fn jp_root() -> Utf8PathBuf {
    let manifest = env::var("CARGO_MANIFEST_DIR")
        .expect("CARGO_MANIFEST_DIR: run tests via `cargo test` or `cargo nextest`");

    links::normalize(&Utf8PathBuf::from(manifest).join("../../../.."))
}

/// JP's own wiki passes `jp wiki check`.
///
/// Its pages link to RFDs, tickets, each other, and source, so a change to any
/// of them can break one.
/// This stands in for CI running `jp wiki check` itself, which T-1bxk579
/// tracks; remove it once that runs.
#[test]
fn jp_wiki_passes_check() {
    let root = jp_root();
    let path = root.join(".jp/config.toml");
    let config: toml::Value = toml::from_str(&fs::read_to_string(&path).unwrap())
        .unwrap_or_else(|error| panic!("{path}: {error}"));
    let config = serde_json::to_value(config).unwrap();
    let options = match config.pointer("/plugins/command/wiki/options") {
        Some(Value::Object(options)) => Options::read(options).unwrap(),
        _ => Options::default(),
    };
    let paths = Paths::new(&root, &config, &options).unwrap();
    let access = Access::Unrestricted;
    let wiki = Wiki::load(&paths.wiki, &access).unwrap();
    let library = Library::load(&paths, &access);
    let repository = crate::options::repository(&root, &config).unwrap();

    let report = check(&Context {
        paths: &paths,
        options: &options,
        wiki: &wiki,
        library: &library,
        repository: repository.as_deref(),
        access: &access,
    });

    assert!(
        report.pages > 0,
        "JP's wiki at {} holds no pages",
        paths.wiki
    );
    assert!(
        report.errors.is_empty(),
        "`jp wiki check` fails on JP's wiki:\n{}",
        messages(&report.errors).join("\n")
    );
}

/// The root of the glossary, which the ticket links to, so it is no orphan.
const ROOT: &str = indoc! {"
    # Glossary

    - **Status**: Reference
    - **Authors**: jp
    - **Date**: 2026-10-30
    - **Summary**: The words the project uses.

    ## Turn

    See [the child](0000002-child.md#pinned), RFD 100#labels, and
    [`by_rfd`](../../crates/x/src/lib.rs#L2).
"};

const CHILD: &str = indoc! {"
    # Child

    - **Status**: Reference
    - **Authors**: jp
    - **Date**: 2026-10-30
    - **Parent**: W-0000001
    - **Summary**: A child page.

    ## Pinned

    ![Turn](0000001-glossary.md#turn)
"};

#[test]
fn a_well_formed_wiki_has_no_problems() {
    let root = workspace(&[("0000001-glossary.md", ROOT), ("0000002-child.md", CHILD)]);

    let report = run(root.path());

    assert_eq!(messages(&report.errors), Vec::<String>::new());
    assert_eq!(messages(&report.warnings), Vec::<String>::new());
    assert_eq!(report.pages, 2);
}

/// A check held to a policy reads only what it grants, and reports what it
/// couldn't see as unchecked, not as broken.
#[test]
fn what_the_policy_hides_is_unchecked_not_broken() {
    let root = workspace(&[("0000001-glossary.md", ROOT), ("0000002-child.md", CHILD)]);
    let access = Access::granting(root.path(), vec![
        FsRule::new("docs/wiki").with_read(true),
        FsRule::new("docs/wiki/0000002-child.md").with_read(false),
    ]);

    let report = run_under(&access, root.path());

    assert_eq!(messages(&report.errors), Vec::<String>::new());
    assert_eq!(messages(&report.unchecked), [
        "docs/wiki/0000001-glossary.md:10: `0000002-child.md#pinned` wasn't checked: access \
         denied: read on docs/wiki/0000002-child.md.",
        "docs/wiki/0000001-glossary.md:11: `../../crates/x/src/lib.rs#L2` wasn't checked: access \
         denied: read on crates/x/src/lib.rs.",
        "docs/wiki/0000001-glossary.md:10: RFD 100 wasn't checked: its directory can't be listed.",
    ]);
}

#[test]
fn the_format_is_held_to() {
    let page = indoc! {"
        - **Status**: Draft
        - **Date**: yesterday
        - **Summary**: Out of place.
        - **Authors**: jp
    "};
    let root = workspace(&[("0000003-bad.md", page)]);

    let errors = messages(&run(root.path()).errors);

    assert_eq!(errors, [
        "docs/wiki/0000003-bad.md:1: The page doesn't open with a `# Title` heading.",
        "docs/wiki/0000003-bad.md: `Draft` is not a status; use Reference, Snapshot, or Archived.",
        "docs/wiki/0000003-bad.md: `yesterday` is not a `YYYY-MM-DD` date.",
        "docs/wiki/0000003-bad.md: `Summary` is not the last field of the block.",
    ]);
}

#[test]
fn links_are_held_to_what_they_point_at() {
    let page = indoc! {"
        # Links

        - **Status**: Reference
        - **Authors**: jp
        - **Date**: 2026-10-30
        - **Parent**: W-0000001
        - **Summary**: Broken links.

        ## Here {#here}

        [Gone](0000009-gone.md), [nowhere](#nowhere), [heading](0000001-glossary.md#nope),
        [past the end](../../crates/x/src/lib.rs#L2-L9), [`three`](../../crates/x/src/lib.rs#L1),
        [`gone`](../../crates/x/src/lib.rs), W-0000009, and RFD 100#nope.
    "};
    let root = workspace(&[
        ("0000001-glossary.md", ROOT),
        ("0000002-child.md", CHILD),
        ("0000004-links.md", page),
    ]);

    let errors = messages(&run(root.path()).errors);

    assert_eq!(errors, [
        "docs/wiki/0000004-links.md:9: A heading carries a custom anchor; the site computes every \
         heading's anchor from its text.",
        "docs/wiki/0000004-links.md:11: `0000009-gone.md`: there is no such file.",
        "docs/wiki/0000004-links.md:11: `#nowhere`: `#nowhere` names no heading there.",
        "docs/wiki/0000004-links.md:11: `0000001-glossary.md#nope`: `#nope` names no heading \
         there.",
        "docs/wiki/0000004-links.md:12: `../../crates/x/src/lib.rs#L2-L9`: lines 2 to 9 are past \
         the file, which has 3.",
        "docs/wiki/0000004-links.md:12: `../../crates/x/src/lib.rs#L1`: `three` is not on lines 1 \
         to 1; it is on line 3 now.",
        "docs/wiki/0000004-links.md:13: `../../crates/x/src/lib.rs`: `gone` is not in the file.",
        "docs/wiki/0000004-links.md:11: W-0000009 names no document.",
        "docs/wiki/0000004-links.md:11: `RFD 100#nope`: `#nope` names no heading there.",
    ]);
}

#[test]
fn a_snapshot_cites_source_by_permalink_and_doesnt_embed() {
    let page = indoc! {"
        # Study

        - **Status**: Snapshot
        - **Authors**: jp
        - **Date**: 2026-10-30
        - **Parent**: W-0000001
        - **Summary**: A study.

        See [the code](../../crates/x/src/lib.rs#L2).

        ![Turn](0000001-glossary.md#turn)
    "};
    let root = workspace(&[
        ("0000001-glossary.md", ROOT),
        ("0000002-child.md", CHILD),
        ("0000005-study.md", page),
    ]);

    let errors = messages(&run(root.path()).errors);

    assert_eq!(errors, [
        "docs/wiki/0000005-study.md:9: `../../crates/x/src/lib.rs#L2` cites source by a relative \
         link, which follows the work tree; `jp wiki pin W-0000005` turns it into a permalink.",
        "docs/wiki/0000005-study.md:11: A Snapshot page doesn't embed, since an embed shows the \
         source as it is now; copy the excerpt and cite it.",
    ]);
}

#[test]
fn the_hierarchy_and_order_are_held_together() {
    let page = |title: &str, status: &str, parent: &str| {
        format!(
            "# {title}\n\n- **Status**: {status}\n- **Authors**: jp\n- **Date**: 2026-10-30\n- \
             **Parent**: {parent}\n- **Summary**: x\n"
        )
    };
    // An archived page keeps whatever parent it had, a missing one included.
    let root = workspace(&[
        ("0000001-a.md", &page("A", "Reference", "W-0000002")),
        ("0000002-b.md", &page("B", "Reference", "W-0000001")),
        ("0000003-c.md", &page("C", "Archived", "W-0000009")),
        ("0000004-d.md", &page("D", "Reference", "W-0000003")),
    ]);
    write(
        root.path(),
        "docs/wiki/.order.json",
        r#"{"top": ["W-0000003", "nope"]}"#,
    );

    let errors = messages(&run(root.path()).errors);

    assert_eq!(errors, [
        "docs/wiki/0000004-d.md: `Parent` names W-0000003, which is archived; name an active page.",
        "docs/wiki/0000001-a.md: Parents loop: W-0000001 under W-0000002 under W-0000001.",
        "docs/wiki/.order.json: `top` names W-0000003, which is archived.",
        "docs/wiki/.order.json: `top` names `nope`, and no page has it.",
    ]);
}

#[test]
fn orphans_and_superseded_citations_are_warnings() {
    let page = indoc! {"
        # Lonely

        - **Status**: Reference
        - **Authors**: jp
        - **Date**: 2026-10-30
        - **Summary**: Nobody links here.

        Built on RFD 033.
    "};
    let root = workspace(&[("0000006-lonely.md", page)]);

    let report = run(root.path());

    assert_eq!(messages(&report.errors), Vec::<String>::new());
    assert_eq!(messages(&report.warnings), [
        "docs/wiki/0000006-lonely.md: W-0000006 is an orphan: it has no parent, no page, RFD, \
         ticket, or the wiki's front page links to it, and it names no RFD under `relevant-rfd`.",
        "docs/wiki/0000006-lonely.md: The page cites RFD 033, which is Superseded.",
    ]);
}

/// The front page is where a reader starts, so a page it links to is reached.
#[test]
fn a_page_the_front_page_links_to_is_no_orphan() {
    let page = indoc! {"
        # Hub

        - **Status**: Reference
        - **Authors**: jp
        - **Date**: 2026-10-30
        - **Summary**: Where the guides start.
    "};
    let root = workspace(&[
        ("0000006-hub.md", page),
        (
            "index.md",
            "# Wiki\n\nStart with the [guides](./0000006-hub.md).\n",
        ),
    ]);

    assert_eq!(messages(&run(root.path()).warnings), Vec::<String>::new());
}

#[test]
fn a_missing_epoch_beside_pages_is_an_error() {
    let root = workspace(&[("0000001-glossary.md", ROOT), ("0000002-child.md", CHILD)]);
    fs::remove_file(root.path().join("docs/wiki").join(EPOCH_FILE)).unwrap();

    let errors = messages(&run(root.path()).errors);

    assert_eq!(errors, ["docs/wiki/.epoch: The epoch file is missing, so \
                         no new page can be created; restore it from \
                         version control."]);
}

#[test]
fn line_problems_name_what_is_wrong() {
    let lines = ["fn one() {}", "fn by_rfd() {}"];

    assert_eq!(
        lines_problem(&lines, Some("L2"), Some("Links::by_rfd")),
        None
    );
    assert_eq!(lines_problem(&lines, None, Some("by_rfd")), None);
    assert_eq!(lines_problem(&lines, None, None), None);
    assert_eq!(
        lines_problem(&lines, Some("section"), None).as_deref(),
        Some("`#section` is not a line range; write `#L12` or `#L12-L24`.")
    );
    assert_eq!(
        lines_problem(&lines, Some("L1"), Some("gone")).as_deref(),
        Some("`gone` is not in the file.")
    );
}
