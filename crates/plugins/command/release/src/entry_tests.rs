use camino_tempfile::Utf8TempDir;
use indoc::indoc;

use super::*;

/// A workspace whose fragments directory, `changelog.d`, exists.
fn workspace() -> Utf8TempDir {
    let root = Utf8TempDir::new().unwrap();
    fs::create_dir(root.path().join("changelog.d")).unwrap();

    root
}

/// `root`'s fragments directory.
fn fragments(root: &Utf8TempDir) -> Fragments<'_> {
    Fragments {
        root: root.path(),
        dir: root.path().join("changelog.d"),
    }
}

fn fixed(body: &str) -> Entry {
    Entry::Fragment {
        type_: Type::Fixed,
        body: body.to_owned(),
    }
}

fn highlight(title: &str, body: &str) -> Entry {
    Entry::Highlight {
        title: title.to_owned(),
        authors: vec!["@JeanMertz".to_owned()],
        body: body.to_owned(),
    }
}

#[test]
fn a_fragment_is_written_as_its_frontmatter_then_its_entry() {
    let entry = fixed("  A turn that stalls while idle is retried instead of failing.\n\n");

    assert_eq!(entry.render(), indoc! {"
        ---
        type: fixed
        ---
        A turn that stalls while idle is retried instead of failing.
    "});
}

/// Laid out the way `comfort` lays out Markdown, so the file passes a
/// formatting check as it is written.
#[test]
fn an_entry_is_laid_out_one_sentence_per_line() {
    let entry = fixed(
        "A turn that stalls while idle is retried instead of failing. The retry waits for the \
         provider's own backoff, and gives up after three attempts, saying why.",
    );

    assert_eq!(entry.render(), indoc! {"
        ---
        type: fixed
        ---
        A turn that stalls while idle is retried instead of failing.
        The retry waits for the provider's own backoff, and gives up after three
        attempts, saying why.
    "});
}

/// The title and the authors are written as quoted strings, so neither a colon
/// nor a quote in them can change what the YAML says.
#[test]
fn a_highlight_quotes_its_title_and_authors() {
    let entry = Entry::Highlight {
        title: "Tool calls \"fan out\": many inputs, one request".to_owned(),
        authors: vec!["@JeanMertz".to_owned(), " Jane Doe ".to_owned()],
        body: "The assistant calls one tool on many inputs at once.".to_owned(),
    };

    assert_eq!(entry.render(), indoc! {r#"
        ---
        title: "Tool calls \"fan out\": many inputs, one request"
        authors: ["@JeanMertz", "Jane Doe"]
        ---
        The assistant calls one tool on many inputs at once.
    "#});
}

#[test]
fn a_fragment_is_named_after_its_first_words() {
    let root = workspace();

    let planned = plan(
        &fragments(&root),
        &fixed("A turn that stalls while idle is retried instead of failing."),
        None,
    )
    .unwrap();

    assert_eq!(
        planned.shown,
        "changelog.d/a-turn-that-stalls-while-idle-is-retried-instead.md"
    );
    assert_eq!(
        planned.path,
        root.path()
            .join("changelog.d/a-turn-that-stalls-while-idle-is-retried-instead.md")
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
        &fragments(&root),
        &fixed("See [the docs](https://example.com/docs) for setup."),
        None,
    )
    .unwrap();

    assert_eq!(planned.shown, "changelog.d/see-the-docs-for-setup.md");
    assert_eq!(planned.text, indoc! {"
        ---
        type: fixed
        ---
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

#[test]
fn a_highlight_is_named_after_its_title_in_highlights() {
    let root = workspace();

    let planned = plan(
        &fragments(&root),
        &highlight(
            "Tool calls fan out",
            "The assistant calls one tool at once.",
        ),
        None,
    )
    .unwrap();

    assert_eq!(
        planned.shown,
        "changelog.d/highlights/tool-calls-fan-out.md"
    );
}

#[test]
fn a_derived_name_another_file_has_gets_a_number() {
    let root = workspace();
    fs::write(root.path().join("changelog.d/crashes-are-fixed.md"), "").unwrap();
    fs::write(root.path().join("changelog.d/crashes-are-fixed-2.md"), "").unwrap();

    let planned = plan(&fragments(&root), &fixed("Crashes are fixed."), None).unwrap();

    assert_eq!(planned.shown, "changelog.d/crashes-are-fixed-3.md");
}

#[test]
fn a_named_slug_another_file_has_is_refused() {
    let root = workspace();
    fs::write(root.path().join("changelog.d/retry.md"), "").unwrap();

    let error = plan(
        &fragments(&root),
        &fixed("Crashes are fixed."),
        Some("retry"),
    )
    .unwrap_err();

    assert_eq!(
        error,
        "changelog.d/retry.md already exists; choose another slug."
    );
}

#[test]
fn a_named_slug_is_used_as_given() {
    let root = workspace();

    let planned = plan(
        &fragments(&root),
        &fixed("Crashes are fixed."),
        Some("Retry_Idle-2"),
    )
    .unwrap();

    assert_eq!(planned.shown, "changelog.d/Retry_Idle-2.md");
}

#[test]
fn a_reserved_slug_is_refused_in_any_letter_case() {
    let root = workspace();

    let error = plan(
        &fragments(&root),
        &fixed("Crashes are fixed."),
        Some("Overview"),
    )
    .unwrap_err();

    assert_eq!(
        error,
        "`Overview` is reserved for a release's introduction, `overview.md` in the release's \
         directory, in any letter case; choose another slug."
    );
}

#[test]
fn an_entry_whose_name_would_be_reserved_is_refused() {
    let root = workspace();

    let error = plan(
        &fragments(&root),
        &highlight("Readme", "The README is clearer."),
        None,
    )
    .unwrap_err();

    assert_eq!(
        error,
        "The entry would be named `readme`, which is reserved for the fragments directory's \
         documentation, `README.md` at its top level; give it another slug."
    );
}

/// A slug is a plain file name: one that reaches another directory, or one with
/// an extension of its own, would break the layout.
#[test]
fn a_slug_that_is_not_a_plain_name_is_refused() {
    let root = workspace();

    for slug in ["../escape", "retry.md", "two words", ""] {
        let error = plan(&fragments(&root), &fixed("Crashes are fixed."), Some(slug)).unwrap_err();

        assert_eq!(
            error,
            format!("`{slug}` can't name a file here: use ASCII letters, digits, `-`, and `_`.")
        );
    }
}

/// What `add` writes passes `jp release check`, so an entry breaking the format
/// is refused rather than written.
#[test]
fn an_entry_that_breaks_the_format_is_refused() {
    let root = workspace();
    let entry = fixed(indoc! {"
        An example of the token:

        ```text
        BREAKING CHANGE: in an example
        ```
    "});

    let error = plan(&fragments(&root), &entry, None).unwrap_err();

    assert_eq!(
        error,
        "line 7 starts with `BREAKING CHANGE:` inside a code block; to show the token, indent the \
         line or put the token in inline code"
    );
}

#[test]
fn an_empty_entry_is_refused() {
    let root = workspace();

    let error = plan(&fragments(&root), &fixed("  \n"), None).unwrap_err();

    assert_eq!(
        error,
        "no entry: say below the frontmatter what changed for users"
    );
}

#[test]
fn there_has_to_be_a_fragments_directory() {
    let root = Utf8TempDir::new().unwrap();

    let error = plan(&fragments(&root), &fixed("Crashes are fixed."), None).unwrap_err();

    assert_eq!(
        error,
        "There is no fragments directory at changelog.d; create it with `jp release init`."
    );
}

/// `highlights/` holds no file until its first highlight, so a checkout has
/// none to begin with, and the first highlight creates it.
#[test]
fn writing_a_highlight_creates_highlights() {
    let root = workspace();

    let planned = plan(
        &fragments(&root),
        &highlight(
            "Tool calls fan out",
            "The assistant calls one tool at once.",
        ),
        None,
    )
    .unwrap();
    planned.write().unwrap();

    assert_eq!(fs::read_to_string(&planned.path).unwrap(), planned.text);
    let report = changelog::check(&root.path().join("changelog.d")).unwrap();
    assert_eq!(report.highlights, 1);
    assert!(report.problems.is_empty(), "{report}");
}

#[test]
fn writing_refuses_a_file_that_appeared_since_it_was_planned() {
    let root = workspace();
    let planned = plan(&fragments(&root), &fixed("Crashes are fixed."), None).unwrap();
    fs::write(&planned.path, "Someone else's.\n").unwrap();

    let error = planned.write().unwrap_err();

    assert_eq!(
        error,
        "changelog.d/crashes-are-fixed.md already exists; choose another slug."
    );
    assert_eq!(
        fs::read_to_string(&planned.path).unwrap(),
        "Someone else's.\n"
    );
}
