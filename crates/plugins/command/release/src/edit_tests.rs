use std::collections::VecDeque;

use camino_tempfile::Utf8TempDir;
use indoc::indoc;
use serde_json::json;

use super::*;

/// The style JP's `.comfort.toml` sets.
const JP_STYLE: &str = indoc! {"
    max_width = 80
    format_markdown = true
    reference_links = true
    prune_reference_links = true
"};

/// A highlight that follows the format.
const HIGHLIGHT: &str = indoc! {"
    # Tool calls fan out

    - **Authors**: @JeanMertz

    The assistant calls one tool on many inputs at once.
"};

/// Plays the user: each `open` leaves the next scripted content in the file,
/// and each question is answered from a script.
#[derive(Default)]
struct Scripted {
    /// What the editor leaves in the file, if it writes at all, and whether it
    /// exits cleanly.
    edits: VecDeque<(Option<&'static str>, bool)>,

    /// The answers to "open the editor again?".
    again: VecDeque<bool>,

    /// The problems the user was asked about, in order.
    asked: Vec<String>,

    /// Each file the editor opened, and what it held then.
    opened: Vec<(Utf8PathBuf, String)>,
}

impl Scripted {
    fn editing(edits: impl IntoIterator<Item = (Option<&'static str>, bool)>) -> Self {
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
        self.opened
            .push((path.to_owned(), fs::read_to_string(path).unwrap()));
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

/// A workspace whose fragments directory, `changelog.d`, exists.
fn workspace() -> Utf8TempDir {
    let root = Utf8TempDir::new().unwrap();
    fs::create_dir(root.path().join("changelog.d")).unwrap();

    root
}

/// `root`'s fragments directory, writing files as they are given.
fn fragments(root: &Utf8TempDir) -> Fragments<'_> {
    Fragments::new(root.path(), root.path().join("changelog.d"), &json!({})).unwrap()
}

/// `root`'s fragments directory, laying files out in JP's style as they are
/// written.
fn formatted(root: &Utf8TempDir) -> Fragments<'_> {
    fs::write(root.path().join(".comfort.toml"), JP_STYLE).unwrap();
    let config = json!({
        "plugins": { "command": { "release": { "options": { "format_on_write": true } } } }
    });

    Fragments::new(root.path(), root.path().join("changelog.d"), &config).unwrap()
}

fn read(root: &Utf8TempDir, path: &str) -> Option<String> {
    fs::read_to_string(root.path().join(path)).ok()
}

fn text(output: Result<Output, String>) -> String {
    output.unwrap().text
}

#[test]
fn a_new_fragment_is_written_as_the_editor_leaves_it() {
    let root = workspace();
    let mut editor = Scripted::editing([(Some("Idle turns are retried.\n"), true)]);

    let output = fragment(&fragments(&root), Type::Fixed, "retry", &mut editor);

    assert_eq!(text(output), "Wrote changelog.d/retry.fixed.md\n");
    assert_eq!(editor.opened, [(
        root.path().join("changelog.d/retry.fixed.md"),
        String::new()
    )]);
    assert_eq!(
        read(&root, "changelog.d/retry.fixed.md").as_deref(),
        Some("Idle turns are retried.\n")
    );
}

#[test]
fn a_new_fragment_left_empty_writes_nothing() {
    let root = workspace();
    let mut editor = Scripted::editing([(Some("  \n"), true)]);

    let output = fragment(&fragments(&root), Type::Fixed, "retry", &mut editor);

    assert_eq!(text(output), "Nothing written.\n");
    assert_eq!(read(&root, "changelog.d/retry.fixed.md"), None);
}

#[test]
fn an_existing_fragment_opens_for_another_edit() {
    let root = workspace();
    fs::write(root.path().join("changelog.d/retry.fixed.md"), "Old.\n").unwrap();
    let mut editor = Scripted::editing([(Some("New.\n"), true)]);

    let output = fragment(&fragments(&root), Type::Fixed, "retry", &mut editor);

    assert_eq!(text(output), "Edited changelog.d/retry.fixed.md\n");
    assert_eq!(editor.opened[0].1, "Old.\n");
    assert_eq!(
        read(&root, "changelog.d/retry.fixed.md").as_deref(),
        Some("New.\n")
    );
}

/// A slug names one fragment of each type, so another type's fragment is a file
/// of its own.
#[test]
fn a_slug_names_one_fragment_of_each_type() {
    let root = workspace();
    fs::write(root.path().join("changelog.d/export.added.md"), "Old.\n").unwrap();
    let mut editor = Scripted::editing([(Some("Changed too.\n"), true)]);

    let output = fragment(&fragments(&root), Type::Changed, "export", &mut editor);

    assert_eq!(text(output), "Wrote changelog.d/export.changed.md\n");
    assert_eq!(
        read(&root, "changelog.d/export.added.md").as_deref(),
        Some("Old.\n")
    );
}

#[test]
fn an_existing_fragment_left_as_it_was_is_unchanged() {
    let root = workspace();
    fs::write(root.path().join("changelog.d/retry.fixed.md"), "Old.\n").unwrap();
    let mut editor = Scripted::editing([(None, true)]);

    let output = fragment(&fragments(&root), Type::Fixed, "retry", &mut editor);

    assert_eq!(text(output), "changelog.d/retry.fixed.md: unchanged\n");
}

#[test]
fn an_unchanged_fragment_that_breaks_the_format_says_so() {
    let root = workspace();
    let path = root.path().join("changelog.d/retry.fixed.md");
    fs::write(&path, "- **Type**: fixed\n\nOld.\n").unwrap();
    let mut editor = Scripted::editing([(None, true)]);

    let error = fragment(&fragments(&root), Type::Fixed, "retry", &mut editor).unwrap_err();

    assert_eq!(
        error,
        "changelog.d/retry.fixed.md is unchanged, and still breaks the format: line 1 names a \
         type, which a fragment's file name holds: `<slug>.<type>.md`"
    );
}

#[test]
fn an_existing_fragment_saved_empty_is_removed() {
    let root = workspace();
    fs::write(root.path().join("changelog.d/retry.fixed.md"), "Old.\n").unwrap();
    let mut editor = Scripted::editing([(Some(""), true)]);

    let output = fragment(&fragments(&root), Type::Fixed, "retry", &mut editor);

    assert_eq!(
        text(output),
        "Removed changelog.d/retry.fixed.md, which was saved empty.\n"
    );
    assert_eq!(read(&root, "changelog.d/retry.fixed.md"), None);
}

#[test]
fn a_fragment_breaking_the_format_opens_again_until_it_follows_it() {
    let root = workspace();
    let mut editor = Scripted::editing([
        (Some("Entry.\n\nBREAKING CHANGE: Title\n"), true),
        (Some("Entry.\n\nBREAKING CHANGE: Title\n\nBody.\n"), true),
    ])
    .answering([true]);

    let output = fragment(&fragments(&root), Type::Changed, "flag", &mut editor);

    assert_eq!(text(output), "Wrote changelog.d/flag.changed.md\n");
    assert_eq!(editor.asked, ["changelog.d/flag.changed.md: the upgrade \
                               section on line 3 has no body: say what \
                               changed and how to upgrade in the \
                               paragraphs after its title"]);
    // The second open finds what the first left.
    assert_eq!(editor.opened[1].1, "Entry.\n\nBREAKING CHANGE: Title\n");
}

#[test]
fn a_new_fragment_given_up_on_is_removed() {
    let root = workspace();
    let mut editor =
        Scripted::editing([(Some("Entry.\n\nBREAKING CHANGE: Title\n"), true)]).answering([false]);

    let error = fragment(&fragments(&root), Type::Changed, "flag", &mut editor).unwrap_err();

    assert_eq!(
        error,
        "Nothing written: the upgrade section on line 3 has no body: say what changed and how to \
         upgrade in the paragraphs after its title"
    );
    assert_eq!(read(&root, "changelog.d/flag.changed.md"), None);
}

#[test]
fn an_existing_fragment_given_up_on_is_restored() {
    let root = workspace();
    fs::write(root.path().join("changelog.d/retry.fixed.md"), "Old.\n").unwrap();
    let mut editor =
        Scripted::editing([(Some("- **Type**: fixed\n\nNew.\n"), true)]).answering([false]);

    let error = fragment(&fragments(&root), Type::Fixed, "retry", &mut editor).unwrap_err();

    assert_eq!(
        error,
        "changelog.d/retry.fixed.md is unchanged: line 1 names a type, which a fragment's file \
         name holds: `<slug>.<type>.md`"
    );
    assert_eq!(
        read(&root, "changelog.d/retry.fixed.md").as_deref(),
        Some("Old.\n")
    );
}

/// An editor that exits with an error is how most report an edit abandoned,
/// such as `:cq` in Vim.
#[test]
fn an_editor_that_fails_writes_nothing() {
    let root = workspace();
    let mut editor = Scripted::editing([(Some("Half a thought.\n"), false)]);

    let error = fragment(&fragments(&root), Type::Fixed, "retry", &mut editor).unwrap_err();

    assert_eq!(error, "Nothing written: the editor exited with an error");
    assert_eq!(read(&root, "changelog.d/retry.fixed.md"), None);
}

#[test]
fn an_accepted_fragment_is_laid_out_in_the_workspaces_style() {
    let root = workspace();
    let mut editor = Scripted::editing([(
        Some(
            "A turn that stalls while idle is retried instead of failing. The retry waits for the \
             provider's own backoff.\n",
        ),
        true,
    )]);

    let output = fragment(&formatted(&root), Type::Fixed, "retry", &mut editor);

    assert_eq!(text(output), "Wrote changelog.d/retry.fixed.md\n");
    assert_eq!(
        read(&root, "changelog.d/retry.fixed.md").as_deref(),
        Some(indoc! {"
            A turn that stalls while idle is retried instead of failing.
            The retry waits for the provider's own backoff.
        "})
    );
}

#[test]
fn a_fragments_slug_has_to_be_a_plain_name() {
    let root = workspace();
    let mut editor = Scripted::default();

    let error = fragment(&fragments(&root), Type::Fixed, "../escape", &mut editor).unwrap_err();

    assert_eq!(
        error,
        "`../escape` can't name a file here: use ASCII letters, digits, `-`, and `_`."
    );
    assert!(editor.opened.is_empty());
}

#[test]
fn there_has_to_be_a_fragments_directory() {
    let root = Utf8TempDir::new().unwrap();
    let mut editor = Scripted::default();

    let error = fragment(&fragments(&root), Type::Fixed, "retry", &mut editor).unwrap_err();

    assert_eq!(
        error,
        "There is no fragments directory at changelog.d; create it with `jp release init`."
    );
}

/// A new highlight is drafted outside the fragments directory, and filed under
/// the slug its title gives.
#[test]
fn a_new_highlight_is_named_after_its_title() {
    let root = workspace();
    let mut editor = Scripted::editing([(Some(HIGHLIGHT), true)]);

    let output = highlight(&fragments(&root), None, Some("@JeanMertz"), &mut editor);

    assert_eq!(
        text(output),
        "Wrote changelog.d/highlights/tool-calls-fan-out.md\n"
    );
    assert_eq!(
        read(&root, "changelog.d/highlights/tool-calls-fan-out.md").as_deref(),
        Some(HIGHLIGHT)
    );
    let (draft, opened_with) = &editor.opened[0];
    assert_eq!(opened_with, "# \n\n- **Authors**: @JeanMertz\n\n");
    assert!(!draft.starts_with(root.path()), "{draft}");
    assert!(!draft.exists(), "the draft stays behind at {draft}");
}

#[test]
fn a_new_highlight_left_as_it_opened_writes_nothing() {
    let root = workspace();
    let mut editor = Scripted::editing([(None, true)]);

    let output = highlight(&fragments(&root), None, None, &mut editor);

    assert_eq!(text(output), "Nothing written.\n");
    assert_eq!(editor.opened[0].1, "# \n\n- **Authors**: \n\n");
    assert!(!root.path().join("changelog.d/highlights").exists());
    assert!(!editor.opened[0].0.exists());
}

#[test]
fn a_new_highlight_given_up_on_is_not_filed() {
    let root = workspace();
    let mut editor = Scripted::editing([(Some("# Fan out\n\nThe assistant fans out.\n"), true)])
        .answering([false]);

    let error = highlight(&fragments(&root), None, None, &mut editor).unwrap_err();

    assert_eq!(
        error,
        "Nothing written: no authors: list each person to credit for the feature below the title, \
         as `- **Authors**: @JeanMertz`"
    );
    assert_eq!(editor.asked.len(), 1);
    assert!(editor.asked[0].starts_with("The new highlight: no authors:"));
    assert!(!root.path().join("changelog.d/highlights").exists());
    assert!(!editor.opened[0].0.exists());
}

#[test]
fn a_new_highlights_title_another_has_gets_a_number() {
    let root = workspace();
    fs::create_dir(root.path().join("changelog.d/highlights")).unwrap();
    fs::write(
        root.path()
            .join("changelog.d/highlights/tool-calls-fan-out.md"),
        HIGHLIGHT,
    )
    .unwrap();
    let mut editor = Scripted::editing([(Some(HIGHLIGHT), true)]);

    let output = highlight(&fragments(&root), None, None, &mut editor);

    assert_eq!(
        text(output),
        "Wrote changelog.d/highlights/tool-calls-fan-out-2.md\n"
    );
}

/// A named highlight is written where its name says, whatever its title.
#[test]
fn a_named_highlight_opens_in_place() {
    let root = workspace();
    let mut editor = Scripted::editing([
        (Some(HIGHLIGHT), true),
        (
            Some("# Fan out\n\n- **Authors**: @JeanMertz\n\nProse.\n"),
            true,
        ),
    ]);

    let wrote = highlight(&fragments(&root), Some("fan-out"), None, &mut editor);
    let edited = highlight(&fragments(&root), Some("fan-out"), None, &mut editor);

    assert_eq!(text(wrote), "Wrote changelog.d/highlights/fan-out.md\n");
    assert_eq!(text(edited), "Edited changelog.d/highlights/fan-out.md\n");
    assert_eq!(
        editor.opened[0],
        (
            root.path().join("changelog.d/highlights/fan-out.md"),
            "# \n\n- **Authors**: \n\n".to_owned()
        )
    );
    assert_eq!(editor.opened[1].1, HIGHLIGHT);
}

#[test]
fn an_old_host_cannot_open_an_editor() {
    assert_eq!(check_host(13), Ok(()));
    assert_eq!(
        check_host(12),
        Err(
            "Opening an editor needs `jp` protocol 13, and this `jp` speaks 12; reinstall the two \
             together."
                .to_owned()
        )
    );
}
