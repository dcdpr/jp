use indoc::indoc;

use super::*;

#[test]
fn a_fragment_without_an_upgrade_section_is_all_entry() {
    let fragment = parse(indoc! {"
        ---
        type: fixed
        ---
        A turn that stalls while idle is retried instead of failing.
    "})
    .unwrap();

    assert_eq!(fragment, Fragment {
        type_: Type::Fixed,
        change_requests: None,
        entry: "A turn that stalls while idle is retried instead of failing.".to_owned(),
        upgrade: None,
    });
}

#[test]
fn each_of_the_seven_ids_names_its_type() {
    let ids = [
        ("added", Type::Added),
        ("changed", Type::Changed),
        ("deprecated", Type::Deprecated),
        ("removed", Type::Removed),
        ("fixed", Type::Fixed),
        ("security", Type::Security),
        ("performance", Type::Performance),
    ];

    for (id, type_) in ids {
        let fragment = parse(&format!("---\ntype: {id}\n---\nEntry.\n")).unwrap();
        assert_eq!(fragment.type_, type_, "{id}");
    }
}

#[test]
fn each_type_reads_back_from_its_id() {
    let ids: Vec<&str> = Type::ALL.iter().map(|type_| type_.id()).collect();

    assert_eq!(ids, [
        "added",
        "changed",
        "deprecated",
        "removed",
        "fixed",
        "security",
        "performance"
    ]);
    for type_ in Type::ALL {
        assert_eq!(type_.id().parse::<Type>(), Ok(type_));
        assert_eq!(type_.to_string(), type_.id());
    }
}

#[test]
fn a_type_says_what_it_is_used_for() {
    assert_eq!(
        Type::Added.description(),
        "A new capability the user can now reach."
    );
    assert_eq!(
        Type::Performance.description(),
        "A user-visible performance improvement."
    );
}

#[test]
fn an_unknown_id_names_the_seven() {
    let error = "feat".parse::<Type>().unwrap_err();

    assert_eq!(
        error.to_string(),
        "`feat` is not a changelog type; use one of: added, changed, deprecated, removed, fixed, \
         security, performance"
    );
    // Written as the ids are, as in the frontmatter.
    assert!("Added".parse::<Type>().is_err());
}

#[test]
fn a_type_is_one_of_the_seven_ids_as_written() {
    let error = parse("---\ntype: Added\n---\nEntry.\n").unwrap_err();

    // Lines count from the top of the file, the opening `---` included.
    assert_eq!(
        error.to_string(),
        "invalid frontmatter: unknown variant `Added`, expected one of added, changed, \
         deprecated, removed, fixed, security, performance at line 2, column 7"
    );
}

#[test]
fn a_fragment_without_a_type_is_an_error() {
    let error = parse("---\nchange_requests: [1207]\n---\nEntry.\n").unwrap_err();

    assert_eq!(
        error.to_string(),
        "invalid frontmatter: missing field `type` at line 2, column 1"
    );
    assert_eq!(
        parse("---\n---\nEntry.\n").unwrap_err().to_string(),
        "invalid frontmatter: missing field `type`"
    );
}

#[test]
fn change_requests_replace_the_credits_history_names() {
    let fragment = parse(indoc! {"
        ---
        type: fixed
        change_requests: [1207, 1211]
        ---
        Entry.
    "})
    .unwrap();

    assert_eq!(
        fragment.change_requests,
        Some(vec![
            NonZeroU64::new(1207).unwrap(),
            NonZeroU64::new(1211).unwrap()
        ])
    );
}

#[test]
fn a_change_request_is_a_positive_number() {
    let error = parse("---\ntype: fixed\nchange_requests: [0]\n---\nEntry.\n").unwrap_err();

    assert_eq!(
        error.to_string(),
        "invalid frontmatter: invalid value: integer `0`, expected a nonzero u64 at line 3, \
         column 19"
    );
}

#[test]
fn an_unknown_frontmatter_field_is_an_error_rather_than_ignored() {
    // A typo for `change_requests`, which would otherwise drop the credits
    // without a word.
    let error = parse("---\ntype: fixed\nchange_request: [1207]\n---\nEntry.\n").unwrap_err();

    assert_eq!(
        error.to_string(),
        "invalid frontmatter: unknown field `change_request`, expected one of type, \
         change_requests at line 3, column 1"
    );
}

#[test]
fn a_fragment_opens_with_its_frontmatter() {
    assert_eq!(
        parse("type: fixed\n---\nEntry.\n"),
        Err(FragmentError::Frontmatter(FrontmatterError::Missing))
    );
    assert_eq!(
        parse("\n---\ntype: fixed\n---\nEntry.\n"),
        Err(FragmentError::Frontmatter(FrontmatterError::Missing))
    );
}

#[test]
fn a_frontmatter_needs_a_closing_line() {
    assert_eq!(
        parse("---\ntype: fixed\nEntry.\n"),
        Err(FragmentError::Frontmatter(FrontmatterError::Unclosed))
    );
}

#[test]
fn a_fragment_needs_an_entry() {
    assert_eq!(
        parse("---\ntype: fixed\n---\n\n  \n"),
        Err(FragmentError::EmptyBody)
    );
    assert_eq!(
        parse("---\ntype: fixed\n---"),
        Err(FragmentError::EmptyBody)
    );
}

#[test]
fn windows_line_endings_read_as_unix_ones() {
    let fragment = parse(
        "---\r\ntype: removed\r\n---\r\nEntry.\r\n\r\nBREAKING CHANGE: Title\r\n\r\nBody.\r\n",
    )
    .unwrap();

    assert_eq!(fragment.entry, "Entry.");
    assert_eq!(
        fragment.upgrade,
        Some(Upgrade {
            title: "Title".to_owned(),
            body: "Body.".to_owned(),
        })
    );
}

#[test]
fn a_byte_order_mark_is_skipped() {
    let fragment = parse("\u{feff}---\ntype: fixed\n---\nEntry.\n").unwrap();

    assert_eq!(fragment.entry, "Entry.");
}

#[test]
fn an_upgrade_section_splits_the_body_into_entry_title_and_body() {
    let fragment = parse(indoc! {r#"
        ---
        type: removed
        ---
        `tools.<name>.enable` no longer accepts the legacy string `"explicit"`.

        BREAKING CHANGE: `enable = "explicit"` is no longer accepted

        `"explicit"` was the last of the string forms `enable` accepted before 0.2.0.
        Replace it with the table form, which means the same thing:

        ```toml
        # 0.2
        enable = "explicit"

        # 0.3
        enable = { state = false, allow_toggle = "if_named" }
        ```
    "#})
    .unwrap();

    assert_eq!(
        fragment.entry,
        r#"`tools.<name>.enable` no longer accepts the legacy string `"explicit"`."#
    );
    assert_eq!(
        fragment.upgrade,
        Some(Upgrade {
            title: r#"`enable = "explicit"` is no longer accepted"#.to_owned(),
            body: indoc! {r#"
                `"explicit"` was the last of the string forms `enable` accepted before 0.2.0.
                Replace it with the table form, which means the same thing:

                ```toml
                # 0.2
                enable = "explicit"

                # 0.3
                enable = { state = false, allow_toggle = "if_named" }
                ```"#}
            .to_owned(),
        })
    );
}

#[test]
fn breaking_hyphen_change_opens_an_upgrade_section_too() {
    let fragment = parse(indoc! {"
        ---
        type: changed
        ---
        Entry.

        BREAKING-CHANGE: Title

        Body.
    "})
    .unwrap();

    assert_eq!(
        fragment.upgrade,
        Some(Upgrade {
            title: "Title".to_owned(),
            body: "Body.".to_owned(),
        })
    );
}

#[test]
fn a_title_wrapped_over_lines_is_read_as_one_line() {
    let fragment = parse(indoc! {"
        ---
        type: changed
        ---
        Entry.

        BREAKING CHANGE: Command plugins are found by
        their manifest

        Body.
    "})
    .unwrap();

    assert_eq!(
        fragment.upgrade.map(|upgrade| upgrade.title),
        Some("Command plugins are found by their manifest".to_owned())
    );
}

#[test]
fn a_token_that_does_not_start_its_line_is_text() {
    let fragment = parse(indoc! {"
        ---
        type: changed
        ---
        A paragraph that starts with `BREAKING CHANGE:` opens an upgrade section.

        `BREAKING CHANGE:` in inline code, and
         BREAKING CHANGE: indented, are both text.
    "})
    .unwrap();

    assert_eq!(fragment.upgrade, None);
    assert_eq!(fragment.entry, indoc! {"
            A paragraph that starts with `BREAKING CHANGE:` opens an upgrade section.

            `BREAKING CHANGE:` in inline code, and
             BREAKING CHANGE: indented, are both text."});
}

#[test]
fn a_token_partway_through_a_paragraph_is_an_error() {
    // Without a blank line before it, the token continues the entry's
    // paragraph rather than opening one, so a reader would see no upgrade
    // section where a search for the token finds one.
    let error = parse(indoc! {"
        ---
        type: changed
        ---
        Entry.
        BREAKING CHANGE: Title

        Body.
    "})
    .unwrap_err();

    assert_eq!(error, FragmentError::MarkerOutsideParagraph {
        line: 5,
        marker: "BREAKING CHANGE:",
    });
}

#[test]
fn a_token_continuing_a_list_item_is_an_error() {
    let error = parse(indoc! {"
        ---
        type: changed
        ---
        Entry:

        - one change
        BREAKING CHANGE: Title

        Body.
    "})
    .unwrap_err();

    assert_eq!(error, FragmentError::MarkerOutsideParagraph {
        line: 7,
        marker: "BREAKING CHANGE:",
    });
}

#[test]
fn a_token_inside_a_code_block_is_an_error() {
    let error = parse(indoc! {"
        ---
        type: changed
        ---
        Entry, with an example:

        ```text
        BREAKING-CHANGE: in an example
        ```
    "})
    .unwrap_err();

    assert_eq!(error, FragmentError::MarkerInCodeBlock {
        line: 7,
        marker: "BREAKING-CHANGE:",
    });
}

#[test]
fn a_fragment_has_at_most_one_upgrade_section() {
    let error = parse(indoc! {"
        ---
        type: changed
        ---
        Entry.

        BREAKING CHANGE: First

        Body.

        BREAKING CHANGE: Second

        Body.
    "})
    .unwrap_err();

    assert_eq!(error, FragmentError::SecondUpgradeSection { line: 10 });
}

#[test]
fn an_upgrade_section_needs_an_entry_before_it() {
    let error = parse(indoc! {"
        ---
        type: changed
        ---
        BREAKING CHANGE: Title

        Body.
    "})
    .unwrap_err();

    assert_eq!(error, FragmentError::EmptyEntry {
        line: 4,
        marker: "BREAKING CHANGE:",
    });
}

#[test]
fn an_upgrade_section_needs_a_title() {
    let error =
        parse("---\ntype: changed\n---\nEntry.\n\nBREAKING CHANGE:  \n\nBody.\n").unwrap_err();

    assert_eq!(error, FragmentError::EmptyUpgradeTitle {
        line: 6,
        marker: "BREAKING CHANGE:",
    });
}

#[test]
fn an_upgrade_section_needs_a_body() {
    let error = parse("---\ntype: changed\n---\nEntry.\n\nBREAKING CHANGE: Title\n").unwrap_err();

    assert_eq!(error, FragmentError::EmptyUpgradeBody { line: 6 });
}

#[test]
fn a_misplaced_token_says_how_to_show_it_instead() {
    assert_eq!(
        FragmentError::MarkerInCodeBlock {
            line: 7,
            marker: "BREAKING CHANGE:",
        }
        .to_string(),
        "line 7 starts with `BREAKING CHANGE:` inside a code block; to show the token, indent the \
         line or put the token in inline code"
    );
}
