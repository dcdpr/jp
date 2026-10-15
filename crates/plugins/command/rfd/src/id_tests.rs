use std::fs;

use camino_tempfile::Utf8TempDir;

use super::*;

fn id(text: &str) -> RfdId {
    text.parse().unwrap()
}

#[test]
fn every_spelling_of_a_number_reads_as_one_id() {
    for text in ["42", "042", "0042", " 42 ", "RFD 042", "rfd 42"] {
        assert_eq!(id(text), RfdId::Published(42), "{text}");
    }
    assert_eq!(id("116").to_string(), "116");
    assert_eq!(id("7").to_string(), "007");
}

#[test]
fn every_spelling_of_a_draft_reads_as_one_id() {
    for text in ["D05", "D5", "d05", "RFD D05"] {
        assert_eq!(id(text), RfdId::Draft(5), "{text}");
    }
    assert_eq!(id("D5").to_string(), "D05");
}

/// `000` belongs to the templates and `D00` to nothing; filenames carry three
/// and two digits, so nothing past them names a file either.
#[test]
fn ids_outside_the_ranges_filenames_carry_are_refused() {
    for text in [
        "0",
        "000",
        "1000",
        "D0",
        "D100",
        "",
        "D",
        "RFD",
        "42a",
        "T-02wt0kx",
        "-1",
    ] {
        assert_eq!(
            text.parse::<RfdId>(),
            Err(InvalidId(text.trim().to_owned())),
            "{text}"
        );
    }
}

#[test]
fn an_invalid_id_says_what_an_id_looks_like() {
    assert_eq!(
        "abc".parse::<RfdId>().unwrap_err().to_string(),
        "Invalid RFD id 'abc'. Use a number (41) or draft ID (D01)."
    );
}

/// A file name carries the id exactly as the board file and the links do, so
/// only the canonical width counts.
#[test]
fn a_file_name_names_an_id_only_in_its_canonical_form() {
    assert_eq!(RfdId::of_file("042-slug.md"), Some(RfdId::Published(42)));
    assert_eq!(RfdId::of_file("D05-slug.md"), Some(RfdId::Draft(5)));

    for name in [
        "42-slug.md",
        "0042-slug.md",
        "D5-slug.md",
        "000-design-template.md",
        "d05-slug.md",
        "D+5-slug.md",
        "slug.md",
        "index.md",
    ] {
        assert_eq!(RfdId::of_file(name), None, "{name}");
    }
}

#[test]
fn resolve_finds_published_rfds_and_drafts_in_their_directories() {
    let dir = Utf8TempDir::new().unwrap();
    fs::create_dir_all(dir.path().join("drafts")).unwrap();
    fs::write(dir.path().join("042-published.md"), "").unwrap();
    fs::write(dir.path().join("drafts/D05-draft.md"), "").unwrap();
    // Nothing a draft id names lives in the RFD directory itself.
    fs::write(dir.path().join("D05-misplaced.md"), "").unwrap();

    assert_eq!(
        resolve(dir.path(), RfdId::Published(42)).unwrap(),
        dir.path().join("042-published.md")
    );
    assert_eq!(
        resolve(dir.path(), RfdId::Draft(5)).unwrap(),
        dir.path().join("drafts/D05-draft.md")
    );
}

#[test]
fn resolve_reports_a_missing_rfd() {
    let dir = Utf8TempDir::new().unwrap();

    assert_eq!(
        resolve(dir.path(), RfdId::Published(42))
            .unwrap_err()
            .to_string(),
        "No RFD found with number 042."
    );
    // No drafts directory at all is the same as no draft in it.
    assert_eq!(
        resolve(dir.path(), RfdId::Draft(5))
            .unwrap_err()
            .to_string(),
        "No draft RFD found with ID D05."
    );
}

/// Two files sharing an id is a repository fault: picking one would let a
/// rewrite land on the wrong document.
#[test]
fn resolve_refuses_an_id_two_files_share() {
    let dir = Utf8TempDir::new().unwrap();
    fs::write(dir.path().join("042-one.md"), "").unwrap();
    fs::write(dir.path().join("042-two.md"), "").unwrap();

    let error = resolve(dir.path(), RfdId::Published(42)).unwrap_err();

    assert_eq!(
        error.to_string(),
        format!(
            "Ambiguous: 2 files share ID 042:\n  {}\n  {}\nRenumber or remove one before \
             continuing; a rewrite keyed on 042 would corrupt the others.",
            dir.path().join("042-one.md"),
            dir.path().join("042-two.md")
        )
    );
}

#[test]
fn the_next_draft_slot_is_the_first_one_free() {
    let dir = Utf8TempDir::new().unwrap();
    assert_eq!(next_draft_slot(dir.path()), Ok(RfdId::Draft(1)));

    fs::create_dir_all(dir.path().join("drafts")).unwrap();
    for name in ["D01-a.md", "D02-b.md", "D04-d.md", "D03-notes.txt"] {
        fs::write(dir.path().join("drafts").join(name), "").unwrap();
    }
    // A draft misplaced among the published RFDs doesn't take a slot.
    fs::write(dir.path().join("D03-misplaced.md"), "").unwrap();

    assert_eq!(next_draft_slot(dir.path()), Ok(RfdId::Draft(3)));
}

#[test]
fn every_draft_slot_taken_is_an_error() {
    let dir = Utf8TempDir::new().unwrap();
    fs::create_dir_all(dir.path().join("drafts")).unwrap();
    for slot in 1..=99 {
        fs::write(dir.path().join(format!("drafts/D{slot:02}-x.md")), "").unwrap();
    }

    assert_eq!(
        next_draft_slot(dir.path()),
        Err("No draft slots available (D01-D99 all in use).".to_owned())
    );
}

/// The lowest gap wins over one past the highest, and the templates' `000`
/// takes no number.
#[test]
fn the_next_number_fills_the_lowest_gap() {
    let dir = Utf8TempDir::new().unwrap();
    assert_eq!(next_number(dir.path()), Ok(RfdId::Published(1)));

    for name in ["000-design-template.md", "001-a.md", "002-b.md", "004-d.md"] {
        fs::write(dir.path().join(name), "").unwrap();
    }
    assert_eq!(next_number(dir.path()), Ok(RfdId::Published(3)));

    fs::write(dir.path().join("003-c.md"), "").unwrap();
    assert_eq!(next_number(dir.path()), Ok(RfdId::Published(5)));
}

#[test]
fn a_slug_keeps_letters_digits_and_dashes() {
    assert_eq!(
        slug("Shareable RFD, Ticket, and Site Tooling"),
        "shareable-rfd-ticket-and-site-tooling"
    );
    assert_eq!(slug("Built-in `ask_user` Tool"), "built-in-ask_user-tool");
    assert_eq!(slug("Déjà Vu  Twice"), "dj-vu--twice");
    assert_eq!(slug("!!!"), "");
}

#[test]
fn resolve_reads_only_markdown_files() {
    let dir = Utf8TempDir::new().unwrap();
    fs::write(dir.path().join("042-notes.txt"), "").unwrap();
    fs::write(dir.path().join("042-rfd.md"), "").unwrap();

    assert_eq!(
        resolve(dir.path(), RfdId::Published(42)).unwrap(),
        dir.path().join("042-rfd.md")
    );
}
