use std::num::NonZeroU64;

use indoc::indoc;

use super::*;

fn number(number: u64) -> NonZeroU64 {
    NonZeroU64::new(number).unwrap()
}

#[test]
fn a_fragment_without_an_upgrade_section_is_all_entry() {
    let fragment = parse(
        Type::Fixed,
        "A turn that stalls while idle is retried instead of failing.\n",
    )
    .unwrap();

    assert_eq!(fragment, Fragment {
        type_: Type::Fixed,
        credits: None,
        entry: "A turn that stalls while idle is retried instead of failing.".to_owned(),
        upgrade: None,
    });
}

#[test]
fn a_name_holds_the_slug_and_the_type() {
    assert_eq!(
        name("retry-idle-turns.fixed.md"),
        Ok(("retry-idle-turns", Type::Fixed))
    );
    // The type is the last part before `.md`, so a slug can hold dots.
    assert_eq!(name("v1.2.added.md"), Ok(("v1.2", Type::Added)));
    assert_eq!(
        file_name("retry-idle-turns", Type::Fixed),
        "retry-idle-turns.fixed.md"
    );
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
        assert_eq!(
            name(&format!("entry.{id}.md")),
            Ok(("entry", type_)),
            "{id}"
        );
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
    // Written as the ids are, as in a file name.
    assert!("Added".parse::<Type>().is_err());
}

#[test]
fn a_name_without_a_type_says_how_to_name_it() {
    let error = name("retry.md").unwrap_err();

    assert_eq!(error, NameError::MissingType {
        slug: "retry".to_owned()
    });
    assert_eq!(
        error.to_string(),
        "no type in the name: name it `retry.<type>.md`, such as `retry.fixed.md`, with one of: \
         added, changed, deprecated, removed, fixed, security, performance"
    );
}

#[test]
fn a_name_with_an_unknown_type_names_the_seven() {
    let error = name("retry.Fixed.md").unwrap_err();

    assert_eq!(error, NameError::UnknownType {
        type_: "Fixed".to_owned()
    });
    assert_eq!(
        error.to_string(),
        "`Fixed` is not a changelog type: the part of the name before `.md` is one of: added, \
         changed, deprecated, removed, fixed, security, performance"
    );
}

#[test]
fn a_name_without_md_or_a_slug_is_not_a_fragment() {
    for not_one in ["notes.txt", ".md", ".fixed.md", "retry.fixed"] {
        assert_eq!(name(not_one), Err(NameError::NotAFragment), "{not_one}");
    }
    assert_eq!(
        NameError::NotAFragment.to_string(),
        "not a fragment, which is named `<slug>.<type>.md`; rename it, or move it out of the \
         fragments directory"
    );
}

#[test]
fn credits_replace_the_credits_history_names() {
    let fragment = parse(Type::Fixed, indoc! {"
        - **Credits**: #1207
        - **Credits**: !1211
        - **Credits**: 4E1F0A2

        Entry.
    "})
    .unwrap();

    assert_eq!(
        fragment.credits,
        Some(vec![
            ListedCredit::ChangeRequest(number(1207)),
            ListedCredit::ChangeRequest(number(1211)),
            ListedCredit::Commit("4e1f0a2".to_owned()),
        ])
    );
    assert_eq!(fragment.entry, "Entry.");
}

#[test]
fn a_credit_names_a_change_request_or_a_commit() {
    for value in ["#0", "1207", "4e1f0a", "@JeanMertz"] {
        let error = parse(Type::Fixed, &format!("- **Credits**: {value}\n\nEntry.\n")).unwrap_err();

        assert_eq!(
            error,
            FragmentError::InvalidCredit {
                line: 1,
                value: value.to_owned()
            },
            "{value}"
        );
    }
    assert_eq!(
        FragmentError::InvalidCredit {
            line: 1,
            value: "@JeanMertz".to_owned()
        }
        .to_string(),
        "line 1 credits `@JeanMertz`, which is neither a change request, such as `#1207`, nor a \
         commit's hash"
    );
}

/// The file name holds the type, so the file can't disagree with it.
#[test]
fn a_type_field_is_refused() {
    let error = parse(Type::Fixed, "- **Type**: fixed\n\nEntry.\n").unwrap_err();

    assert_eq!(error, FragmentError::TypeField { line: 1 });
    assert_eq!(
        error.to_string(),
        "line 1 names a type, which a fragment's file name holds: `<slug>.<type>.md`"
    );
}

#[test]
fn an_unknown_field_is_an_error_rather_than_ignored() {
    // A typo for `Credits`, which would otherwise drop the credits without a
    // word.
    let error = parse(Type::Fixed, "- **Credit**: #1207\n\nEntry.\n").unwrap_err();

    assert_eq!(error, FragmentError::UnknownField {
        line: 1,
        key: "Credit".to_owned()
    });
    assert_eq!(
        error.to_string(),
        "line 1 lists `Credit`, and the only field a fragment takes is `Credits`"
    );
}

/// Only a block that opens the file is read as fields: a list further down is
/// the entry's.
#[test]
fn a_list_in_the_entry_is_not_metadata() {
    let fragment = parse(Type::Changed, indoc! {"
        Two settings moved:

        - **Old**: `tools.enable`
        - **New**: `tools.state`
    "})
    .unwrap();

    assert_eq!(fragment.credits, None);
    assert_eq!(fragment.entry, indoc! {"
        Two settings moved:

        - **Old**: `tools.enable`
        - **New**: `tools.state`"});
}

#[test]
fn a_fragment_needs_an_entry() {
    for empty in [
        "",
        "\n  \n",
        "- **Credits**: #1207\n",
        "- **Credits**: #1207\n\n  \n",
    ] {
        assert_eq!(
            parse(Type::Fixed, empty),
            Err(FragmentError::EmptyBody),
            "{empty:?}"
        );
    }
    assert_eq!(
        FragmentError::EmptyBody.to_string(),
        "no entry: say what changed for users"
    );
}

#[test]
fn windows_line_endings_read_as_unix_ones() {
    let fragment = parse(
        Type::Removed,
        "- **Credits**: #7\r\n\r\nEntry.\r\n\r\nBREAKING CHANGE: Title\r\n\r\nBody.\r\n",
    )
    .unwrap();

    assert_eq!(
        fragment.credits,
        Some(vec![ListedCredit::ChangeRequest(number(7))])
    );
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
    assert_eq!(
        parse(Type::Fixed, "\u{feff}Entry.\n").unwrap().entry,
        "Entry."
    );
    assert_eq!(
        parse(Type::Fixed, "\u{feff}- **Credits**: #7\n\nEntry.\n")
            .unwrap()
            .credits,
        Some(vec![ListedCredit::ChangeRequest(number(7))])
    );
}

#[test]
fn an_upgrade_section_splits_the_body_into_entry_title_and_body() {
    let fragment = parse(Type::Removed, indoc! {r#"
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
    let fragment = parse(Type::Changed, indoc! {"
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
    let fragment = parse(Type::Changed, indoc! {"
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
    let fragment = parse(Type::Changed, indoc! {"
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
    let error = parse(Type::Changed, indoc! {"
        Entry.
        BREAKING CHANGE: Title

        Body.
    "})
    .unwrap_err();

    assert_eq!(error, FragmentError::MarkerOutsideParagraph {
        line: 2,
        marker: "BREAKING CHANGE:",
    });
}

#[test]
fn a_token_continuing_a_list_item_is_an_error() {
    let error = parse(Type::Changed, indoc! {"
        Entry:

        - one change
        BREAKING CHANGE: Title

        Body.
    "})
    .unwrap_err();

    assert_eq!(error, FragmentError::MarkerOutsideParagraph {
        line: 4,
        marker: "BREAKING CHANGE:",
    });
}

#[test]
fn a_token_inside_a_code_block_is_an_error() {
    let error = parse(Type::Changed, indoc! {"
        Entry, with an example:

        ```text
        BREAKING-CHANGE: in an example
        ```
    "})
    .unwrap_err();

    assert_eq!(error, FragmentError::MarkerInCodeBlock {
        line: 4,
        marker: "BREAKING-CHANGE:",
    });
}

#[test]
fn a_fragment_has_at_most_one_upgrade_section() {
    let error = parse(Type::Changed, indoc! {"
        Entry.

        BREAKING CHANGE: First

        Body.

        BREAKING CHANGE: Second

        Body.
    "})
    .unwrap_err();

    assert_eq!(error, FragmentError::SecondUpgradeSection { line: 7 });
}

#[test]
fn an_upgrade_section_needs_an_entry_before_it() {
    let error = parse(Type::Changed, "BREAKING CHANGE: Title\n\nBody.\n").unwrap_err();

    assert_eq!(error, FragmentError::EmptyEntry {
        line: 1,
        marker: "BREAKING CHANGE:",
    });
}

#[test]
fn an_upgrade_section_needs_a_title() {
    let error = parse(Type::Changed, "Entry.\n\nBREAKING CHANGE:  \n\nBody.\n").unwrap_err();

    assert_eq!(error, FragmentError::EmptyUpgradeTitle {
        line: 3,
        marker: "BREAKING CHANGE:",
    });
}

#[test]
fn an_upgrade_section_needs_a_body() {
    let error = parse(Type::Changed, "Entry.\n\nBREAKING CHANGE: Title\n").unwrap_err();

    assert_eq!(error, FragmentError::EmptyUpgradeBody { line: 3 });
}

/// Lines count from the top of the file, the metadata block included.
#[test]
fn a_line_an_error_names_is_the_files() {
    let error = parse(Type::Changed, indoc! {"
        - **Credits**: #1207

        Entry.
        BREAKING CHANGE: Title

        Body.
    "})
    .unwrap_err();

    assert_eq!(error, FragmentError::MarkerOutsideParagraph {
        line: 4,
        marker: "BREAKING CHANGE:",
    });
}

/// A formatter that gathers definitions at the end of the file puts them below
/// the upgrade section, and each part still reads its links' URLs from them.
#[test]
fn each_part_reads_its_links_urls_from_definitions_anywhere_in_the_file() {
    let fragment = parse(Type::Removed, indoc! {r#"
        `jp query --legacy` is gone, as [the flag's docs] said it would be.

        BREAKING CHANGE: [`--legacy`] is no longer accepted

        Drop the flag; [the upgrade notes][notes] say what replaces it.

        [`--legacy`]: https://example.com/legacy
        [notes]: https://example.com/notes
        [the flag's docs]: https://example.com/flags "Flags"
    "#})
    .unwrap();

    assert_eq!(
        fragment.entry,
        r#"`jp query --legacy` is gone, as [the flag's docs](https://example.com/flags "Flags") said it would be."#
    );
    assert_eq!(
        fragment.upgrade,
        Some(Upgrade {
            title: "[`--legacy`](https://example.com/legacy) is no longer accepted".to_owned(),
            body: "Drop the flag; [the upgrade notes](https://example.com/notes) say what \
                   replaces it."
                .to_owned(),
        })
    );
}

#[test]
fn a_body_of_definitions_alone_has_no_entry() {
    assert_eq!(
        parse(Type::Fixed, "[docs]: https://example.com/docs\n"),
        Err(FragmentError::EmptyBody)
    );
}

#[test]
fn an_upgrade_body_of_definitions_alone_is_empty() {
    let error = parse(Type::Changed, indoc! {"
        See [the docs].

        BREAKING CHANGE: Title

        [the docs]: https://example.com/docs
    "})
    .unwrap_err();

    assert_eq!(error, FragmentError::EmptyUpgradeBody { line: 3 });
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
