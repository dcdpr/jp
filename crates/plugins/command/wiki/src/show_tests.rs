use std::fs;

use camino_tempfile::Utf8TempDir;
use indoc::indoc;
use jp_plugin::message::OutputFormat;
use serde_json::{Map, Value};

use super::*;
use crate::{
    access::Access,
    options::{Options, Paths},
};

const GLOSSARY: &str = indoc! {"
    # Glossary

    - **Status**: Reference
    - **Authors**: jp
    - **Date**: 2026-10-30
    - **Label**: relevant-rfd=100
    - **Summary**: The words.

    ## Turn

    One cycle, as RFD 033 put it; see T-02wt0kx.

    ### Pinned Conversation

    Pinned.
"};

const CHILD: &str = indoc! {"
    # Child

    - **Status**: Reference
    - **Authors**: jp
    - **Date**: 2026-10-30
    - **Parent**: W-0000001
    - **Summary**: A child.

    ## Context

    ![Turn](0000001-glossary.md#turn)
"};

/// A workspace with the glossary, a child page that embeds from it, two RFDs,
/// and a ticket.
fn workspace() -> Utf8TempDir {
    let root = Utf8TempDir::new().unwrap();
    for (path, text) in [
        ("docs/wiki/0000001-glossary.md", GLOSSARY),
        ("docs/wiki/0000002-child.md", CHILD),
        (
            "docs/rfd/100-tickets.md",
            "# RFD 100: Tickets\n\n- **Status**: Implemented\n",
        ),
        (
            "docs/rfd/033-old.md",
            "# RFD 033: Old\n\n- **Status**: Superseded\n\nSee W-0000001.\n",
        ),
        (
            "docs/ticket/02wt0kx-a-ticket.md",
            "# A ticket\n\n- **Status**: Todo\n",
        ),
    ] {
        let path = root.path().join(path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }

    root
}

fn show_in(root: &Utf8Path, id: &str, body: bool, section: Option<&str>) -> Output {
    show_as(root, id, body, section, false)
}

fn show_as(root: &Utf8Path, id: &str, body: bool, section: Option<&str>, raw: bool) -> Output {
    let options = Options::default();
    let config = Value::Object(Map::new());
    let ctx = Context {
        paths: Paths::new(root, &config, &options).unwrap(),
        config: &config,
        options: &options,
        format: OutputFormat::default(),
        access: Access::Unrestricted,
    };

    show(&ctx, id.parse().unwrap(), body, section, raw).unwrap()
}

#[test]
fn a_page_shows_its_metadata_references_and_contents() {
    let root = workspace();

    let output = show_in(root.path(), "W-0000001", false, None);

    assert_eq!(output.text, indoc! {"
        # W-0000001: Glossary

        - **Path**: docs/wiki/0000001-glossary.md
        - **Status**: Reference
        - **Authors**: jp
        - **Date**: 2026-10-30
        - **Label**: relevant-rfd=100
        - **Summary**: The words.

        ## References

        - RFD 033: Old
        - RFD 100: Tickets
        - T-02wt0kx: A ticket

        ## Referenced by

        - W-0000002: Child
        - RFD 033: Old

        ## Contents

        - Turn `#turn`
          - Pinned Conversation `#pinned-conversation`
    "});
}

#[test]
fn a_section_shows_with_its_embeds_expanded() {
    let root = workspace();

    let output = show_in(root.path(), "W-0000002", false, Some("context"));

    assert_eq!(output.text, indoc! {"
        ## Context

        <!-- embed W-0000001#turn -->
        ### Turn

        One cycle, as RFD 033 put it; see T-02wt0kx.

        #### Pinned Conversation

        Pinned.
        <!-- /embed -->
    "});
    assert_eq!(output.warnings, Vec::<String>::new());
}

#[test]
fn the_body_shows_in_place_of_the_contents() {
    let root = workspace();

    let output = show_in(root.path(), "W-0000002", true, None);

    assert!(
        output.text.ends_with(
            "## Context\n\n<!-- embed W-0000001#turn -->\n### Turn\n\nOne cycle, as RFD 033 put \
             it; see T-02wt0kx.\n\n#### Pinned Conversation\n\nPinned.\n<!-- /embed -->\n"
        ),
        "{}",
        output.text
    );
    assert!(!output.text.contains("## Contents"), "{}", output.text);
}

/// The text an edit replaces is the text as written, so an editor shows the
/// embed rather than what it expands to.
#[test]
fn raw_text_leaves_the_embeds_as_written() {
    let root = workspace();

    let section = show_as(root.path(), "W-0000002", false, Some("context"), true);
    let body = show_as(root.path(), "W-0000002", true, None, true);

    assert_eq!(
        section.text,
        "## Context\n\n![Turn](0000001-glossary.md#turn)\n"
    );
    assert!(
        body.text
            .ends_with("\n## Context\n\n![Turn](0000001-glossary.md#turn)\n"),
        "{}",
        body.text
    );
}
