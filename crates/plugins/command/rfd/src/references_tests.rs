use camino_tempfile::Utf8TempDir;

use super::*;

fn token(old: &str, new: &str) -> Rule {
    Rule::Token {
        old: old.to_owned(),
        new: new.to_owned(),
    }
}

fn stem(stem: &str, to: &str) -> Rule {
    Rule::Stem {
        stem: stem.to_owned(),
        to: to.to_owned(),
    }
}

fn citation(old: &str, new: &str) -> Rule {
    Rule::Citation {
        old: old.to_owned(),
        new: new.to_owned(),
    }
}

/// `sed`'s `g` flag consumed the boundary character of one match, which is why
/// the recipes ran the token rule twice; checking the boundaries without
/// consuming them reaches back-to-back mentions in one pass.
#[test]
fn a_token_needs_a_non_word_character_on_both_sides() {
    let text = "D05 D05, (D05) xD05 D056 D05_ _D05\nD05";

    assert_eq!(
        apply(text, &[token("D05", "117")]),
        "117 117, (117) xD05 D056 D05_ _D05\n117"
    );
}

#[test]
fn a_citation_needs_no_digit_after_it() {
    let text = "RFD 042, RFD 0421, and RFD 042: the end. RFD 042";

    assert_eq!(
        apply(text, &[citation("042", "097")]),
        "RFD 097, RFD 0421, and RFD 097: the end. RFD 097"
    );
}

/// A stem names one file; another file whose slug merely starts the same way,
/// as two documents sharing an id after a merge can, keeps its links.
#[test]
fn a_stem_matches_a_whole_file_name_with_or_without_md() {
    let text = "[a](D05-sketch.md) [b](D05-sketch) [c](D05-sketch-two.md) [d](D05-sketch.md#x)";

    assert_eq!(
        apply(text, &[stem("D05-sketch", "../117-sketch")]),
        "[a](../117-sketch.md) [b](../117-sketch) [c](D05-sketch-two.md) [d](../117-sketch.md#x)"
    );
}

#[test]
fn rules_apply_in_order() {
    let text = "See RFD D05 in [it](D05-sketch.md); D05 is a draft.";

    let rules = [
        citation("D05", "117"),
        stem("D05-sketch", "117-sketch"),
        token("D05", "117"),
    ];

    assert_eq!(
        apply(text, &rules),
        "See RFD 117 in [it](117-sketch.md); 117 is a draft."
    );
}

#[test]
fn a_literal_replaces_every_occurrence() {
    let rule = Rule::Literal {
        from: "042-old.md".to_owned(),
        to: "042-new.md".to_owned(),
    };

    assert_eq!(
        apply("[a](042-old.md) [b](../rfd/042-old.md)", &[rule]),
        "[a](042-new.md) [b](../rfd/042-new.md)"
    );
}

#[test]
fn cites_reads_a_whole_id() {
    assert!(cites("See RFD 042.", "042"));
    assert!(cites("RFD 042", "042"));
    assert!(!cites("RFD 0421", "042"));
    assert!(!cites("RFD 042a", "042"));
    assert!(!cites("rfd 042", "042"));
}

#[test]
fn relative_paths_climb_out_of_the_shared_prefix() {
    let path = |from: &str, to: &str| relative_path(Utf8Path::new(from), Utf8Path::new(to));

    assert_eq!(path("/r/docs/ticket", "/r/docs/rfd"), "../rfd");
    assert_eq!(path("/r/docs/rfd/drafts", "/r/docs/rfd"), "..");
    assert_eq!(path("/r/docs/rfd", "/r/docs/rfd/drafts"), "drafts");
    assert_eq!(path("/r/tickets", "/r/docs/rfd"), "../docs/rfd");
    assert_eq!(path("/r/docs/rfd", "/r/docs/rfd"), "");
}

#[test]
fn a_link_in_the_same_directory_is_the_file_name() {
    let rfd = Utf8Path::new("/r/docs/rfd");

    assert_eq!(link(rfd, rfd, "042-x.md"), "042-x.md");
    assert_eq!(
        link(Utf8Path::new("/r/docs/ticket"), rfd, "042-x.md"),
        "../rfd/042-x.md"
    );
}

#[test]
fn rewrite_file_writes_only_what_changed() {
    let dir = Utf8TempDir::new().unwrap();
    let changed = dir.path().join("a.md");
    let unchanged = dir.path().join("b.md");
    fs::write(&changed, "RFD D05\n").unwrap();
    fs::write(&unchanged, "nothing here\n").unwrap();
    let rules = [citation("D05", "117")];

    assert_eq!(rewrite_file(&changed, &rules), Ok(true));
    assert_eq!(rewrite_file(&unchanged, &rules), Ok(false));
    assert_eq!(fs::read_to_string(&changed).unwrap(), "RFD 117\n");
}

#[test]
fn markdown_files_are_the_md_files_directly_in_a_directory() {
    let dir = Utf8TempDir::new().unwrap();
    fs::create_dir_all(dir.path().join("drafts")).unwrap();
    fs::write(dir.path().join("b.md"), "").unwrap();
    fs::write(dir.path().join("a.md"), "").unwrap();
    fs::write(dir.path().join("c.json"), "").unwrap();
    fs::write(dir.path().join("drafts/d.md"), "").unwrap();

    assert_eq!(markdown_files(dir.path()).unwrap(), [
        dir.path().join("a.md"),
        dir.path().join("b.md")
    ]);
    assert_eq!(markdown_files(&dir.path().join("missing")), Ok(vec![]));
}

/// The leftovers a rewrite reports are found the way `rg` finds them: hidden
/// files and ignored ones don't count, and neither do the directories the
/// rewrite already covered.
#[test]
fn mentions_search_what_rg_would_outside_the_excluded_directories() {
    let dir = Utf8TempDir::new().unwrap();
    let root = dir.path();
    for (path, text) in [
        ("docs/rfd/042-x.md", "RFD 042"),
        ("docs/architecture.md", "See RFD 042."),
        ("crates/lib.rs", "// RFD 0420 is another one"),
        ("crates/main.rs", "// docs/rfd/042-x.md"),
        (".hidden/notes.md", "RFD 042"),
        ("ignored/notes.md", "RFD 042"),
    ] {
        let path = root.join(path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }
    fs::write(root.join(".ignore"), "ignored/\n").unwrap();

    let found = mentions(root, &[&root.join("docs/rfd")], |text| {
        cites(text, "042") || text.contains("042-x.md")
    });

    assert_eq!(found, [
        root.join("crates/main.rs"),
        root.join("docs/architecture.md")
    ]);
}
