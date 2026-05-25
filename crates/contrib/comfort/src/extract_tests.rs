use indoc::indoc;
use pretty_assertions::assert_eq;
use ra_ap_rustc_lexer::DocStyle;

use super::{Block, find_blocks};

#[test]
fn finds_single_outer_block() {
    let src = "/// One line.\nfn f() {}\n";
    let blocks = find_blocks(src);
    assert_eq!(blocks.len(), 1);
    assert_eq!(blocks[0].style, DocStyle::Outer);
    assert_eq!(blocks[0].indent, "");
    assert_eq!(blocks[0].lines, vec!["One line."]);
    // Range covers `/// One line.` (13 chars), not the trailing newline.
    assert_eq!(&src[blocks[0].range.clone()], "/// One line.");
}

#[test]
fn finds_inner_doc_block() {
    let src = "//! Module docs.\n//! Second line.\n";
    let blocks = find_blocks(src);
    assert_eq!(blocks.len(), 1);
    assert_eq!(blocks[0].style, DocStyle::Inner);
    assert_eq!(blocks[0].lines, vec!["Module docs.", "Second line."]);
}

#[test]
fn groups_consecutive_outer_lines_into_one_block() {
    let src = indoc! {"
        /// First.
        /// Second.
        /// Third.
        fn f() {}
    "};
    let blocks = find_blocks(src);
    assert_eq!(blocks.len(), 1);
    assert_eq!(blocks[0].lines, vec!["First.", "Second.", "Third."]);
}

#[test]
fn preserves_empty_doc_lines_within_block() {
    let src = indoc! {"
        /// First paragraph.
        ///
        /// Second paragraph.
        fn f() {}
    "};
    let blocks = find_blocks(src);
    assert_eq!(blocks.len(), 1);
    assert_eq!(blocks[0].lines, vec![
        "First paragraph.",
        "",
        "Second paragraph."
    ]);
}

#[test]
fn separates_blocks_across_blank_source_line() {
    let src = indoc! {"
        /// First block.

        /// Second block.
        fn f() {}
    "};
    let blocks = find_blocks(src);
    assert_eq!(blocks.len(), 2);
    assert_eq!(blocks[0].lines, vec!["First block."]);
    assert_eq!(blocks[1].lines, vec!["Second block."]);
}

#[test]
fn separates_blocks_across_intervening_code() {
    let src = indoc! {"
        /// First.
        fn f() {}
        /// Second.
        fn g() {}
    "};
    let blocks = find_blocks(src);
    assert_eq!(blocks.len(), 2);
    assert_eq!(blocks[0].lines, vec!["First."]);
    assert_eq!(blocks[1].lines, vec!["Second."]);
}

#[test]
fn separates_outer_from_inner_block() {
    // Different doc styles never merge, even with no intervening code.
    let src = indoc! {"
        //! Module doc.
        /// Item doc.
        fn f() {}
    "};
    let blocks = find_blocks(src);
    assert_eq!(blocks.len(), 2);
    assert_eq!(blocks[0].style, DocStyle::Inner);
    assert_eq!(blocks[1].style, DocStyle::Outer);
}

#[test]
fn captures_indentation() {
    let src = indoc! {"
        mod m {
            /// Indented doc.
            /// Second line.
            fn f() {}
        }
    "};
    let blocks = find_blocks(src);
    assert_eq!(blocks.len(), 1);
    assert_eq!(blocks[0].indent, "    ");
    assert_eq!(blocks[0].lines, vec!["Indented doc.", "Second line."]);
}

#[test]
fn skips_trailing_doc_after_code_on_same_line() {
    // `///` only triggers a doc-comment when it starts the line. A `///`
    // after code on the same line is still a doc-comment token to the
    // lexer but it's misplaced; we ignore it.
    let src = "let x = 5; /// not really a doc\n/// real doc\nfn f() {}\n";
    let blocks = find_blocks(src);
    assert_eq!(blocks.len(), 1);
    assert_eq!(blocks[0].lines, vec!["real doc"]);
}

#[test]
fn ignores_triple_slash_inside_string_literals() {
    let src = "fn f() { let s = \"/// not a doc\"; }\n/// real doc\nfn g() {}\n";
    let blocks = find_blocks(src);
    assert_eq!(blocks.len(), 1);
    assert_eq!(blocks[0].lines, vec!["real doc"]);
}

#[test]
fn ignores_block_doc_comments() {
    let src = "/** outer block doc */\n/*! inner block doc */\n/// real doc\nfn f() {}\n";
    let blocks = find_blocks(src);
    assert_eq!(blocks.len(), 1);
    assert_eq!(blocks[0].lines, vec!["real doc"]);
}

#[test]
fn preserves_extra_leading_whitespace_for_markdown_code_blocks() {
    // `///     foo` (4 extra spaces) becomes `    foo` in the body, which is
    // a 4-space-indented markdown code block. Only ONE separator space is
    // stripped.
    let src = "/// para\n///     code_block_line\n";
    let blocks = find_blocks(src);
    assert_eq!(blocks.len(), 1);
    assert_eq!(blocks[0].lines, vec!["para", "    code_block_line"]);
}

#[test]
fn block_marker_returns_outer_or_inner() {
    let outer = Block {
        range: 0..0,
        indent: String::new(),
        style: DocStyle::Outer,
        lines: vec![],
    };
    let inner = Block {
        range: 0..0,
        indent: String::new(),
        style: DocStyle::Inner,
        lines: vec![],
    };
    assert_eq!(outer.marker(), "///");
    assert_eq!(inner.marker(), "//!");
}

#[test]
fn reassemble_uses_indent_and_marker() {
    let block = Block {
        range: 0..0,
        indent: "    ".to_owned(),
        style: DocStyle::Outer,
        lines: vec![],
    };
    let formatted = "First line.\n\nSecond paragraph.";
    let out = block.reassemble(formatted);
    assert_eq!(
        out,
        "    /// First line.\n    ///\n    /// Second paragraph."
    );
}

#[test]
fn reassemble_does_not_add_trailing_space_on_empty_lines() {
    let block = Block {
        range: 0..0,
        indent: String::new(),
        style: DocStyle::Outer,
        lines: vec![],
    };
    let out = block.reassemble("a\n\nb");
    assert_eq!(out, "/// a\n///\n/// b");
    // Verify there's no `/// ` (with trailing space) on the empty line.
    assert!(!out.contains("/// \n"));
}
