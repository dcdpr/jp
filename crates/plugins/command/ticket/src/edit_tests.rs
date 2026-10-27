use std::{collections::VecDeque, io};

use camino::Utf8PathBuf;
use camino_tempfile::Utf8TempDir;
use serde_json::{Value, json};
use ticket::{Labels, NewTicket, render};

use super::*;

const ID: &str = "T-02wt0kx";

/// Plays the user: each `open` leaves the next scripted content in the file,
/// and each question is answered from a script.
#[derive(Default)]
struct Scripted {
    /// What the editor leaves in the file, if it writes at all, and whether it
    /// exits cleanly.
    edits: VecDeque<(Option<String>, bool)>,

    /// The answers to "open the editor again?".
    again: VecDeque<bool>,

    /// The problems the user was asked about, in order.
    asked: Vec<String>,
}

impl Scripted {
    fn editing(edits: impl IntoIterator<Item = (Option<String>, bool)>) -> Self {
        Self {
            edits: edits.into_iter().collect(),
            ..Self::default()
        }
    }

    fn answering(mut self, again: impl IntoIterator<Item = bool>) -> Self {
        self.again = again.into_iter().collect();
        self
    }
}

impl Editing for Scripted {
    fn open(&mut self, path: &Utf8Path) -> Result<bool, String> {
        let (content, saved) = self
            .edits
            .pop_front()
            .expect("the editor was opened more often than scripted");
        if let Some(content) = content {
            fs::write(path, content).unwrap();
        }

        Ok(saved)
    }

    fn again(&mut self, problem: &str) -> bool {
        self.asked.push(problem.to_owned());
        self.again
            .pop_front()
            .expect("asked more often than scripted")
    }
}

/// A board that takes `client=cli` and `client=macos`.
fn options() -> Options {
    options_with(&json!({
        "client": { "description": "The client.", "values": ["cli", "macos"] }
    }))
}

fn options_with(labels: &Value) -> Options {
    let Value::Object(options) = json!({ "labels": labels }) else {
        unreachable!("options are an object");
    };

    Options::read(&options).unwrap()
}

/// File ticket [`ID`] under a name its title doesn't produce, returning the
/// path and what it holds.
fn write_ticket(dir: &Utf8TempDir) -> (Utf8PathBuf, String) {
    let id: TicketId = ID.parse().unwrap();
    let path = dir.path().join(format!("{}slug.md", id.file_prefix()));
    let source = render::ticket(&NewTicket {
        title: "Wrapping is wrong",
        authors: "jp",
        date: "2026-08-05",
        implements: None,
        labels: &Labels::default(),
        description: "Something is wrong.",
    });
    fs::write(&path, &source).unwrap();

    (path, source)
}

fn id() -> TicketId {
    ID.parse().unwrap()
}

#[test]
fn a_saved_edit_is_kept_where_it_is() {
    let dir = Utf8TempDir::new().unwrap();
    let (path, original) = write_ticket(&dir);
    let edited = original.replace("Something is wrong.", "Off by one at 80 columns.");
    let mut editing = Scripted::editing([(Some(edited.clone()), true)]);

    let output = in_editor(dir.path(), id(), &options(), &mut editing).unwrap();

    assert_eq!(output.text, format!("Edited {path}\n"));
    assert_eq!(fs::read_to_string(&path).unwrap(), edited);
}

/// A new heading takes the file name with it, as `rename` does.
#[test]
fn a_new_title_moves_the_file() {
    let dir = Utf8TempDir::new().unwrap();
    let (path, original) = write_ticket(&dir);
    let edited = original.replace("# Wrapping is wrong", "# The wrap calculation is off");
    let mut editing = Scripted::editing([(Some(edited.clone()), true)]);

    let output = in_editor(dir.path(), id(), &options(), &mut editing).unwrap();

    let moved = dir.path().join("02wt0kx-the-wrap-calculation-is-off.md");
    assert_eq!(
        output.text,
        format!("Edited {moved}, renamed from {path}\n")
    );
    assert_eq!(fs::read_to_string(&moved).unwrap(), edited);
    assert!(!path.exists());
}

#[test]
fn an_untouched_file_is_reported_unchanged() {
    let dir = Utf8TempDir::new().unwrap();
    let (path, _) = write_ticket(&dir);
    let mut editing = Scripted::editing([(None, true)]);

    let output = in_editor(dir.path(), id(), &options(), &mut editing).unwrap();

    assert_eq!(output.text, format!("{path}: unchanged\n"));
}

/// An editor exiting with an error, as `:cq` makes Vim do, abandons the edit
/// even if it wrote the file first.
#[test]
fn an_edit_abandoned_in_the_editor_is_undone() {
    let dir = Utf8TempDir::new().unwrap();
    let (path, original) = write_ticket(&dir);
    let mut editing = Scripted::editing([(Some("half an edit".to_owned()), false)]);

    let error = in_editor(dir.path(), id(), &options(), &mut editing).unwrap_err();

    assert_eq!(
        error,
        "The editor exited with an error; T-02wt0kx is unchanged."
    );
    assert_eq!(fs::read_to_string(&path).unwrap(), original);
}

#[test]
fn a_file_that_no_longer_reads_as_a_ticket_goes_back_to_the_editor() {
    let dir = Utf8TempDir::new().unwrap();
    let (path, original) = write_ticket(&dir);
    let fixed = original.replace("Something is wrong.", "Off by one at 80 columns.");
    let mut editing = Scripted::editing([
        (Some("no heading here\n".to_owned()), true),
        (Some(fixed.clone()), true),
    ])
    .answering([true]);

    let output = in_editor(dir.path(), id(), &options(), &mut editing).unwrap();

    assert_eq!(output.text, format!("Edited {path}\n"));
    assert_eq!(fs::read_to_string(&path).unwrap(), fixed);
    let [asked] = editing.asked.as_slice() else {
        panic!("expected one question, got {:?}", editing.asked);
    };
    assert!(
        asked.starts_with("T-02wt0kx can't be saved: Ticket does not open with"),
        "{asked}"
    );
}

#[test]
fn an_edit_given_up_on_is_undone() {
    let dir = Utf8TempDir::new().unwrap();
    let (path, original) = write_ticket(&dir);
    let mut editing =
        Scripted::editing([(Some("no heading here\n".to_owned()), true)]).answering([false]);

    let error = in_editor(dir.path(), id(), &options(), &mut editing).unwrap_err();

    assert!(
        error.starts_with("T-02wt0kx is unchanged: Ticket does not open with"),
        "{error}"
    );
    assert_eq!(fs::read_to_string(&path).unwrap(), original);
}

/// A label typed into the file is held to the board's labels, as one passed
/// with `--label` is.
#[test]
fn a_label_the_board_refuses_goes_back_to_the_editor() {
    let dir = Utf8TempDir::new().unwrap();
    let (path, original) = write_ticket(&dir);
    let edited = original.replace(
        "- **Status**: Todo\n",
        "- **Status**: Todo\n- **Label**: client=windows\n",
    );
    assert_ne!(edited, original, "the fixture has no status line to follow");
    let mut editing = Scripted::editing([(Some(edited), true)]).answering([false]);

    let error = in_editor(dir.path(), id(), &options(), &mut editing).unwrap_err();

    assert!(error.contains("client=windows"), "{error}");
    assert_eq!(fs::read_to_string(&path).unwrap(), original);
}

/// A label the board lets one ticket carry, added by hand while another ticket
/// holds it, goes back to the editor naming that ticket.
#[test]
fn a_label_another_ticket_holds_goes_back_to_the_editor() {
    let dir = Utf8TempDir::new().unwrap();
    let (path, original) = write_ticket(&dir);
    let holder: TicketId = "T-02wt0k1".parse().unwrap();
    let held = Labels::from_tokens(["rfd=116"]).unwrap();
    fs::write(
        dir.path().join(format!("{}held.md", holder.file_prefix())),
        render::ticket(&NewTicket {
            title: "Tracks 116",
            authors: "jp",
            date: "2026-08-05",
            implements: None,
            labels: &held,
            description: "",
        }),
    )
    .unwrap();
    let edited = original.replace(
        "- **Status**: Todo\n",
        "- **Status**: Todo\n- **Label**: rfd=116\n",
    );
    let mut editing = Scripted::editing([(Some(edited), true)]).answering([false]);
    let options = options_with(&json!({
        "rfd": { "description": "The RFD.", "values": [], "unique": "key-value" }
    }));

    let error = in_editor(dir.path(), id(), &options, &mut editing).unwrap_err();

    assert_eq!(editing.asked, [format!(
        "{ID} can't be saved: `rfd=116` is already on {holder}, and the board lets one ticket \
         carry each `rfd` value."
    )]);
    assert!(
        error.starts_with(&format!("{ID} is unchanged: ")),
        "{error}"
    );
    assert_eq!(fs::read_to_string(&path).unwrap(), original);
}

/// Labels left as they were don't read the vocabulary, so a board whose
/// vocabulary is broken can still have its descriptions edited.
#[test]
fn an_edit_that_leaves_the_labels_alone_needs_no_vocabulary() {
    let dir = Utf8TempDir::new().unwrap();
    let (path, original) = write_ticket(&dir);
    let edited = original.replace("Something is wrong.", "Off by one at 80 columns.");
    let mut editing = Scripted::editing([(Some(edited), true)]);

    let output = in_editor(
        dir.path(),
        id(),
        &options_with(&json!("not a map")),
        &mut editing,
    )
    .unwrap();

    assert_eq!(output.text, format!("Edited {path}\n"));
}

#[test]
fn an_older_host_is_told_to_upgrade() {
    let dir = Utf8TempDir::new().unwrap();
    write_ticket(&dir);

    let error = through_host(
        12,
        dir.path(),
        id(),
        &options(),
        &mut io::empty(),
        &mut Vec::new(),
    )
    .unwrap_err();

    assert_eq!(
        error,
        "Opening an editor needs `jp` protocol 13, and this `jp` speaks 12; reinstall the two \
         together. Pass what to change instead: --title, --body, --status, or --label."
    );
}
