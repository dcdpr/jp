use indoc::indoc;

use super::*;

const PAGE: &str = indoc! {"
    # Glossary

    - **Status**: Reference

    ## Turn

    One cycle.

    ### Pinned Conversation

    Pinned.

    ## Examples

    First.

    ## Examples

    Second, see [Turn][turn].

    [turn]: #turn
"};

#[test]
fn a_section_runs_to_the_next_heading_of_its_level() {
    let turn = find(PAGE, "turn").unwrap();

    assert_eq!(
        text(PAGE, &turn),
        "## Turn\n\nOne cycle.\n\n### Pinned Conversation\n\nPinned."
    );
}

#[test]
fn the_last_section_stops_before_the_definitions() {
    let last = find(PAGE, "examples-1").unwrap();

    assert_eq!(
        text(PAGE, &last),
        "## Examples\n\nSecond, see [Turn][turn]."
    );
}

#[test]
fn a_title_names_a_section_when_one_heading_carries_it() {
    let pinned = find(PAGE, "pinned conversation").unwrap();

    assert_eq!(pinned.heading.slug, "pinned-conversation");
}

#[test]
fn a_shared_title_is_refused_with_the_anchors() {
    assert_eq!(
        find(PAGE, "Examples"),
        Err(
            "2 sections are titled `Examples`; name one by its anchor: `#examples`, `#examples-1`."
                .to_owned()
        )
    );
}

#[test]
fn an_unknown_section_names_the_ones_there_are() {
    let error = find("# A\n\n## B\n", "c").unwrap_err();

    assert_eq!(error, "No section `c`; the sections are `#a`, `#b`.");
}

#[test]
fn a_body_opening_with_a_heading_replaces_the_whole_section() {
    let section = find(PAGE, "turn").unwrap();

    let out = replace(PAGE, &section, "## Turn\n\nTwo cycles.");

    assert!(
        out.contains("- **Status**: Reference\n\n## Turn\n\nTwo cycles.\n\n## Examples\n"),
        "{out}"
    );
}

#[test]
fn a_body_without_a_heading_keeps_the_section_heading() {
    let section = find(PAGE, "pinned-conversation").unwrap();

    let out = replace(PAGE, &section, "Kept on top.\n");

    assert!(
        out.contains("### Pinned Conversation\n\nKept on top.\n\n## Examples\n"),
        "{out}"
    );
}

#[test]
fn replacing_the_last_section_keeps_the_definitions() {
    let section = find(PAGE, "examples-1").unwrap();

    let out = replace(PAGE, &section, "Replaced.");

    assert!(
        out.ends_with("## Examples\n\nReplaced.\n\n[turn]: #turn\n"),
        "{out}"
    );
}
