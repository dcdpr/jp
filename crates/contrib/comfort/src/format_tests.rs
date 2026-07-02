//! Tests for the format pipeline.
//! The engine trait is gone, so these tests exercise the real sentence splitter
//! directly.
//! Output is deterministic and idempotent, so we assert on exact strings where
//! it's useful and on invariants otherwise.

use indoc::indoc;
use pretty_assertions::assert_eq;

use super::{format_markdown_canonical, format_source, reflow_markdown, reflow_paragraph};

// ---------------------------------------------------------------------------
// reflow_paragraph: sentence splitting + width wrapping
// ---------------------------------------------------------------------------

#[test]
fn paragraph_splits_two_sentences_onto_their_own_lines() {
    let out = reflow_paragraph("Hello world. This is a test.", &[], 0);
    assert_eq!(out, "Hello world.\nThis is a test.");
}

#[test]
fn paragraph_single_sentence_returns_single_line() {
    let out = reflow_paragraph("Just one sentence.", &[], 0);
    assert_eq!(out, "Just one sentence.");
}

#[test]
fn paragraph_width_wraps_at_word_boundaries() {
    let out = reflow_paragraph("alpha beta gamma delta epsilon zeta.", &[], 12);
    for line in out.lines() {
        assert!(line.len() <= 12, "line exceeded max_width: {line:?}");
    }
    assert!(out.lines().count() > 1);
}

#[test]
fn paragraph_does_not_break_long_unbreakable_tokens() {
    // A URL longer than `max_width` must stay on one line rather than be
    // split mid-token.
    let url = "https://example.com/path/to/very/long/resource";
    let input = format!("Visit {url} for details.");
    let out = reflow_paragraph(&input, &[], 10);
    // The URL appears intact on some line.
    assert!(
        out.lines().any(|l| l.contains(url)),
        "URL was broken: {out:?}"
    );
}

#[test]
fn paragraph_idempotent_under_repeated_reflow() {
    let input = "First sentence here. Second sentence too. Third for good measure.";
    let once = reflow_paragraph(input, &[], 30);
    let twice = reflow_paragraph(&once, &[], 30);
    assert_eq!(once, twice);
}

#[test]
fn paragraph_empty_input_returns_empty() {
    assert_eq!(reflow_paragraph("", &[], 0), "");
    assert_eq!(reflow_paragraph("   ", &[], 0), "");
}

// ---------------------------------------------------------------------------
// reflow_markdown: comrak-driven block awareness
// ---------------------------------------------------------------------------

#[test]
fn reference_link_definitions_are_preserved_verbatim() {
    let body = indoc! {"
        [`format`]: super::format
        [`extract`]: super::extract
        [`engine`]: super::engine
    "};
    assert_eq!(reflow_markdown(body, 0), body);
    // Idempotent under width-wrapping too.
    assert_eq!(reflow_markdown(body, 80), body);
}

#[test]
fn paragraph_with_trailing_ref_link_defs_reflows_only_the_paragraph() {
    let body = indoc! {"
        First. Second.

        [foo]: bar
        [baz]: qux
    "};
    let expected = indoc! {"
        First.
        Second.

        [foo]: bar
        [baz]: qux
    "};
    assert_eq!(reflow_markdown(body, 0), expected);
}

#[test]
fn canonical_block_quote_blank_lines_have_no_trailing_whitespace() {
    // Regression: comrak's CommonMark formatter writes the `> ` block-quote
    // prefix on blank lines, leaving `> ` with trailing whitespace that git
    // and editors flag. The canonical pass must strip it.
    let body = indoc! {"
        > First sentence. Second sentence here.
        >
        > - Item one. More detail.
    "};
    let expected = indoc! {"
        > First sentence.
        > Second sentence here.
        >
        > - Item one.
        >   More detail.
    "};
    assert_eq!(format_markdown_canonical(body, 0), expected);
}

#[test]
fn canonical_preserves_marker_only_line_inside_code_block() {
    // A `> ` line inside a fenced code block is literal sample content, not a
    // generated block-quote prefix, so its trailing space must survive.
    let body = concat!("```\n", "> \n", "```\n");
    assert_eq!(format_markdown_canonical(body, 0), body);
}

#[test]
fn block_quote_two_sentences_split_to_two_lines() {
    let body = indoc! {"
        > First sentence. Second sentence.
    "};
    let expected = indoc! {"
        > First sentence.
        > Second sentence.
    "};
    assert_eq!(reflow_markdown(body, 0), expected);
}

#[test]
fn block_quote_multi_line_paragraph_is_reflowed_as_one_logical_para() {
    // The `>` markers on continuation lines must be stripped before sembr;
    // otherwise they leak into sentence content and double on output.
    let body = indoc! {"
        > First sentence here. This second sentence
        > continues onto another line.
    "};
    let expected = indoc! {"
        > First sentence here.
        > This second sentence continues onto another line.
    "};
    assert_eq!(reflow_markdown(body, 0), expected);
}

#[test]
fn block_quote_single_sentence_stays_on_one_line() {
    let body = indoc! {"
        > A B C D E F.
    "};
    assert_eq!(reflow_markdown(body, 0), body);
}

#[test]
fn nested_block_quote_uses_compound_prefix() {
    let body = indoc! {"
        > > First. Second.
    "};
    let expected = indoc! {"
        > > First.
        > > Second.
    "};
    assert_eq!(reflow_markdown(body, 0), expected);
}

#[test]
fn alert_body_reflows_like_block_quote() {
    // GFM `> [!NOTE]` admonition. The `[!NOTE]` header line stays put,
    // body paragraph is sembr'd with `> ` continuation.
    let body = indoc! {"
        > [!NOTE]
        > First sentence. Second sentence.
    "};
    let expected = indoc! {"
        > [!NOTE]
        > First sentence.
        > Second sentence.
    "};
    assert_eq!(reflow_markdown(body, 0), expected);
}

#[test]
fn alert_multi_line_body_strips_continuation_markers() {
    // The `>` on continuation lines inside the body must be stripped
    // before sembr (same logic as plain block quotes).
    let body = indoc! {"
        > [!WARNING]
        > First sentence here.
        > Second sentence here.
    "};
    let expected = indoc! {"
        > [!WARNING]
        > First sentence here.
        > Second sentence here.
    "};
    assert_eq!(reflow_markdown(body, 0), expected);
}

#[test]
fn footnote_definition_continuation_uses_four_space_indent() {
    // CommonMark's footnotes extension specifies 4 spaces of continuation
    // indent regardless of the label width. Comrak only retains footnote
    // definitions in the AST when they're actually referenced, so the test
    // includes a reference too.
    let body = indoc! {"
        See[^note] for details.

        [^note]: First sentence. Second sentence.
    "};
    let expected = indoc! {"
        See[^note] for details.

        [^note]: First sentence.
            Second sentence.
    "};
    assert_eq!(reflow_markdown(body, 0), expected);
}

#[test]
fn footnote_definition_long_label_still_four_spaces() {
    // Continuation indent is the spec's 4 spaces, *not* aligned with the
    // label width.
    let body = indoc! {"
        Like[^very-long-label] this.

        [^very-long-label]: First. Second.
    "};
    let expected = indoc! {"
        Like[^very-long-label] this.

        [^very-long-label]: First.
            Second.
    "};
    assert_eq!(reflow_markdown(body, 0), expected);
}

#[test]
fn orphan_footnote_definitions_are_preserved_verbatim() {
    // Comrak silently drops unreferenced footnote definitions from the AST;
    // we can't reflow what we can't see, but the source bytes survive
    // intact because nothing in the AST triggers a replacement.
    let body = indoc! {"
        [^orphan]: Some text. More text.
    "};
    assert_eq!(reflow_markdown(body, 0), body);
}

#[test]
fn block_directive_reflows_interior_without_per_line_prefix() {
    // `:::name` block directive: like multiline block quote, delimiters
    // sit on their own lines and content inside has no per-line prefix.
    let body = indoc! {"
        :::warning
        First sentence. Second sentence.
        :::
    "};
    let expected = indoc! {"
        :::warning
        First sentence.
        Second sentence.
        :::
    "};
    assert_eq!(reflow_markdown(body, 0), expected);
}

#[test]
fn multiline_block_quote_reflows_interior_without_per_line_prefix() {
    // `>>>` block quote: delimiters are unique to their own lines, the
    // content inside is unprefixed.
    let body = indoc! {"
        >>>
        First sentence. Second sentence.
        >>>
    "};
    let expected = indoc! {"
        >>>
        First sentence.
        Second sentence.
        >>>
    "};
    assert_eq!(reflow_markdown(body, 0), expected);
}

#[test]
fn fenced_code_blocks_are_preserved_verbatim() {
    let body = indoc! {"
        Some prose.

        ```rust
        let x = 1;
        let y = 2;
        ```

        More prose.
    "};
    let out = reflow_markdown(body, 0);
    assert!(out.contains("```rust\nlet x = 1;\nlet y = 2;\n```"));
}

#[test]
fn list_items_each_reflow_independently() {
    let body = indoc! {"
        - First item. With two sentences.
        - Second item. Also two.
    "};
    let expected = indoc! {"
        - First item.
          With two sentences.
        - Second item.
          Also two.
    "};
    assert_eq!(reflow_markdown(body, 0), expected);
}

#[test]
fn ordered_list_item_uses_three_space_continuation() {
    let body = indoc! {"
        1. First step. With detail.
        2. Second step.
    "};
    let expected = indoc! {"
        1. First step.
           With detail.
        2. Second step.
    "};
    assert_eq!(reflow_markdown(body, 0), expected);
}

#[test]
fn align_tables_covers_full_last_row_with_trailing_code_span() {
    // Comrak reports the table's end sourcepos short of the last row's final
    // `|` when that row's last cell ends with a code span and another block
    // follows the table in the same list item. Without snapping the
    // replacement range to the line end, the stale tail bytes survive as
    // `` ` | `` junk after the rewritten table.
    let body = indoc! {r#"
        1. Intro:

           | A         | B      |
           | --------- | ------ |
           | `x = "y"` | `Code` |

           Trailing paragraph inside the same item.
    "#};
    let out = crate::format::format_markdown_canonical(body, 80);
    assert_eq!(out, body, "table must survive with no trailing junk");
    assert_eq!(crate::format::format_markdown_canonical(&out, 80), out);
}

#[test]
fn canonical_preserves_table_inside_list_item() {
    // Regression: `align_tables` emitted rows `2..n` at column 0, losing the
    // list-item continuation indent that sits inside the replaced source
    // range. The de-indented rows no longer re-parsed as a table, so the
    // reflow pass collapsed them into wrapped prose.
    let body = indoc! {"
        6. Some intro text:

           | Head A | Head B |
           | ------ | ------ |
           | a1     | b1     |
           | a2     | b2     |
    "};
    let out = crate::format::format_markdown_canonical(body, 80);
    assert_eq!(out, body, "in-item table must survive canonicalization");
    // The canonical form is a fixed point.
    assert_eq!(crate::format::format_markdown_canonical(&out, 80), out);
}

#[test]
fn canonical_preserves_table_inside_block_quote() {
    // Same regression class as the list-item case: continuation lines inside
    // a block quote carry a `> ` prefix that lives inside the replaced range.
    let body = indoc! {"
        > Intro:
        >
        > | Head A | Head B |
        > | ------ | ------ |
        > | a1     | b1     |
    "};
    let out = crate::format::format_markdown_canonical(body, 80);
    assert_eq!(out, body, "in-quote table must survive canonicalization");
    assert_eq!(crate::format::format_markdown_canonical(&out, 80), out);
}

#[test]
fn list_item_continuation_indent_matches_marker_width() {
    // `100. ` is a 5-char marker, so continuation lines should be indented
    // by 5 spaces.
    let body = indoc! {"
        100. A very long item with several sentences. Like this one.
    "};
    let expected = indoc! {"
        100. A very long item with several sentences.
             Like this one.
    "};
    assert_eq!(reflow_markdown(body, 0), expected);
}

#[test]
fn task_item_continuation_aligns_with_text_after_checkbox() {
    // `- [ ] ` is 6 chars total: 2 for the bullet marker, 4 for `[X] `.
    // Continuation lines should land at column 7.
    let body = indoc! {"
        - [ ] First task. With more detail.
    "};
    let expected = indoc! {"
        - [ ] First task.
              With more detail.
    "};
    assert_eq!(reflow_markdown(body, 0), expected);
}

#[test]
fn checked_task_item_aligns_the_same_as_unchecked() {
    let body = indoc! {"
        - [x] Done thing. Some explanation.
    "};
    let expected = indoc! {"
        - [x] Done thing.
              Some explanation.
    "};
    assert_eq!(reflow_markdown(body, 0), expected);
}

#[test]
fn task_items_in_a_list_each_get_aligned_continuation() {
    let body = indoc! {"
        - [ ] First. With more.
        - [x] Second. With more.
    "};
    let expected = indoc! {"
        - [ ] First.
              With more.
        - [x] Second.
              With more.
    "};
    assert_eq!(reflow_markdown(body, 0), expected);
}

#[test]
fn list_item_in_block_quote_uses_compound_prefix() {
    let body = indoc! {"
        > - First. Second.
    "};
    let expected = indoc! {"
        > - First.
        >   Second.
    "};
    assert_eq!(reflow_markdown(body, 0), expected);
}

#[test]
fn gfm_pipe_tables_are_preserved_verbatim() {
    // Tables are gated on the `table` extension. Without it comrak parses
    // each row as a soft-broken paragraph, which sembr would then split
    // mid-row.
    let body = indoc! {"
        Some prose.

        | head | row |
        | ---- | --- |
        | a    | b   |
        | c    | d   |

        More prose. Two sentences.
    "};
    let expected = indoc! {"
        Some prose.

        | head | row |
        | ---- | --- |
        | a    | b   |
        | c    | d   |

        More prose.
        Two sentences.
    "};
    assert_eq!(reflow_markdown(body, 0), expected);
    // Idempotent across width-wrapping too.
    let once = reflow_markdown(body, 80);
    let twice = reflow_markdown(&once, 80);
    assert_eq!(once, twice);
}

#[test]
fn paragraph_with_backslash_hard_break_is_preserved_verbatim() {
    // GFM hard break: `\` at end of line. The paragraph stays untouched
    // even when it contains content sembr would otherwise split.
    let body = "Foo. Bar.\\\nBaz.\n";
    assert_eq!(reflow_markdown(body, 0), body);
}

#[test]
fn paragraph_with_trailing_spaces_hard_break_is_preserved_verbatim() {
    // GFM hard break: two trailing spaces before `\n`. `collapse_whitespace`
    // would silently eat the marker, so we opt out of reflow.
    let body = concat!("Foo. Bar.", "  \n", "Baz.\n");
    assert_eq!(reflow_markdown(body, 0), body);
}

#[test]
fn hard_break_nested_inside_emphasis_is_preserved() {
    // Regression: `has_hard_line_break` used to only check direct paragraph
    // children, so a hard break under an `Emph` (or any other inline
    // container) was invisible. The emphasis span was then treated as
    // atomic and `fold_line_breaks` collapsed the hard break into a space.
    let body = concat!("*first.", "  \n", "second*\n");
    assert_eq!(reflow_markdown(body, 80), body);
}

#[test]
fn hard_break_nested_inside_link_text_is_preserved() {
    // Same problem class as the emphasis case: a hard break under a Link
    // node escapes the direct-children check.
    let body = concat!("[first.", "  \n", "second](https://example.com)\n");
    assert_eq!(reflow_markdown(body, 80), body);
}

#[test]
fn backslash_hard_break_nested_inside_emphasis_is_preserved() {
    // Backslash form of hard break, nested inside emphasis. Same protection.
    let body = "*first.\\\nsecond*\n";
    assert_eq!(reflow_markdown(body, 80), body);
}

#[test]
fn hard_break_only_skips_its_own_paragraph() {
    // A paragraph with a hard break stays verbatim; siblings still reflow.
    let body = concat!(
        "First sentence. Second sentence.\n",
        "\n",
        "Has break.",
        "  \n",
        "Stays put.\n",
        "\n",
        "Third sentence. Fourth sentence.\n",
    );
    let expected = concat!(
        "First sentence.\nSecond sentence.\n",
        "\n",
        "Has break.",
        "  \n",
        "Stays put.\n",
        "\n",
        "Third sentence.\nFourth sentence.\n",
    );
    assert_eq!(reflow_markdown(body, 0), expected);
}

#[test]
fn atx_headings_are_preserved_verbatim() {
    let body = indoc! {"
        # A Heading

        Some prose.
    "};
    let out = reflow_markdown(body, 0);
    assert!(out.contains("# A Heading"));
    assert!(out.contains("Some prose."));
}

#[test]
fn body_with_no_top_level_paragraphs_is_unchanged() {
    let body = "[`x`]: y\n";
    assert_eq!(reflow_markdown(body, 0), body);
}

#[test]
fn empty_body_returns_empty() {
    assert_eq!(reflow_markdown("", 0), "");
}

// ---------------------------------------------------------------------------
// format_source: full pipeline including extract + reassemble
// ---------------------------------------------------------------------------

#[test]
fn empty_source_returns_empty() {
    assert_eq!(format_source("", 0), "");
}

#[test]
fn source_without_doc_comments_is_unchanged() {
    let src = indoc! {"
        fn main() {
            let x = 1; // not a doc comment
            println!(\"{x}\");
        }
    "};
    assert_eq!(format_source(src, 0), src);
}

#[test]
fn multiple_blocks_are_all_reformatted() {
    let src = indoc! {"
        /// First. Second.
        fn one() {}

        /// Third. Fourth.
        fn two() {}
    "};
    let expected = indoc! {"
        /// First.
        /// Second.
        fn one() {}

        /// Third.
        /// Fourth.
        fn two() {}
    "};
    assert_eq!(format_source(src, 0), expected);
}

#[test]
fn reassembly_uses_block_indent() {
    let src = indoc! {"
        mod m {
            /// Hello. World.
            fn f() {}
        }
    "};
    let expected = indoc! {"
        mod m {
            /// Hello.
            /// World.
            fn f() {}
        }
    "};
    assert_eq!(format_source(src, 0), expected);
}

#[test]
fn trailing_newline_is_preserved() {
    let src = "/// foo bar baz.\nfn f() {}\n";
    let out = format_source(src, 0);
    assert!(out.ends_with('\n'));
}

#[test]
fn surrounding_code_is_preserved_verbatim() {
    let src = indoc! {"
        use std::io;

        /// Greet. Politely.
        pub fn greet() {
            // inline comment with weird chars: !@#$%
            let s = \"contains /// inside string\";
            println!(\"{s}\");
        }
    "};
    let out = format_source(src, 0);
    assert!(out.contains("use std::io;"));
    assert!(out.contains("// inline comment with weird chars: !@#$%"));
    assert!(out.contains("\"contains /// inside string\""));
    assert!(out.contains("/// Greet."));
    assert!(out.contains("/// Politely."));
}

#[test]
fn format_source_reflows_paragraphs_but_preserves_ref_link_defs() {
    let src = indoc! {"
        //! Some prose. Sentence two.
        //!
        //! [`x`]: y
        //! [`z`]: w
        fn f() {}
    "};
    let out = format_source(src, 100);
    assert!(out.contains("//! Some prose.\n//! Sentence two."));
    assert!(out.contains("//! [`x`]: y\n//! [`z`]: w"));
}
