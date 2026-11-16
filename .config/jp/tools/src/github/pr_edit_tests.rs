use super::*;

fn strip_ansi(rendered: &str) -> String {
    String::from_utf8(strip_ansi_escapes::strip(rendered)).unwrap()
}

#[test]
fn edit_requires_at_least_one_field() {
    assert_eq!(
        Edit::new(None, None),
        Err("Provide `title`, `description`, or both.")
    );
}

#[test]
fn edit_rejects_a_blank_title() {
    assert_eq!(
        Edit::new(Some("   ".to_owned()), None),
        Err("`title` must not be empty.")
    );
}

#[test]
fn edit_rejects_a_multi_line_title() {
    assert_eq!(
        Edit::new(Some("feat: Add X\n\nBody text".to_owned()), None),
        Err("`title` must be a single line.")
    );
}

#[test]
fn edit_trims_the_title() {
    // Titles copied from commit messages often end with a newline, so
    // `Edit::new` trims whitespace before checking for multiple lines.
    assert_eq!(
        Edit::new(Some("  fix: Handle empty input\n".to_owned()), None),
        Ok(Edit {
            title: Some("fix: Handle empty input".to_owned()),
            description: None,
        })
    );
}

#[test]
fn edit_accepts_an_empty_description() {
    assert_eq!(
        Edit::new(None, Some(String::new())),
        Ok(Edit {
            title: None,
            description: Some(String::new()),
        })
    );
}

#[test]
fn edit_names_the_fields_it_changes() {
    let both = Edit::new(Some("t".to_owned()), Some("d".to_owned())).unwrap();
    let title = Edit::new(Some("t".to_owned()), None).unwrap();
    let description = Edit::new(None, Some("d".to_owned())).unwrap();

    assert_eq!(both.fields(), "title and description");
    assert_eq!(title.fields(), "title");
    assert_eq!(description.fields(), "description");
}

#[test]
fn preview_diffs_the_title_against_the_current_one() {
    let current = PullText {
        title: "fix: thing is brokn".to_owned(),
        description: "Unrelated.".to_owned(),
    };
    let edit = Edit::new(Some("fix: Repair the broken thing".to_owned()), None).unwrap();

    let out = strip_ansi(&render_edit_preview(1234, &current, &edit));

    assert_eq!(
        out,
        concat!(
            "PR #1234 \u{2014} edit title\n",
            "\n",
            "-1,+1 \u{2502} title\n",
            "\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{253c}",
            "\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\n",
            "1     \u{2502}-fix: thing is brokn\n",
            "  1   \u{2502}+fix: Repair the broken thing\n",
            "\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{253c}",
            "\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\n",
            "-1,+1 \u{2502} title\n",
        )
    );
}

#[test]
fn preview_ignores_crlf_line_endings_in_the_current_description() {
    let current = PullText {
        title: "feat: Add X".to_owned(),
        description: "First line.\r\nSecond lnie.\r\n".to_owned(),
    };
    let edit = Edit::new(None, Some("First line.\nSecond line.".to_owned())).unwrap();

    let out = strip_ansi(&render_edit_preview(7, &current, &edit));

    assert_eq!(
        out,
        concat!(
            "PR #7 \u{2014} edit description\n",
            "\n",
            "-1,+1 \u{2502} description\n",
            "\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{253c}",
            "\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}",
            "\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\n",
            "1 1   \u{2502} First line.\n",
            "2     \u{2502}-Second lnie.\n",
            "  2   \u{2502}+Second line.\n",
            "\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{253c}",
            "\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}",
            "\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\n",
            "-1,+1 \u{2502} description\n",
        )
    );
}

#[test]
fn preview_marks_a_field_set_to_its_current_text_as_unchanged() {
    let current = PullText {
        title: "feat: Add X".to_owned(),
        description: "Old text.".to_owned(),
    };
    let edit = Edit::new(Some("feat: Add X".to_owned()), Some("Old text.".to_owned())).unwrap();

    let out = strip_ansi(&render_edit_preview(7, &current, &edit));

    assert_eq!(
        out,
        concat!(
            "PR #7 \u{2014} edit title and description\n",
            "\n",
            "title: unchanged\n",
            "\n",
            "description: unchanged",
        )
    );
}

#[test]
fn unverified_preview_quotes_the_new_text_and_names_the_failure() {
    let edit = Edit::new(Some("feat: Add X".to_owned()), Some("Adds X.".to_owned())).unwrap();

    let out = strip_ansi(&render_unverified_preview(7, &edit, "Pull #7 not found"));

    // The renderer gives a blank quote line a trailing space.
    assert_eq!(
        out,
        concat!(
            "PR #7 \u{2014} edit title and description (current text unavailable: Pull #7 not \
             found)\n",
            "\n",
            "> feat: Add X\n",
            "> \n",
            "> Adds X.\n",
        )
    );
}
