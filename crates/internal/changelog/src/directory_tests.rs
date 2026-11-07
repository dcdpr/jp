use camino_tempfile::Utf8TempDir;
use indoc::indoc;

use super::*;
use crate::fragment::Type;

/// A fragment's text that follows the format, for a file named
/// `<slug>.fixed.md`.
const FRAGMENT: &str = "A turn that stalls while idle is retried.\n";

/// A highlight that follows the format.
const HIGHLIGHT: &str = "# Tool calls fan out\n\n- **Authors**: @JeanMertz\n\nThe assistant calls \
                         one tool on many inputs at once.\n";

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
        ("enable-explicit.removed.md", FRAGMENT),
        (
            "0.3.0/overview.md",
            "JP 0.3.0 lets the assistant call one tool on many inputs.\n",
        ),
        ("0.3.0/retry-idle-turns.fixed.md", FRAGMENT),
        ("1.0.0-rc.1/plugin-tools.added.md", FRAGMENT),
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
        ("0.3.0/retry-idle-turns.fixed.md", FRAGMENT),
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
        ("highlights/retry-idle-turns.fixed.md", FRAGMENT),
    ]);

    let report = check(dir.path()).unwrap();

    assert_eq!(report.fragments, 0);
    assert_eq!(report.highlights, 1);
    assert_eq!(problems(&report, dir.path()), vec![
        "fan-out.md: no type in the name: name it `fan-out.<type>.md`, such as \
         `fan-out.fixed.md`, with one of: added, changed, deprecated, removed, fixed, security, \
         performance",
        "highlights/retry-idle-turns.fixed.md: no title: a highlight opens with `# Title`, which \
         heads its section of the release notes",
    ]);
}

#[test]
fn a_malformed_fragment_is_reported_with_the_rule_it_breaks() {
    let dir = fragments_dir(&[
        ("a.fixed.md", "- **Type**: fixed\n\nEntry.\n"),
        ("0.3.0/b.fixed.md", "\n\n"),
    ]);

    let report = check(dir.path()).unwrap();

    assert_eq!(report.fragments, 2);
    assert_eq!(problems(&report, dir.path()), vec![
        "0.3.0/b.fixed.md: no entry: say what changed for users",
        "a.fixed.md: line 1 names a type, which a fragment's file name holds: `<slug>.<type>.md`",
    ]);
}

#[test]
fn a_malformed_highlight_is_reported_with_the_rule_it_breaks() {
    let dir = fragments_dir(&[
        (
            "highlights/fan-out.md",
            "#\n\n- **Authors**: @JeanMertz\n\nProse.\n",
        ),
        ("highlights/editor.md", "# Unified editor\n\nProse.\n"),
        (
            "0.3.0/highlights/plugin-tools.md",
            "# Plugin tools\n\n- **Authors**: @JeanMertz\n",
        ),
    ]);

    let report = check(dir.path()).unwrap();

    assert_eq!(report.highlights, 3);
    assert_eq!(problems(&report, dir.path()), vec![
        "0.3.0/highlights/plugin-tools.md: no prose: write what the feature does for users below \
         the title and its authors",
        "highlights/editor.md: no authors: list each person to credit for the feature below the \
         title, as `- **Authors**: @JeanMertz`",
        "highlights/fan-out.md: no title: a highlight opens with `# Title`, which heads its \
         section of the release notes",
    ]);
}

/// The type in a fragment's name, and the `highlights/` a highlight sits in,
/// keep every slug clear of `README.md` and `overview.md`.
#[test]
fn any_slug_can_name_a_fragment_or_a_highlight() {
    let dir = fragments_dir(&[
        ("README.md", "# Changelog fragments\n"),
        ("readme.fixed.md", FRAGMENT),
        ("0.3.0/overview.md", "JP 0.3.0 retries idle turns.\n"),
        ("0.3.0/overview.fixed.md", FRAGMENT),
        ("highlights/README.md", HIGHLIGHT),
        ("0.3.0/highlights/overview.md", HIGHLIGHT),
    ]);

    let report = check(dir.path()).unwrap();

    assert_eq!(report.fragments, 2);
    assert_eq!(report.highlights, 2);
    assert_eq!(problems(&report, dir.path()), Vec::<String>::new());
}

#[test]
fn a_file_not_named_slug_type_md_is_not_a_fragment() {
    let dir = fragments_dir(&[
        (".md", FRAGMENT),
        ("enable-explicit.markdown", FRAGMENT),
        ("notes.txt", "Remember the overview.\n"),
        ("0.3.0/overview.md.bak", "An older introduction.\n"),
        ("retry.md", FRAGMENT),
        ("retry.feat.md", FRAGMENT),
    ]);

    let report = check(dir.path()).unwrap();

    assert_eq!(report.fragments, 0);
    assert_eq!(problems(&report, dir.path()), vec![
        ".md: not a fragment, which is named `<slug>.<type>.md`; rename it, or move it out of the \
         fragments directory",
        "0.3.0/overview.md.bak: not a fragment, which is named `<slug>.<type>.md`; rename it, or \
         move it out of the fragments directory",
        "enable-explicit.markdown: not a fragment, which is named `<slug>.<type>.md`; rename it, \
         or move it out of the fragments directory",
        "notes.txt: not a fragment, which is named `<slug>.<type>.md`; rename it, or move it out \
         of the fragments directory",
        "retry.feat.md: `feat` is not a changelog type: the part of the name before `.md` is one \
         of: added, changed, deprecated, removed, fixed, security, performance",
        "retry.md: no type in the name: name it `retry.<type>.md`, such as `retry.fixed.md`, with \
         one of: added, changed, deprecated, removed, fixed, security, performance",
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
        ("v0.3.0/retry-idle-turns.fixed.md", FRAGMENT),
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
        ("0.3.0/retry-idle-turns.fixed.md", FRAGMENT),
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
        dir.path().join("latin-1.fixed.md"),
        b"Caf\xe9 menus render again.\n",
    )
    .unwrap();

    let report = check(dir.path()).unwrap();

    assert_eq!(report.fragments, 1);
    assert_eq!(problems(&report, dir.path()), vec![
        "latin-1.fixed.md: not UTF-8 text"
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
fn reading_the_unreleased_part_leaves_every_release_out() {
    let dir = fragments_dir(&[
        ("README.md", "# Changelog fragments\n"),
        ("enable-explicit.fixed.md", FRAGMENT),
        ("highlights/fan-out.md", HIGHLIGHT),
        ("0.3.0/retry-idle-turns.fixed.md", FRAGMENT),
        // Broken, and in a part this read doesn't touch.
        ("0.3.0/highlights/plugin-tools.md", "Not a highlight.\n"),
    ]);

    let contents = read(dir.path(), &Part::Unreleased).unwrap();

    let fragments: Vec<(&str, &str)> = contents
        .fragments
        .iter()
        .map(|filed| (filed.slug.as_str(), filed.path.as_str()))
        .collect();
    assert_eq!(fragments, [("enable-explicit", "enable-explicit.fixed.md")]);
    let highlights: Vec<(&str, &str)> = contents
        .highlights
        .iter()
        .map(|filed| (filed.slug.as_str(), filed.path.as_str()))
        .collect();
    assert_eq!(highlights, [("fan-out", "highlights/fan-out.md")]);
    assert_eq!(
        contents.fragments[0].file,
        fragment::parse(Type::Fixed, FRAGMENT).unwrap()
    );
}

#[test]
fn reading_a_release_returns_its_files_without_its_overview() {
    let dir = fragments_dir(&[
        ("enable-explicit.removed.md", FRAGMENT),
        (
            "0.3.0/overview.md",
            "JP 0.3.0 calls tools on many inputs.\n",
        ),
        ("0.3.0/retry-idle-turns.fixed.md", FRAGMENT),
        ("0.3.0/highlights/plugin-tools.md", HIGHLIGHT),
        ("0.4.0/later.added.md", FRAGMENT),
    ]);

    let contents = read(dir.path(), &Part::Release(Version::parse("0.3.0").unwrap())).unwrap();

    let paths: Vec<&str> = contents
        .fragments
        .iter()
        .map(|filed| filed.path.as_str())
        .collect();
    assert_eq!(paths, ["0.3.0/retry-idle-turns.fixed.md"]);
    assert_eq!(contents.fragments[0].file.type_, Type::Fixed);
    assert_eq!(contents.highlights[0].slug, "plugin-tools");
    assert_eq!(
        contents.highlights[0].path,
        "0.3.0/highlights/plugin-tools.md"
    );
}

#[test]
fn reading_a_part_reports_every_problem_in_it() {
    let dir = fragments_dir(&[
        ("0.3.0/a.fixed.md", "- **Type**: fixed\n\nEntry.\n"),
        ("0.3.0/notes.txt", "Remember the overview.\n"),
        ("0.3.0/retry-idle-turns.fixed.md", FRAGMENT),
    ]);

    let Err(ReadError::Problems(found)) =
        read(dir.path(), &Part::Release(Version::parse("0.3.0").unwrap()))
    else {
        panic!("expected the part's problems");
    };

    let report = Report {
        problems: found,
        ..Report::default()
    };
    assert_eq!(problems(&report, dir.path()), vec![
        "0.3.0/a.fixed.md: line 1 names a type, which a fragment's file name holds: \
         `<slug>.<type>.md`",
        "0.3.0/notes.txt: not a fragment, which is named `<slug>.<type>.md`; rename it, or move \
         it out of the fragments directory",
    ]);
}

#[test]
fn reading_a_release_without_a_directory_says_so() {
    let dir = fragments_dir(&[("0.3.0/retry-idle-turns.fixed.md", FRAGMENT)]);

    let error = read(dir.path(), &Part::Release(Version::parse("0.4.0").unwrap())).unwrap_err();

    assert!(matches!(error, ReadError::NoRelease { .. }), "{error:?}");
    assert_eq!(
        error.to_string(),
        format!("{} has no release 0.4.0: no directory 0.4.0/", dir.path())
    );
}

#[test]
fn a_place_is_read_from_a_path_and_written_back() {
    for (path, part, highlight, name) in [
        ("retry.fixed.md", Part::Unreleased, false, "retry.fixed.md"),
        (
            "highlights/fan-out.md",
            Part::Unreleased,
            true,
            "fan-out.md",
        ),
        (
            "0.3.0/retry.fixed.md",
            Part::Release(Version::new(0, 3, 0)),
            false,
            "retry.fixed.md",
        ),
        (
            "0.3.0/highlights/fan-out.md",
            Part::Release(Version::new(0, 3, 0)),
            true,
            "fan-out.md",
        ),
    ] {
        let place = Place::parse(path).unwrap();

        assert_eq!(place, Place {
            part,
            highlight,
            name: name.to_owned(),
        });
        assert_eq!(place.path(), path);
    }
}

#[test]
fn a_path_the_layout_holds_nothing_at_is_no_place() {
    for path in [
        "",
        "0.3.0/",
        "drafts/retry.fixed.md",
        "0.3.0/drafts/retry.fixed.md",
        "a/b/c.md",
    ] {
        assert_eq!(Place::parse(path), None, "{path}");
    }
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
                path: Utf8PathBuf::from("changelog.d/a.fixed.md"),
                kind: ProblemKind::NotUtf8,
            },
            Problem {
                path: Utf8PathBuf::from("changelog.d/notes.txt"),
                kind: ProblemKind::FragmentName(NameError::NotAFragment),
            },
        ],
    };

    assert_eq!(report.to_string(), indoc! {"
        changelog.d/a.fixed.md: not UTF-8 text
        changelog.d/notes.txt: not a fragment, which is named `<slug>.<type>.md`; rename it, or move it out of the fragments directory

        Checked 2 fragments and 1 highlight: 2 problems."});
}
