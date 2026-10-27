use std::fs;

use camino::{Utf8Path, Utf8PathBuf};
use camino_tempfile::Utf8TempDir;

use super::*;

/// A workspace with JP's layout: RFDs in `docs/rfd`, drafts in its `drafts/`,
/// and tickets in `docs/ticket`.
pub(super) struct Workspace {
    _dir: Utf8TempDir,
    pub paths: Paths,
}

impl Workspace {
    pub(super) fn new() -> Self {
        let dir = Utf8TempDir::new().unwrap();
        let paths = Paths::new(
            dir.path(),
            Utf8Path::new("docs/rfd"),
            Utf8Path::new("docs/ticket"),
        );
        fs::create_dir_all(paths.drafts()).unwrap();
        fs::create_dir_all(&paths.tickets).unwrap();

        Self { _dir: dir, paths }
    }

    /// Write `text` to `relative`, creating its directory.
    pub(super) fn write(&self, relative: &str, text: &str) -> Utf8PathBuf {
        let path = self.paths.root.join(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, text).unwrap();

        path
    }

    pub(super) fn read(&self, relative: &str) -> String {
        fs::read_to_string(self.paths.root.join(relative))
            .unwrap_or_else(|error| panic!("{relative}: {error}"))
    }

    pub(super) fn exists(&self, relative: &str) -> bool {
        self.paths.root.join(relative).exists()
    }

    /// Write an RFD document named `name` (`042-slug.md` or `D05-slug.md`) into
    /// the directory its id belongs in, returning its path relative to the
    /// root.
    ///
    /// Its title is its slug in words: `Sketch` for `D05-sketch.md`.
    pub(super) fn rfd(&self, name: &str, fields: &[(&str, &str)], body: &str) -> String {
        let (id, slug) = name.trim_end_matches(".md").split_once('-').unwrap();
        let relative = if id.starts_with('D') {
            format!("docs/rfd/drafts/{name}")
        } else {
            format!("docs/rfd/{name}")
        };
        let title = capitalized(&slug.replace('-', " "));
        self.write(&relative, &document(id, &title, fields, body));

        relative
    }

    /// Write the board file.
    pub(super) fn board(&self, json: &str) {
        self.write("docs/rfd/.priority.json", json);
    }
}

/// An RFD document: its heading, a metadata block of `fields`, and `body`.
pub(super) fn document(id: &str, title: &str, fields: &[(&str, &str)], body: &str) -> String {
    let mut text = format!("# RFD {id}: {title}\n\n");
    for (key, value) in fields {
        text.push_str(&format!("- **{key}**: {value}\n"));
    }
    text.push_str("\n## Summary\n\n");
    text.push_str(body);
    if !body.ends_with('\n') {
        text.push('\n');
    }

    text
}

/// A ticket file, as `jp ticket` writes one.
pub(super) fn ticket(
    title: &str,
    status: &str,
    extra: &[(&str, &str)],
    description: &str,
) -> String {
    let mut text = format!(
        "# {title}\n\n- **Status**: {status}\n- **Authors**: Jane <jane@example.com>\n- **Date**: \
         2026-08-05\n"
    );
    for (key, value) in extra {
        text.push_str(&format!("- **{key}**: {value}\n"));
    }
    if !description.is_empty() {
        text.push('\n');
        text.push_str(description);
        text.push('\n');
    }

    text
}

#[test]
fn a_title_has_to_make_a_file_name() {
    assert_eq!(check_title("A Title"), Ok(()));
    assert_eq!(check_title(""), Err("Give the RFD a title.".to_owned()));
    assert_eq!(
        check_title("Two\nLines"),
        Err("A title is one line.".to_owned())
    );
    assert_eq!(
        check_title("!!!"),
        Err("Title '!!!' produces an empty filename slug.".to_owned())
    );
}

#[test]
fn alternatives_read_as_a_sentence() {
    let values = |list: &[&str]| {
        list.iter()
            .map(|value| (*value).to_owned())
            .collect::<Vec<_>>()
    };

    assert_eq!(alternatives(&values(&["design"])), "'design'");
    assert_eq!(
        alternatives(&values(&["design", "guide"])),
        "'design' or 'guide'"
    );
    assert_eq!(
        alternatives(&values(&["design", "decision", "guide"])),
        "'design', 'decision', or 'guide'"
    );
}

#[test]
fn a_category_is_capitalized_for_the_metadata() {
    assert_eq!(capitalized("design"), "Design");
    assert_eq!(capitalized("post-mortem"), "Post-mortem");
    assert_eq!(capitalized(""), "");
}

#[test]
fn leftovers_name_the_files_outside_what_was_covered() {
    let workspace = Workspace::new();
    workspace.write("docs/rfd/042-x.md", "RFD 042");
    workspace.write("crates/lib.rs", "// See RFD 042.");
    let paths = &workspace.paths;
    let leftovers = Leftovers {
        covered: &[&paths.rfds],
        scope: "outside docs/rfd/".to_owned(),
        finding: "still mention 042".to_owned(),
    };

    assert_eq!(
        leftovers.report(paths, true, |text| text.contains("RFD 042")),
        Some(
            "Warning: references outside docs/rfd/ still mention 042:\n  crates/lib.rs".to_owned()
        )
    );
    assert_eq!(leftovers.report(paths, true, |_| false), None);
}

/// A tool whose access policy keeps the workspace out of reach can't search it,
/// and says so rather than reporting nothing found.
#[test]
fn leftovers_not_searched_say_so() {
    let workspace = Workspace::new();
    let paths = &workspace.paths;
    let leftovers = Leftovers {
        covered: &[&paths.rfds],
        scope: "outside docs/rfd/".to_owned(),
        finding: "still mention 042".to_owned(),
    };

    assert_eq!(
        leftovers.report(paths, false, |_| true),
        Some(
            "References outside docs/rfd/ were not checked: the access policy doesn't allow \
             reading the workspace."
                .to_owned()
        )
    );
}
