use camino_tempfile::Utf8TempDir;
use indoc::indoc;
use serde_json::json;

use super::*;

const JP_STYLE: &str = indoc! {"
    max_width = 80
    format_markdown = true
    reference_links = true
    prune_reference_links = true
"};

const TWO_SENTENCES: &str = "A turn that stalls while idle is retried instead of failing. The \
                             retry waits for the provider's own backoff, and gives up after three \
                             attempts, saying why.";

fn workspace() -> Utf8TempDir {
    let root = Utf8TempDir::new().unwrap();
    fs::create_dir(root.path().join("changelog.d")).unwrap();

    root
}

fn fragments(root: &Utf8TempDir) -> Fragments<'_> {
    Fragments::new(root.path(), root.path().join("changelog.d"), &json!({})).unwrap()
}

fn formatted(root: &Utf8TempDir) -> Fragments<'_> {
    fs::write(root.path().join(".comfort.toml"), JP_STYLE).unwrap();
    let config = json!({
        "plugins": { "command": { "release": { "options": { "format_on_write": true } } } }
    });

    Fragments::new(root.path(), root.path().join("changelog.d"), &config).unwrap()
}

#[test]
fn a_fragment_holds_its_entry_and_its_name_its_type() {
    let root = workspace();

    let planned = plan(
        &fragments(&root),
        Type::Fixed,
        "  A turn that stalls while idle is retried instead of failing.\n\n",
        Some("retry"),
    )
    .unwrap();

    assert_eq!(planned.shown, "changelog.d/retry.fixed.md");
    assert_eq!(
        planned.text,
        "A turn that stalls while idle is retried instead of failing.\n"
    );
}

#[test]
fn an_entry_is_written_as_given_unless_the_workspace_formats_on_write() {
    let root = workspace();

    let planned = plan(&fragments(&root), Type::Fixed, TWO_SENTENCES, Some("retry")).unwrap();

    assert_eq!(planned.text, format!("{TWO_SENTENCES}\n"));
}

/// Laid out in the style of the workspace's `.comfort.toml`, so the file passes
/// the workspace's formatting check as it is written.
#[test]
fn an_entry_is_laid_out_in_the_workspaces_style_when_it_formats_on_write() {
    let root = workspace();

    let planned = plan(&formatted(&root), Type::Fixed, TWO_SENTENCES, Some("retry")).unwrap();

    assert_eq!(planned.text, indoc! {"
        A turn that stalls while idle is retried instead of failing.
        The retry waits for the provider's own backoff, and gives up after three
        attempts, saying why.
    "});
}

/// Laying a paragraph out a sentence per line can start a line with the upgrade
/// section's token partway through it, so a file is held to the format as it is
/// laid out.
#[test]
fn an_entry_is_checked_as_it_is_laid_out() {
    let root = workspace();
    let entry = "Retries are fixed. BREAKING CHANGE: the old flag is gone.";

    let error = plan(&formatted(&root), Type::Fixed, entry, None).unwrap_err();

    assert_eq!(
        error,
        "once laid out as the workspace formats it, line 2 starts with `BREAKING CHANGE:` without \
         starting a paragraph; an upgrade section opens a paragraph of its own, after a blank \
         line, and to show the token instead, indent the line or put the token in inline code"
    );
    // Written as given, the token sits partway through a line, where it is
    // text.
    assert!(plan(&fragments(&root), Type::Fixed, entry, None).is_ok());
}

#[test]
fn a_format_on_write_that_is_not_true_or_false_is_refused() {
    let root = workspace();
    let config = json!({
        "plugins": { "command": { "release": { "options": { "format_on_write": "yes" } } } }
    });

    let error = Fragments::new(root.path(), root.path().join("changelog.d"), &config).unwrap_err();

    assert_eq!(
        error,
        "`plugins.command.release.options.format_on_write` must be `true` or `false`."
    );
}

#[test]
fn a_fragment_is_named_after_its_first_words() {
    let root = workspace();

    let planned = plan(
        &fragments(&root),
        Type::Fixed,
        "A turn that stalls while idle is retried instead of failing.",
        None,
    )
    .unwrap();

    assert_eq!(
        planned.shown,
        "changelog.d/a-turn-that-stalls-while-idle-is-retried-instead.fixed.md"
    );
    assert_eq!(
        planned.path,
        root.path()
            .join("changelog.d/a-turn-that-stalls-while-idle-is-retried-instead.fixed.md")
    );
}

/// Markdown between the first words separates them like a space does.
#[test]
fn markdown_does_not_reach_a_derived_slug() {
    assert_eq!(
        derive_slug("`tools.<name>.enable` splits into `state` and `allow_toggle`."),
        Some("tools-name-enable-splits-into-state-and".to_owned())
    );
    assert_eq!(derive_slug("**ÉÉ**"), None);
}

/// The file keeps its links in the reference form the formatter writes, and the
/// slug is named after the entry's words, not its URLs.
#[test]
fn an_entry_with_links_is_named_after_its_words() {
    let root = workspace();

    let planned = plan(
        &formatted(&root),
        Type::Fixed,
        "See [the docs](https://example.com/docs) for setup.",
        None,
    )
    .unwrap();

    assert_eq!(planned.shown, "changelog.d/see-the-docs-for-setup.fixed.md");
    assert_eq!(planned.text, indoc! {"
        See [the docs] for setup.

        [the docs]: https://example.com/docs
    "});
}

/// A block break separates words, and markup inside a word doesn't split it.
#[test]
fn plain_text_reads_words_as_they_render() {
    assert_eq!(
        derive_slug(&plain_text("One\n\nTwo, then **re**try.")),
        Some("one-two-then-retry".to_owned())
    );
}

/// A first word longer than a slug may be is cut, rather than leaving the entry
/// without a name.
#[test]
fn a_long_first_word_is_cut() {
    let word = "a".repeat(80);

    assert_eq!(derive_slug(&word), Some("a".repeat(MAX_DERIVED_SLUG)));
}

/// Only a fragment of the same type takes the name: a slug can name one
/// fragment of each type.
#[test]
fn a_derived_name_a_fragment_of_the_type_has_gets_a_number() {
    let root = workspace();
    for taken in [
        "crashes-are-fixed.fixed.md",
        "crashes-are-fixed-2.fixed.md",
        "crashes-are-fixed-3.added.md",
    ] {
        fs::write(root.path().join("changelog.d").join(taken), "").unwrap();
    }

    let planned = plan(&fragments(&root), Type::Fixed, "Crashes are fixed.", None).unwrap();

    assert_eq!(planned.shown, "changelog.d/crashes-are-fixed-3.fixed.md");
}

#[test]
fn a_named_slug_a_fragment_of_the_type_has_is_refused() {
    let root = workspace();
    fs::write(root.path().join("changelog.d/retry.fixed.md"), "").unwrap();

    let error = plan(
        &fragments(&root),
        Type::Fixed,
        "Crashes are fixed.",
        Some("retry"),
    )
    .unwrap_err();

    assert_eq!(
        error,
        "changelog.d/retry.fixed.md already exists; choose another slug."
    );
}

/// The type in a fragment's name keeps every slug clear of `README.md` and a
/// release's `overview.md`.
#[test]
fn a_named_slug_is_used_as_given() {
    let root = workspace();

    for (slug, shown) in [
        ("Retry_Idle-2", "changelog.d/Retry_Idle-2.fixed.md"),
        ("readme", "changelog.d/readme.fixed.md"),
        ("overview", "changelog.d/overview.fixed.md"),
    ] {
        let planned = plan(
            &fragments(&root),
            Type::Fixed,
            "Crashes are fixed.",
            Some(slug),
        )
        .unwrap();

        assert_eq!(planned.shown, shown);
    }
}

/// A slug is a plain file name: one that reaches another directory, or one with
/// a `.` that would read as part of the type, would break the layout.
#[test]
fn a_slug_that_is_not_a_plain_name_is_refused() {
    let root = workspace();

    for slug in ["../escape", "retry.md", "retry.added", "two words", ""] {
        let error = plan(
            &fragments(&root),
            Type::Fixed,
            "Crashes are fixed.",
            Some(slug),
        )
        .unwrap_err();

        assert_eq!(
            error,
            format!("`{slug}` can't name a file here: use ASCII letters, digits, `-`, and `_`.")
        );
    }
}

/// What the tool writes passes `jp release check`, so an entry breaking the
/// format is refused rather than written.
#[test]
fn an_entry_that_breaks_the_format_is_refused() {
    let root = workspace();
    let entry = indoc! {"
        An example of the token:

        ```text
        BREAKING CHANGE: in an example
        ```
    "};

    let error = plan(&fragments(&root), Type::Fixed, entry, None).unwrap_err();

    assert_eq!(
        error,
        "line 4 starts with `BREAKING CHANGE:` inside a code block; to show the token, indent the \
         line or put the token in inline code"
    );
}

#[test]
fn an_empty_entry_is_refused() {
    let root = workspace();

    let error = plan(&fragments(&root), Type::Fixed, "  \n", None).unwrap_err();

    assert_eq!(error, "no entry: say what changed for users");
}

#[test]
fn there_has_to_be_a_fragments_directory() {
    let root = Utf8TempDir::new().unwrap();

    let error = plan(&fragments(&root), Type::Fixed, "Crashes are fixed.", None).unwrap_err();

    assert_eq!(
        error,
        "There is no fragments directory at changelog.d; create it with `jp release init`."
    );
}

#[test]
fn a_planned_fragment_passes_the_check() {
    let root = workspace();

    let planned = plan(&fragments(&root), Type::Added, "Exports work.", None).unwrap();
    planned.write().unwrap();

    assert_eq!(fs::read_to_string(&planned.path).unwrap(), planned.text);
    let report = changelog::check(&root.path().join("changelog.d")).unwrap();
    assert_eq!(report.fragments, 1);
    assert!(report.problems.is_empty(), "{report}");
}

#[test]
fn writing_refuses_a_file_that_appeared_since_it_was_planned() {
    let root = workspace();
    let planned = plan(&fragments(&root), Type::Fixed, "Crashes are fixed.", None).unwrap();
    fs::write(&planned.path, "Someone else's.\n").unwrap();

    let error = planned.write().unwrap_err();

    assert_eq!(
        error,
        "changelog.d/crashes-are-fixed.fixed.md already exists; choose another slug."
    );
    assert_eq!(
        fs::read_to_string(&planned.path).unwrap(),
        "Someone else's.\n"
    );
}

#[test]
fn a_branch_names_a_file_by_its_last_part() {
    assert_eq!(
        branch_slug("jean/fix-idle-turns"),
        Some("fix-idle-turns".to_owned())
    );
    assert_eq!(
        branch_slug("Export_Conversations"),
        Some("export_conversations".to_owned())
    );
    assert_eq!(branch_slug("feature/v1.2"), Some("v1-2".to_owned()));
}

/// The default branch holds every change, so no file is named after it.
#[test]
fn the_default_branch_names_no_file() {
    assert_eq!(branch_slug("main"), None);
    assert_eq!(branch_slug("jean/🔥"), None);
}
