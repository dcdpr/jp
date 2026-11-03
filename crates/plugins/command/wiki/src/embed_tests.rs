use std::fs;

use camino_tempfile::Utf8TempDir;
use indoc::indoc;
use jp_tool::FsRule;

use super::*;

/// A workspace with a glossary page, a page in another directory that embeds
/// from it, and a source file.
fn workspace() -> Utf8TempDir {
    let root = Utf8TempDir::new().unwrap();
    for (path, text) in [
        ("docs/wiki/0a1b2c3-glossary.md", GLOSSARY),
        (
            "crates/x/src/lib.rs",
            "fn one() {}\nfn by_rfd() {\n    // ```\n}\nfn four() {}\n",
        ),
        (
            "docs/other/page.md",
            "# Page\n\n## Context\n\n![Turn](../wiki/0a1b2c3-glossary.md#turn)\n",
        ),
    ] {
        let path = root.path().join(path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }

    root
}

const GLOSSARY: &str = indoc! {"
    # Glossary

    - **Status**: Reference

    ## Turn

    One [cycle][cycle], see [Pinned](#pinned-conversation) and
    [the code](../../crates/x/src/lib.rs#L2-L4).

    ### Pinned Conversation

    Pinned.

    ## Loop

    ![Loop](0a1b2c3-glossary.md#loop)

    [cycle]: https://example.com/cycle
"};

fn expand(root: &Utf8Path, text: &str, path: &str) -> (String, Vec<Problem>) {
    expand_under(&Access::Unrestricted, root, text, path)
}

fn expand_under(
    access: &Access,
    root: &Utf8Path,
    text: &str,
    path: &str,
) -> (String, Vec<Problem>) {
    let library = Library::default();
    let wiki = root.join("docs/wiki");
    let mut expander = Expander::new(root, &wiki, &library, access);

    let out = expander.expand(text, &root.join(path), None);
    (out, expander.problems)
}

#[test]
fn an_embedded_section_reads_from_where_it_appears() {
    let root = workspace();
    let page = fs::read_to_string(root.path().join("docs/other/page.md")).unwrap();

    let (out, problems) = expand(root.path(), &page, "docs/other/page.md");

    assert_eq!(problems, []);
    assert_eq!(out, indoc! {"
        # Page

        ## Context

        <!-- embed W-0a1b2c3#turn -->
        ### Turn

        One [cycle](https://example.com/cycle), see [Pinned](../wiki/0a1b2c3-glossary.md#pinned-conversation) and
        [the code](../../crates/x/src/lib.rs#L2-L4).

        #### Pinned Conversation

        Pinned.
        <!-- /embed -->"});
}

#[test]
fn a_line_range_embeds_as_a_fenced_block() {
    let root = workspace();
    let text = "# Page\n\n![`by_rfd`](../../crates/x/src/lib.rs#L2-4)\n";

    let (out, problems) = expand(root.path(), text, "docs/wiki/0b0b0b0-page.md");

    assert_eq!(problems, []);
    assert_eq!(out, indoc! {"
        # Page

        <!-- embed crates/x/src/lib.rs#L2-L4 -->
        [`by_rfd`](../../crates/x/src/lib.rs#L2-L4)

        ````rs
        fn by_rfd() {
            // ```
        }
        ````
        <!-- /embed -->"});
}

/// A section that embeds itself would expand forever; it is left as the link it
/// is, and the loop named.
#[test]
fn an_embed_that_loops_is_left_alone() {
    let root = workspace();

    let (out, problems) = expand(root.path(), GLOSSARY, "docs/wiki/0a1b2c3-glossary.md");

    assert!(
        out.contains(
            "<!-- embed W-0a1b2c3#loop -->\n### Loop\n\n![Loop](0a1b2c3-glossary.md#loop)\n<!-- \
             /embed -->"
        ),
        "{out}"
    );
    assert_eq!(problems, [Problem::Loop(vec![
        "W-0a1b2c3#loop".to_owned(),
        "W-0a1b2c3#loop".to_owned(),
    ])]);
}

/// Reading a page grants nothing about what it embeds: an embed the policy
/// doesn't let the call read is a marker naming it and the refusal, and the
/// rest of the page is still expanded.
#[test]
fn an_embed_the_policy_hides_is_a_marker() {
    let root = workspace();
    let access = Access::granting(root.path(), vec![FsRule::new("docs").with_read(true)]);
    let text =
        "# Page\n\n![Code](../../crates/x/src/lib.rs#L2-4)\n\n![Turn](0a1b2c3-glossary.md#turn)\n";

    let (out, problems) = expand_under(&access, root.path(), text, "docs/wiki/0b0b0b0-page.md");

    assert!(
        out.starts_with(
            "# Page\n\n<!-- embed crates/x/src/lib.rs#L2-L4: not shown (access denied: read on \
             crates/x/src/lib.rs) -->\n\n<!-- embed W-0a1b2c3#turn -->\n## Turn\n"
        ),
        "{out}"
    );
    assert_eq!(problems, [Problem::Denied {
        url: "../../crates/x/src/lib.rs#L2-4".to_owned(),
        refusal: "access denied: read on crates/x/src/lib.rs".to_owned(),
    }]);
}

/// The policy is asked again for what an embedded section embeds.
#[test]
fn a_nested_embed_is_held_to_the_policy_too() {
    let root = workspace();
    fs::write(
        root.path().join("docs/wiki/0a1b2c4-nested.md"),
        "# Nested\n\n## Code\n\n![Code](../../crates/x/src/lib.rs#L1)\n",
    )
    .unwrap();
    let access = Access::granting(root.path(), vec![FsRule::new("docs").with_read(true)]);

    let (out, problems) = expand_under(
        &access,
        root.path(),
        "![Code](0a1b2c4-nested.md#code)\n",
        "docs/wiki/0b0b0b0-page.md",
    );

    assert_eq!(
        out,
        "<!-- embed W-0a1b2c4#code -->\n# Code\n\n<!-- embed crates/x/src/lib.rs#L1-L1: not shown \
         (access denied: read on crates/x/src/lib.rs) -->\n<!-- /embed -->"
    );
    assert_eq!(problems.len(), 1);
}

#[test]
fn an_embed_of_a_missing_section_is_left_alone() {
    let root = workspace();
    let text = "![Gone](0a1b2c3-glossary.md#gone)\n";

    let (out, problems) = expand(root.path(), text, "docs/wiki/0b0b0b0-page.md");

    assert_eq!(out, "![Gone](0a1b2c3-glossary.md#gone)");
    assert!(
        matches!(&problems[..], [Problem::Unresolved(message)] if message.contains("No section `gone`")),
        "{problems:?}"
    );
}

#[test]
fn an_image_that_shares_its_paragraph_is_no_embed() {
    let found = embeds(
        "See ![Turn](a.md#turn) inline.\n\n![Badge](b.svg)\n\n![Turn](a.md#turn)\n",
        Utf8Path::new("/w/docs/wiki"),
    );

    assert_eq!(found.len(), 1);
    assert_eq!(found[0].shown, Shown::Section("turn".to_owned()));
    assert_eq!(found[0].target, "/w/docs/wiki/a.md");
}

#[test]
fn rebase_reads_a_destination_from_another_directory() {
    let from = Utf8Path::new("/w/docs/wiki/0a1b2c3-glossary.md");
    let to = Utf8Path::new("/w/docs/other");

    assert_eq!(
        rebase("#turn", from, to),
        "../wiki/0a1b2c3-glossary.md#turn"
    );
    assert_eq!(rebase("../rfd/100-x.md", from, to), "../rfd/100-x.md");
    assert_eq!(rebase("https://x.y/z", from, to), "https://x.y/z");
    assert_eq!(rebase("/wiki/", from, to), "/wiki/");
}
