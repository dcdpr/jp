use camino_tempfile::Utf8TempDir;
use indoc::indoc;

use super::*;

/// A fragment that follows the format.
const FRAGMENT: &str = "---\ntype: fixed\n---\nA turn that stalls while idle is retried.\n";

/// A highlight that follows the format.
const HIGHLIGHT: &str = "---\ntitle: Tool calls fan out\nauthors: [\"@JeanMertz\"]\n---\nThe \
                         assistant calls one tool on many inputs at once.\n";

/// A fragments directory holding `files`, each a path below it and the file's
/// contents.
fn fragments_dir(files: &[(&str, &str)]) -> Utf8TempDir {
    let dir = camino_tempfile::tempdir().unwrap();
    for (path, contents) in files {
        let path = dir.path().join(path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, contents).unwrap();
    }

    dir
}

/// Each problem in `report`, as its path below `dir` and its message.
fn problems(report: &Report, dir: &Utf8Path) -> Vec<String> {
    report
        .problems
        .iter()
        .map(|problem| {
            let path = problem.path.strip_prefix(dir).unwrap();
            // Written with `/` on every platform, so one expectation fits all.
            format!("{}: {}", path.as_str().replace('\\', "/"), problem.kind)
        })
        .collect()
}

#[test]
fn a_directory_holding_only_its_readme_has_nothing_to_check() {
    // The README documents the formats; its examples would not pass as a
    // fragment.
    let dir = fragments_dir(&[("README.md", "# Changelog fragments\n")]);

    let report = check(dir.path()).unwrap();

    assert_eq!(report.fragments, 0);
    assert_eq!(report.highlights, 0);
    assert_eq!(report.problems, vec![]);
}

#[test]
fn fragments_are_read_at_the_top_level_and_in_each_release() {
    let dir = fragments_dir(&[
        ("README.md", "# Changelog fragments\n"),
        ("enable-explicit.md", FRAGMENT),
        (
            "0.3.0/overview.md",
            "JP 0.3.0 lets the assistant call one tool on many inputs.\n",
        ),
        ("0.3.0/retry-idle-turns.md", FRAGMENT),
        ("1.0.0-rc.1/plugin-tools.md", FRAGMENT),
    ]);

    let report = check(dir.path()).unwrap();

    // `overview.md` introduces its release rather than recording a change, so
    // it is neither counted nor held to a fragment's format.
    assert_eq!(report.fragments, 3);
    assert_eq!(report.problems, vec![]);
}

#[test]
fn highlights_are_read_at_the_top_level_and_in_each_release() {
    let dir = fragments_dir(&[
        ("highlights/fan-out.md", HIGHLIGHT),
        ("0.3.0/retry-idle-turns.md", FRAGMENT),
        ("0.3.0/highlights/plugin-tools.md", HIGHLIGHT),
    ]);

    let report = check(dir.path()).unwrap();

    assert_eq!(report.fragments, 1);
    assert_eq!(report.highlights, 2);
    assert_eq!(problems(&report, dir.path()), Vec::<String>::new());
}

#[test]
fn each_file_is_held_to_the_format_of_the_directory_it_sits_in() {
    // Each written to the other's place.
    let dir = fragments_dir(&[
        ("fan-out.md", HIGHLIGHT),
        ("highlights/retry-idle-turns.md", FRAGMENT),
    ]);

    let report = check(dir.path()).unwrap();

    assert_eq!(report.fragments, 1);
    assert_eq!(report.highlights, 1);
    assert_eq!(problems(&report, dir.path()), vec![
        "fan-out.md: invalid frontmatter: unknown field `title`, expected one of type, \
         change_requests at line 2, column 1",
        "highlights/retry-idle-turns.md: invalid frontmatter: unknown field `type`, expected one \
         of title, authors, change_requests at line 2, column 1",
    ]);
}

#[test]
fn a_malformed_fragment_is_reported_with_the_rule_it_breaks() {
    let dir = fragments_dir(&[
        ("a.md", "type: fixed\n\nEntry.\n"),
        ("0.3.0/b.md", "---\ntype: fixed\n---\n\n"),
    ]);

    let report = check(dir.path()).unwrap();

    assert_eq!(report.fragments, 2);
    assert_eq!(problems(&report, dir.path()), vec![
        "0.3.0/b.md: no entry: say below the frontmatter what changed for users",
        "a.md: no frontmatter: the first line has to be `---`",
    ]);
}

#[test]
fn a_malformed_highlight_is_reported_with_the_rule_it_breaks() {
    let dir = fragments_dir(&[
        (
            "highlights/fan-out.md",
            "---\ntitle: \"\"\nauthors: [\"@JeanMertz\"]\n---\nProse.\n",
        ),
        (
            "highlights/editor.md",
            "---\ntitle: Unified editor\nauthors: []\n---\nProse.\n",
        ),
        (
            "0.3.0/highlights/plugin-tools.md",
            "---\ntitle: Plugin tools\nauthors: [\"@JeanMertz\"]\n---\n",
        ),
    ]);

    let report = check(dir.path()).unwrap();

    assert_eq!(report.highlights, 3);
    assert_eq!(problems(&report, dir.path()), vec![
        "0.3.0/highlights/plugin-tools.md: no prose: write below the frontmatter what the feature \
         does for users",
        "highlights/editor.md: no authors: list the people to credit for the feature in `authors`",
        "highlights/fan-out.md: no title: name the feature in `title`, which heads its section of \
         the release notes",
    ]);
}

#[test]
fn reserved_slugs_are_refused_in_any_letter_case() {
    let dir = fragments_dir(&[
        ("overview.md", FRAGMENT),
        ("Readme.md", FRAGMENT),
        ("0.3.0/Overview.md", FRAGMENT),
        ("0.3.0/README.md", FRAGMENT),
        ("highlights/README.md", HIGHLIGHT),
        ("0.3.0/highlights/overview.md", HIGHLIGHT),
    ]);

    let report = check(dir.path()).unwrap();

    assert_eq!(report.fragments, 0);
    assert_eq!(report.highlights, 0);
    assert_eq!(problems(&report, dir.path()), vec![
        "0.3.0/Overview.md: the slug `overview` is reserved, in any letter case, for a release's \
         introduction, `overview.md` in the release's directory; rename it",
        "0.3.0/README.md: the slug `readme` is reserved, in any letter case, for the fragments \
         directory's documentation, `README.md` at its top level; rename it",
        "0.3.0/highlights/overview.md: the slug `overview` is reserved, in any letter case, for a \
         release's introduction, `overview.md` in the release's directory; rename it",
        "Readme.md: the slug `readme` is reserved, in any letter case, for the fragments \
         directory's documentation, `README.md` at its top level; rename it",
        "highlights/README.md: the slug `readme` is reserved, in any letter case, for the \
         fragments directory's documentation, `README.md` at its top level; rename it",
        "overview.md: the slug `overview` is reserved, in any letter case, for a release's \
         introduction, `overview.md` in the release's directory; rename it",
    ]);
}

#[test]
fn a_file_not_named_slug_md_is_not_a_fragment() {
    let dir = fragments_dir(&[
        (".md", FRAGMENT),
        ("enable-explicit.markdown", FRAGMENT),
        ("notes.txt", "Remember the overview.\n"),
        ("0.3.0/overview.md.bak", "An older introduction.\n"),
    ]);

    let report = check(dir.path()).unwrap();

    assert_eq!(report.fragments, 0);
    assert_eq!(problems(&report, dir.path()), vec![
        ".md: not a fragment, which is named `<slug>.md`; rename it, or move it out of the \
         fragments directory",
        "0.3.0/overview.md.bak: not a fragment, which is named `<slug>.md`; rename it, or move it \
         out of the fragments directory",
        "enable-explicit.markdown: not a fragment, which is named `<slug>.md`; rename it, or move \
         it out of the fragments directory",
        "notes.txt: not a fragment, which is named `<slug>.md`; rename it, or move it out of the \
         fragments directory",
    ]);
}

#[test]
fn a_file_in_highlights_not_named_slug_md_is_not_a_highlight() {
    let dir = fragments_dir(&[
        ("highlights/fan-out.markdown", HIGHLIGHT),
        ("0.3.0/highlights/notes.txt", "Mention the editor.\n"),
    ]);

    let report = check(dir.path()).unwrap();

    assert_eq!(report.highlights, 0);
    assert_eq!(problems(&report, dir.path()), vec![
        "0.3.0/highlights/notes.txt: not a highlight, which is named `<slug>.md`; rename it, or \
         move it out of the fragments directory",
        "highlights/fan-out.markdown: not a highlight, which is named `<slug>.md`; rename it, or \
         move it out of the fragments directory",
    ]);
}

#[test]
fn a_directory_not_named_after_a_version_is_reported_without_reading_it() {
    let dir = fragments_dir(&[
        ("drafts/plugin-tools.md", "Not a fragment yet.\n"),
        ("v0.3.0/retry-idle-turns.md", FRAGMENT),
        // Matched as written: a highlight here would never reach a release.
        ("Highlights/fan-out.md", HIGHLIGHT),
    ]);
    fs::create_dir(dir.path().join("0.3")).unwrap();

    let report = check(dir.path()).unwrap();

    assert_eq!(report.fragments, 0);
    assert_eq!(report.highlights, 0);
    assert_eq!(problems(&report, dir.path()), vec![
        "0.3: neither `highlights/` nor a release's directory, which is named after its version, \
         such as `0.3.0`",
        "Highlights: neither `highlights/` nor a release's directory, which is named after its \
         version, such as `0.3.0`",
        "drafts: neither `highlights/` nor a release's directory, which is named after its \
         version, such as `0.3.0`",
        "v0.3.0: neither `highlights/` nor a release's directory, which is named after its \
         version, such as `0.3.0`",
    ]);
}

#[test]
fn a_directory_inside_a_release_is_reported_without_reading_it() {
    let dir = fragments_dir(&[
        ("0.3.0/retry-idle-turns.md", FRAGMENT),
        ("0.3.0/drafts/plugin-tools.md", "Not a fragment yet.\n"),
    ]);

    let report = check(dir.path()).unwrap();

    assert_eq!(report.fragments, 1);
    assert_eq!(problems(&report, dir.path()), vec![
        "0.3.0/drafts: a release's directory holds no directory but `highlights/`"
    ]);
}

#[test]
fn a_directory_inside_highlights_is_reported_without_reading_it() {
    let dir = fragments_dir(&[
        ("highlights/fan-out.md", HIGHLIGHT),
        (
            "highlights/drafts/plugin-tools.md",
            "Not a highlight yet.\n",
        ),
        ("0.3.0/highlights/old/editor.md", HIGHLIGHT),
    ]);

    let report = check(dir.path()).unwrap();

    assert_eq!(report.highlights, 1);
    assert_eq!(problems(&report, dir.path()), vec![
        "0.3.0/highlights/old: a `highlights/` directory holds no directories",
        "highlights/drafts: a `highlights/` directory holds no directories",
    ]);
}

#[test]
fn a_fragment_that_is_not_utf8_is_reported() {
    let dir = fragments_dir(&[]);
    fs::write(
        dir.path().join("latin-1.md"),
        b"---\ntype: fixed\n---\nCaf\xe9 menus render again.\n",
    )
    .unwrap();

    let report = check(dir.path()).unwrap();

    assert_eq!(report.fragments, 1);
    assert_eq!(problems(&report, dir.path()), vec![
        "latin-1.md: not UTF-8 text"
    ]);
}

#[test]
fn a_missing_fragments_directory_is_an_error_rather_than_an_empty_one() {
    let dir = fragments_dir(&[]);
    let missing = dir.path().join("changelog.d");

    let error = check(&missing).unwrap_err();

    assert_eq!(error.path, missing);
    assert_eq!(error.source.kind(), io::ErrorKind::NotFound);
}

#[test]
fn the_reserved_slugs_say_what_they_are_reserved_for_in_any_letter_case() {
    assert_eq!(
        reserved_for("Overview"),
        Some("a release's introduction, `overview.md` in the release's directory")
    );
    assert_eq!(
        reserved_for("readme"),
        Some("the fragments directory's documentation, `README.md` at its top level")
    );
    assert_eq!(reserved_for("overview-of-tools"), None);
}

#[test]
fn only_a_semantic_version_names_a_release() {
    assert!(is_version("0.3.0"));
    assert!(is_version("1.0.0-rc.1"));

    assert!(!is_version("v0.3.0"));
    assert!(!is_version("0.3"));
    assert!(!is_version("00.3.0"));
    assert!(!is_version("drafts"));
}

#[test]
fn a_report_without_problems_says_how_many_files_it_checked() {
    let report = Report {
        fragments: 1,
        highlights: 2,
        problems: vec![],
    };

    assert_eq!(
        report.to_string(),
        "Checked 1 fragment and 2 highlights: no problems."
    );
}

#[test]
fn a_report_lists_each_problem_then_counts_them() {
    let report = Report {
        fragments: 2,
        highlights: 1,
        problems: vec![
            Problem {
                path: Utf8PathBuf::from("changelog.d/a.md"),
                kind: ProblemKind::NotUtf8,
            },
            Problem {
                path: Utf8PathBuf::from("changelog.d/notes.txt"),
                kind: ProblemKind::NotAFragment,
            },
        ],
    };

    assert_eq!(report.to_string(), indoc! {"
        changelog.d/a.md: not UTF-8 text
        changelog.d/notes.txt: not a fragment, which is named `<slug>.md`; rename it, or move it out of the fragments directory

        Checked 2 fragments and 1 highlight: 2 problems."});
}
