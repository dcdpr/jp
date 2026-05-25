//! End-to-end tests through the full pipeline (extract + markdown parsing +
//! sentence splitting + width wrapping).
//! These tests assert on invariants the user-visible contract makes —
//! surrounding code preserved, idempotence, markdown blocks unmolested.

use std::{io, path::PathBuf};

use indoc::indoc;
use pretty_assertions::assert_eq;
use unicode_width::UnicodeWidthStr;

use crate::{
    DEFAULT_MAX_WIDTH, Error,
    format::{
        FormatOptions, format_markdown_canonical, format_markdown_with, format_rust_source_with,
        format_source, format_source_canonical, reflow_markdown,
    },
};

#[test]
fn formatting_is_idempotent() {
    let src = indoc! {"
        /// First sentence here. Second sentence on the same source line, which
        /// should be split by sembr into two separate output lines.
        pub fn f() {}
    "};
    let once = format_source(src, DEFAULT_MAX_WIDTH);
    let twice = format_source(&once, DEFAULT_MAX_WIDTH);
    assert_eq!(once, twice, "format_source must be idempotent");
}

#[test]
fn surrounding_code_unchanged() {
    let src = indoc! {"
        use std::io;

        /// Two sentences here. The splitter will split them.
        pub fn greet() -> io::Result<()> {
            // inline // not a doc
            let s = \"contains /// inside string\";
            println!(\"{s}\");
            Ok(())
        }
    "};
    let out = format_source(src, DEFAULT_MAX_WIDTH);
    assert!(out.contains("use std::io;"));
    assert!(out.contains("    // inline // not a doc"));
    assert!(out.contains("\"contains /// inside string\""));
    assert!(out.contains("    println!(\"{s}\");"));
    assert!(out.contains("    Ok(())"));
}

#[test]
fn fenced_code_block_inside_doc_comment_survives() {
    let src = indoc! {"
        /// Example.
        ///
        /// ```rust
        /// let x = 1;
        /// let y = 2;
        /// ```
        ///
        /// More prose.
        pub fn f() {}
    "};
    let out = format_source(src, DEFAULT_MAX_WIDTH);
    assert!(out.contains("/// ```rust"));
    assert!(out.contains("/// let x = 1;"));
    assert!(out.contains("/// let y = 2;"));
    assert!(out.contains("/// ```"));
    let twice = format_source(&out, DEFAULT_MAX_WIDTH);
    assert_eq!(out, twice);
}

#[test]
fn inner_module_docs_are_handled() {
    let src = indoc! {"
        //! This module does a thing. It does several things, actually.

        pub fn f() {}
    "};
    let out = format_source(src, DEFAULT_MAX_WIDTH);
    assert!(out.contains("//! This module does a thing."));
    assert!(out.contains("//! It does several things, actually."));
}

#[test]
fn max_width_accounts_for_indent_and_prefix() {
    // The user's exact example: a 4-space-indented `//!` block with
    // max_width=10 should fit content within `10 - 4 - 4 = 2` columns.
    // Words longer than 2 chars stay intact (NoHyphenation, break_words=false).
    let src = indoc! {"
        mod m {
            //! foo bar
        }
    "};
    let expected = indoc! {"
        mod m {
            //! foo
            //! bar
        }
    "};
    assert_eq!(format_source(src, 10), expected);
}

#[test]
fn long_urls_are_not_broken_under_tight_max_width() {
    // The other regression we cared about: max_width small enough to want
    // to break a URL, but the URL must stay intact.
    let src = indoc! {"
        /// See https://example.com/path/to/very/long/resource for details.
        pub fn f() {}
    "};
    let out = format_source(src, 20);
    // The URL is on a line by itself but unbroken.
    assert!(
        out.contains("https://example.com/path/to/very/long/resource"),
        "URL was broken: {out}"
    );
}

#[test]
fn max_width_zero_disables_width_wrapping() {
    let src = indoc! {"
        /// One very long sentence with many words that would otherwise wrap.
        pub fn f() {}
    "};
    let out = format_source(src, 0);
    assert!(out.contains("/// One very long sentence with many words that would otherwise wrap."));
}

#[test]
fn max_width_smaller_than_prefix_degrades_to_pure_sembr() {
    let src = indoc! {"
        mod outer {
            mod inner {
                /// First sentence. Second sentence.
                pub fn f() {}
            }
        }
    "};
    let out = format_source(src, 4);
    assert!(out.contains("/// First sentence."));
    assert!(out.contains("/// Second sentence."));
}

#[test]
fn soft_line_breaks_in_paragraph_source_are_collapsed() {
    // Regression: a paragraph spanning multiple source lines (with `///`
    // prefixes preserving its layout) must be reflowed as one logical
    // paragraph, not as multiple line-broken sentences.
    let src = indoc! {"
        /// If `forced_tool` is provided, that tool is included even when its
        /// `enable()` check returns `false`. This prevents a mismatch between
        /// `tool_choice` and the declared tools list.
        pub fn f() {}
    "};
    let out = format_source(src, DEFAULT_MAX_WIDTH);

    // The two sentences each occupy a contiguous run of lines, but neither
    // mid-sentence `///` line break from the input survives — `This\n`
    // followed by `prevents` on the next line was the original bug.
    assert!(!out.contains("`false`.\n/// This\n/// prevents"));
    assert!(!out.contains("that\n/// tool"));

    // Idempotence: running twice produces the same output.
    let twice = format_source(&out, DEFAULT_MAX_WIDTH);
    assert_eq!(out, twice);
}

#[test]
fn reference_link_definitions_survive_end_to_end() {
    let src = indoc! {"
        //! Module docs.
        //!
        //! [`format`]: super::format
        //! [`extract`]: super::extract
        //! [`engine`]: super::engine

        pub fn f() {}
    "};
    let out = format_source(src, DEFAULT_MAX_WIDTH);
    assert!(out.contains("//! [`format`]: super::format"));
    assert!(out.contains("//! [`extract`]: super::extract"));
    assert!(out.contains("//! [`engine`]: super::engine"));
    assert!(!out.contains("super::format ["));
}

#[test]
fn block_quote_round_trips_when_already_sembr() {
    // Input is already one sentence per `> ` line — idempotent under
    // reflow.
    let src = indoc! {"
        /// > This is a note.
        /// > It spans two lines.
        pub fn f() {}
    "};
    let out = format_source(src, DEFAULT_MAX_WIDTH);
    assert_eq!(out, src);
}

#[test]
fn block_quote_reflows_two_sentences_end_to_end() {
    let src = indoc! {"
        /// > Two sentences on one line. Like this.
        pub fn f() {}
    "};
    let expected = indoc! {"
        /// > Two sentences on one line.
        /// > Like this.
        pub fn f() {}
    "};
    assert_eq!(format_source(src, DEFAULT_MAX_WIDTH), expected);
}

#[test]
fn list_items_reflow_with_marker_aligned_continuation_end_to_end() {
    let src = indoc! {"
        /// - First item with two sentences. Like so.
        /// - 100. Outer item. Continues.
        pub fn f() {}
    "};
    // Bulleted list: 2-space continuation. The `100.` text inside the
    // first item is literal (no nested list parsed inside a bullet item
    // without proper formatting), so it just becomes prose.
    let out = format_source(src, DEFAULT_MAX_WIDTH);
    assert!(out.contains("/// - First item with two sentences.\n///   Like so."));
}

#[test]
fn list_item_inside_block_quote_uses_compound_prefix_end_to_end() {
    let src = indoc! {"
        /// > - First. Second.
        pub fn f() {}
    "};
    let expected = indoc! {"
        /// > - First.
        /// >   Second.
        pub fn f() {}
    "};
    assert_eq!(format_source(src, DEFAULT_MAX_WIDTH), expected);
}

#[test]
fn list_item_idempotent_end_to_end() {
    let src = indoc! {"
        /// - First item.
        ///   With continuation.
        /// - Second item.
        pub fn f() {}
    "};
    let once = format_source(src, DEFAULT_MAX_WIDTH);
    let twice = format_source(&once, DEFAULT_MAX_WIDTH);
    assert_eq!(once, twice);
}

#[test]
fn gfm_pipe_table_in_doc_comment_survives_end_to_end() {
    let src = indoc! {"
        /// Examples.
        ///
        /// | name | meaning |
        /// | ---- | ------- |
        /// | foo  | a thing |
        /// | bar  | another |
        ///
        /// See above.
        pub fn f() {}
    "};
    let out = format_source(src, DEFAULT_MAX_WIDTH);
    assert!(out.contains("/// | name | meaning |"));
    assert!(out.contains("/// | ---- | ------- |"));
    assert!(out.contains("/// | foo  | a thing |"));
    assert!(out.contains("/// | bar  | another |"));
    let twice = format_source(&out, DEFAULT_MAX_WIDTH);
    assert_eq!(out, twice);
}

#[test]
fn backslash_hard_break_in_doc_comment_survives_end_to_end() {
    // Address-block-style use of hard breaks: each line is meant to render
    // as a forced `<br>` in rustdoc.
    let src = concat!(
        "/// Example output:\\\n",
        "/// 123 Main St\\\n",
        "/// Springfield\n",
        "pub fn f() {}\n",
    );
    let out = format_source(src, DEFAULT_MAX_WIDTH);
    assert_eq!(out, src);
}

#[test]
fn trailing_spaces_hard_break_in_doc_comment_survives_end_to_end() {
    // The trailing-two-spaces hard-break syntax must survive too — it's
    // the variant whose marker is invisible in plain text and therefore
    // easiest to lose by accident.
    let src = concat!(
        "/// Note: this works.",
        "  \n",
        "/// More info below.\n",
        "pub fn f() {}\n",
    );
    let out = format_source(src, DEFAULT_MAX_WIDTH);
    assert_eq!(out, src);
}

#[test]
fn markdown_paragraph_is_reflowed_end_to_end() {
    // Treated as a raw markdown file (would be invoked as `comfort foo.md`).
    // No `///` prefix; the whole file is markdown.
    let src = indoc! {"
        # Title

        First sentence here. Second sentence on the same source line.

        > A blockquote. With two sentences.

        - Item one. With detail.
        - Item two.

        [^note]: A footnote. With two sentences.

        See[^note] for details.
    "};
    let out = reflow_markdown(src, DEFAULT_MAX_WIDTH);

    // Paragraph reflowed.
    assert!(out.contains("First sentence here.\nSecond sentence on the same source line."));
    // Blockquote reflowed with `> ` continuation.
    assert!(out.contains("> A blockquote.\n> With two sentences."));
    // List item reflowed with 2-space continuation.
    assert!(out.contains("- Item one.\n  With detail."));
    // Footnote reflowed with 4-space continuation.
    assert!(out.contains("[^note]: A footnote.\n    With two sentences."));
    // Heading preserved.
    assert!(out.contains("# Title"));
}

#[test]
fn markdown_frontmatter_is_preserved_verbatim() {
    // YAML frontmatter at the top of the file must not be reflowed; the
    // `title: Foo` line would otherwise look like a one-line paragraph and
    // pass through the sentence splitter as content.
    let src = indoc! {"
        ---
        title: Foo
        date: 2024-01-01
        tags:
          - one
          - two
        ---

        # Heading

        A paragraph. With two sentences.
    "};
    let out = reflow_markdown(src, DEFAULT_MAX_WIDTH);
    // Frontmatter survives byte-for-byte.
    assert!(out.contains("---\ntitle: Foo\ndate: 2024-01-01"));
    assert!(out.contains("  - one\n  - two\n---"));
    // Paragraph below still reflows.
    assert!(out.contains("A paragraph.\nWith two sentences."));
}

#[test]
fn list_item_with_bold_lead_in_keeps_bold_intact() {
    // Regression: a list item whose first sentence ended inside a `**...**`
    // span used to split at the period, leaving the closing `**` on the
    // next line.
    let src = indoc! {"
        - **What every rerank call records.** Provider ID, model name.
    "};
    let out = reflow_markdown(src, 80);
    assert!(
        out.contains("**What every rerank call records.**"),
        "bold span was broken: {out}"
    );
    // And the closing `**` is on the same line as the opening one.
    assert!(
        !out.lines().any(|l| l.trim_start().starts_with("**")),
        "closing `**` got stranded on its own line: {out}"
    );
}

#[test]
fn italic_span_with_period_keeps_emphasis_intact() {
    // Asterisk italics: `*foo.*` should not split at the inner period.
    let src = indoc! {"
        *Foo.* Body sentence here.
    "};
    let out = reflow_markdown(src, 80);
    assert!(
        out.contains("*Foo.* Body sentence here."),
        "italic span broken: {out}"
    );
}

#[test]
fn underscore_italic_with_period_keeps_emphasis_intact() {
    // Underscore italic. The regex fallback would over-match `snake_case`,
    // but the AST knows the right rules.
    let src = indoc! {"
        _Foo._ Body sentence here.
    "};
    let out = reflow_markdown(src, 80);
    assert!(
        out.contains("_Foo._ Body sentence here."),
        "underscore italic broken: {out}"
    );
}

#[test]
fn underscore_bold_with_period_keeps_emphasis_intact() {
    let src = indoc! {"
        __Title.__ Body sentence here.
    "};
    let out = reflow_markdown(src, 80);
    assert!(
        out.contains("__Title.__ Body sentence here."),
        "underscore bold broken: {out}"
    );
}

#[test]
fn triple_asterisk_bold_italic_with_period_keeps_emphasis_intact() {
    // CommonMark `***foo***` is Strong nested in Emph (or vice versa);
    // either way the outer span's AST range covers everything.
    let src = indoc! {"
        ***Title.*** Body sentence here.
    "};
    let out = reflow_markdown(src, 80);
    assert!(
        out.contains("***Title.*** Body sentence here."),
        "triple-asterisk bold-italic broken: {out}"
    );
}

#[test]
fn italic_inside_block_quote_keeps_emphasis_intact() {
    // The block-quote stripping shifts byte offsets, but the re-parse on
    // the cleaned text gives us inline sourcepos in the right coordinate
    // system. Underscore italics (which the regex fallback can't catch
    // without false-matching `snake_case`) survive inside blockquotes too.
    let src = indoc! {"
        > _Foo._ Body sentence here.
    "};
    let out = reflow_markdown(src, 80);
    assert!(
        out.contains("> _Foo._ Body sentence here."),
        "underscore italic broken inside blockquote: {out}"
    );
}

#[test]
fn snake_case_inside_block_quote_is_not_protected() {
    // Inverse of the above: an identifier inside a blockquote must not be
    // treated as italic.
    let src = indoc! {"
        > See foo_bar_baz. Next sentence.
    "};
    let out = reflow_markdown(src, 80);
    assert!(
        out.contains("> See foo_bar_baz.\n> Next sentence."),
        "snake_case got mangled inside blockquote: {out}"
    );
}

#[test]
fn nested_block_quote_emphasis_survives() {
    // Two `>` markers stripped, then re-parsed. Emphasis inside survives.
    let src = indoc! {"
        > > _Foo._ Body sentence here.
    "};
    let out = reflow_markdown(src, 80);
    assert!(
        out.contains("> > _Foo._ Body sentence here."),
        "emphasis broken inside nested blockquote: {out}"
    );
}

#[test]
fn emphasis_spanning_two_source_lines_does_not_over_indent_continuation() {
    // Regression: a list item containing an italic that crosses a source
    // line boundary used to make the continuation line over-indent by
    // four spaces instead of two, because the embedded `\n  ` from the
    // italic span survived into textwrap's view and the container prefix
    // step then doubled the indent.
    let body = indoc! {"
        - Lead in here. *Italics span across
          two source lines*, then more body sentence here.
    "};
    let out = reflow_markdown(body, 80);
    for line in out.lines() {
        // Either column 0 (the list-marker line) or exactly two spaces of
        // continuation indent. Four spaces would be the bug.
        assert!(
            !line.starts_with("    "),
            "line over-indented (4 spaces): {line:?}"
        );
    }
    // And the italic span is now folded onto a single logical sentence
    // — no `\n  ` survives inside.
    assert!(
        !out.contains("*Italics span across\n"),
        "italic span retained its source-level newline: {out}"
    );
}

#[test]
fn inline_code_spanning_two_source_lines_does_not_over_indent_continuation() {
    // Same as above but for inline code spans (reproduction of the
    // `tracing::warn!(...)` case from the original report).
    let body = indoc! {"
        - Emit `tracing::warn!(\"foo bar baz qux quux corge
          grault garply\")` for the legacy field on each launch.
    "};
    let out = reflow_markdown(body, 80);
    for line in out.lines() {
        assert!(
            !line.starts_with("    "),
            "line over-indented (4 spaces): {line:?}"
        );
    }
    // The inline code span is folded onto a single line — the source-level
    // `\n  ` inside it does not survive.
    assert!(
        !out.contains("corge\n"),
        "inline code retained its source-level newline: {out}"
    );
}

#[test]
fn snake_case_identifier_is_not_treated_as_underscore_italic() {
    // The regex `_[^_]+_` would falsely match `_bar_` inside `foo_bar_baz`.
    // The AST approach uses CommonMark rules, which require word-boundary
    // markers for underscore emphasis, so identifiers survive.
    let src = indoc! {"
        See foo_bar_baz. Next sentence.
    "};
    let out = reflow_markdown(src, 80);
    // The identifier survives literally, and the period after it does
    // trigger a sembr split.
    assert!(
        out.contains("See foo_bar_baz.\nNext sentence."),
        "snake_case got mangled: {out}"
    );
}

#[test]
fn markdown_is_idempotent_end_to_end() {
    let src = indoc! {"
        # Title

        Some prose. Two sentences worth.

        - Item. Continued.
    "};
    let once = reflow_markdown(src, DEFAULT_MAX_WIDTH);
    let twice = reflow_markdown(&once, DEFAULT_MAX_WIDTH);
    assert_eq!(once, twice);
}

// ---------------------------------------------------------------------------
// `--format-markdown` (canonical) mode
// ---------------------------------------------------------------------------

#[test]
fn canonical_default_off_preserves_alternate_list_marker_byte_for_byte() {
    // Without `--format-markdown`, `*` bullets stay as `*` even when the
    // markdown content is otherwise reflowable. Default mode is
    // byte-preserving outside paragraphs.
    let body = indoc! {"
        * First item.
        * Second item.
    "};
    let out = reflow_markdown(body, 80);
    assert!(
        out.contains("* First item."),
        "default mode rewrote the bullet marker: {out}"
    );
}

#[test]
fn canonical_mode_normalizes_list_markers_to_dash() {
    // With canonical mode on, comrak's formatter applies our `Dash`
    // preference.
    let body = indoc! {"
        * First item.
        * Second item.
    "};
    let out = format_markdown_canonical(body, 80);
    assert!(
        out.contains("- First item."),
        "canonical mode didn't normalize bullet to dash: {out}"
    );
    assert!(
        !out.contains("* First item."),
        "original `*` marker leaked through: {out}"
    );
}

#[test]
fn canonical_mode_aligns_table_columns() {
    // Misaligned source table; canonical mode should pad data cells to
    // match the widest cell per column.
    let body = indoc! {"
        | A | B |
        |---|---|
        | short | very long content |
        | x | y |
    "};
    let out = format_markdown_canonical(body, 80);
    // Every row's `|` separators should be at consistent column positions.
    let table_lines: Vec<&str> = out
        .lines()
        .filter(|l| l.trim_start().starts_with('|'))
        .collect();
    assert!(
        table_lines.len() >= 4,
        "expected header + separator + 2 data rows, got {} lines",
        table_lines.len()
    );
    let pipe_positions: Vec<Vec<usize>> = table_lines
        .iter()
        .map(|l| {
            l.char_indices()
                .filter(|(_, c)| *c == '|')
                .map(|(i, _)| i)
                .collect()
        })
        .collect();
    let first = &pipe_positions[0];
    for (i, positions) in pipe_positions.iter().enumerate() {
        assert_eq!(
            positions, first,
            "row {i} pipe positions don't align with header: {table_lines:#?}"
        );
    }
}

#[test]
fn canonical_mode_aligns_with_explicit_alignment_markers() {
    // The separator row's colon pattern carries through after alignment.
    let body = indoc! {"
        | left | center | right |
        | :--- | :---: | ---: |
        | a | b | c |
    "};
    let out = format_markdown_canonical(body, 80);
    // Left-aligned column keeps leading `:`, right-aligned trailing `:`,
    // center has both. The dashes get padded to match column width.
    assert!(
        out.contains(":---") && out.contains(":----:") && out.contains("----:"),
        "alignment markers lost or mis-shaped: {out}"
    );
}

#[test]
fn canonical_mode_aligns_table_with_wide_characters() {
    // Wide characters (CJK) count as 2 cells per `UnicodeWidthStr`. The
    // table should align visually, not by codepoint count.
    let body = indoc! {"
        | en | jp |
        |---|---|
        | hi | こんにちは |
        | x | y |
    "};
    let out = format_markdown_canonical(body, 80);
    let table_lines: Vec<&str> = out
        .lines()
        .filter(|l| l.trim_start().starts_with('|'))
        .collect();
    // Pipe positions are computed in BYTE offsets, which won't match for
    // multi-byte CJK rows. The correct check is visual: every row, after
    // the second `|`, the second cell should be padded to the same display
    // width. Approximation: count `|` characters per line — every row
    // should have exactly 3 pipes (start, between cols, end).
    for line in &table_lines {
        let pipe_count = line.chars().filter(|c| *c == '|').count();
        assert_eq!(pipe_count, 3, "row has unexpected pipe count: {line:?}");
    }
    // And the CJK row should be padded such that its right edge `|`
    // lands at the same DISPLAY column as the other rows.
    let display_col_of_last_pipe = |line: &str| -> usize {
        let last_pipe_byte = line.rfind('|').unwrap();
        UnicodeWidthStr::width(&line[..last_pipe_byte])
    };
    let first_last = display_col_of_last_pipe(table_lines[0]);
    for line in &table_lines[1..] {
        assert_eq!(
            display_col_of_last_pipe(line),
            first_last,
            "row's right edge isn't aligned: header={:?} other={:?}",
            table_lines[0],
            line
        );
    }
}

#[test]
fn canonical_mode_table_alignment_is_idempotent() {
    // Aligned table should round-trip unchanged.
    let body = indoc! {"
        | A     | B                 |
        | ----- | ----------------- |
        | short | very long content |
        | x     | y                 |
    "};
    let once = format_markdown_canonical(body, 80);
    let twice = format_markdown_canonical(&once, 80);
    assert_eq!(once, twice);
}

#[test]
fn canonical_mode_still_does_sembr_on_paragraphs() {
    // After canonicalisation, the sembr pipeline still runs on paragraphs.
    let body = indoc! {"
        First sentence. Second sentence on the same line.
    "};
    let out = format_markdown_canonical(body, 80);
    assert!(
        out.contains("First sentence.\nSecond sentence on the same line."),
        "sembr didn't run after canonicalisation: {out}"
    );
}

#[test]
fn canonical_mode_is_idempotent_end_to_end() {
    let body = indoc! {"
        # Heading

        First sentence here. Second sentence here.

        * Item one. With more.
        * Item two.

        | A | B |
        |---|---|
        | x | y |
    "};
    let once = format_markdown_canonical(body, 80);
    let twice = format_markdown_canonical(&once, 80);
    assert_eq!(once, twice, "canonical mode must be idempotent");
}

#[test]
fn canonical_mode_on_rust_source_normalizes_inside_doc_comments() {
    // `format_source_canonical` is the Rust-source entry that runs
    // canonical mode per `///` block.
    let src = indoc! {"
        /// First sentence here. Second sentence here.
        ///
        /// * Item one.
        /// * Item two.
        pub fn f() {}
    "};
    let out = format_source_canonical(src, 80);
    assert!(
        out.contains("/// - Item one."),
        "list markers not normalised inside doc comment: {out}"
    );
    assert!(
        out.contains("/// First sentence here.\n/// Second sentence here."),
        "sembr didn't run inside doc comment: {out}"
    );
    // The surrounding code is byte-preserved as always.
    assert!(out.contains("pub fn f() {}"));
}

#[test]
fn canonical_mode_preserves_doc_comment_scaffolding() {
    // Even with canonical mode on, the `///` prefix and indentation come
    // straight from the original source.
    let src = indoc! {"
        mod m {
            /// Inner doc. Two sentences.
            pub fn f() {}
        }
    "};
    let out = format_source_canonical(src, 80);
    assert!(out.contains("    /// Inner doc."));
    assert!(out.contains("    /// Two sentences."));
}

#[test]
fn canonical_mode_preserves_hard_line_breaks() {
    // Hard breaks (two trailing spaces or `\\\n`) are semantically distinct
    // from soft breaks: they render as `<br>` rather than a space. The
    // canonical pipeline must preserve them through the width=MAX change.
    // comrak normalises two-trailing-spaces to backslash form, which is
    // semantically equivalent and arguably more readable in source.
    let body = "First line.  \nSecond line, hard-broken from first.\n";
    let out = format_markdown_canonical(body, 80);
    assert!(
        out.contains("First line.\\\n") || out.contains("First line.  \n"),
        "hard break lost: {out:?}"
    );
}

#[test]
fn default_mode_preserves_hard_line_breaks_verbatim() {
    // Without --format-markdown, hard breaks pass through byte-for-byte
    // (we don't round-trip through comrak's formatter).
    let body = "First line.  \nSecond line, hard-broken from first.\n";
    let out = reflow_markdown(body, 80);
    assert!(
        out.contains("First line.  \n"),
        "two-trailing-spaces hard break form not preserved verbatim: {out:?}"
    );
}

#[test]
fn canonical_mode_does_not_escape_digit_period_in_continuation_lines() {
    // Regression: with `render.width = 0`, comrak's `format_commonmark`
    // preserves source soft breaks and defensively escapes `N.` sequences
    // (e.g. `404\.`) that land at the start of continuation lines, on the
    // theory that they could be interpreted as ordered-list markers on
    // re-parse. The fix is `render.width = usize::MAX`, which collapses
    // soft breaks so digit-period sequences end up mid-line.
    let body = indoc! {"
        Each model is loaded at startup; requests for unloaded models return HTTP
        404.
        Apply sigmoid normalization next.
    "};
    let out = format_markdown_canonical(body, 80);
    assert!(
        out.contains("404."),
        "output missing literal `404.`: {out:?}"
    );
    assert!(
        !out.contains(r"404\."),
        "output has defensive escape `404\\.`: {out:?}"
    );
}

#[test]
fn canonical_mode_preserves_rust_intra_doc_shortcut_references() {
    // Regression: `[`format_source`]` and similar shortcut references
    // (no `[label]: url` definition in the body) used to be escaped as
    // `[`format_source`]` by comrak's defensive escape logic, because
    // the parser treated them as plain bracketed text. The fix uses a
    // narrow `broken_link_callback` in `protect_reference_form_links`
    // that resolves intra-doc-like labels to `Link` nodes, so the
    // protection step can sentinelise their source bytes.
    let body = indoc! {"
        1. [`format_source`] finds `///` blocks via
           [`find_blocks`] and splices bodies back.
        2. [`reflow_markdown`] parses each block's body.
    "};
    let out = format_markdown_canonical(body, 80);
    for needle in [
        "[`format_source`]",
        "[`find_blocks`]",
        "[`reflow_markdown`]",
    ] {
        assert!(
            out.contains(needle),
            "intra-doc reference {needle:?} missing from output: {out:?}"
        );
    }
    assert!(
        !out.contains(r"\["),
        "defensive bracket escape leaked into output: {out:?}"
    );
}

#[test]
fn intra_doc_callback_does_not_break_task_items() {
    // The `broken_link_callback` would gobble `[ ]` task markers if it
    // returned `Some` for them. Narrow filter (empty / `x` / `X` labels)
    // returns `None`, letting the tasklist extension recognise them.
    let body = indoc! {"
        - [ ] First task. With more detail.
    "};
    let out = reflow_markdown(body, 0);
    assert!(
        out.contains("- [ ] First task."),
        "task marker lost: {out:?}"
    );
    // Continuation indent should be 6 spaces (2 for list padding + 4 for
    // task item), confirming the parser still recognised the task item.
    assert!(
        out.contains("\n      With more detail."),
        "task item continuation indent wrong: {out:?}"
    );
}

#[test]
fn intra_doc_callback_does_not_break_footnotes() {
    // The `broken_link_callback` would gobble `[^note]` references if it
    // returned `Some` for them. Narrow filter (`^...` labels) returns
    // `None`, letting the footnotes extension recognise them.
    let body = indoc! {"
        See[^note] for details.

        [^note]: First sentence. Second sentence.
    "};
    let out = format_markdown_canonical(body, 0);
    // The reference in prose stays as `[^note]` — not the defensive
    // `\[^note\]` we'd see if the parser failed to recognise it as a
    // footnote reference.
    assert!(
        out.contains("See[^note] for details."),
        "footnote reference got escaped: {out:?}"
    );
    assert!(
        !out.contains(r"\[^note\]"),
        "defensive escape leaked into footnote reference: {out:?}"
    );
    // The definition survives the canonical pass (comrak may reshape it
    // — e.g. put the label on its own line — but the content stays).
    assert!(
        out.contains("[^note]:"),
        "footnote definition disappeared: {out:?}"
    );
    assert!(
        out.contains("First sentence.") && out.contains("Second sentence."),
        "footnote definition content lost: {out:?}"
    );
}

#[test]
fn markdown_pipeline_preserves_exact_trailing_newline_count() {
    // Regression: conform.nvim's `injected` formatter extracts the markdown
    // body of Rust doc comments and runs comfort as the markdown formatter
    // on it. The body ending in `\n\n` corresponds to a trailing empty
    // `///` line in the source. If we collapse `\n\n` to `\n`, the empty
    // `///` is silently lost on every save.
    let body_two_newlines = "Some prose.\n\n[link]: https://example.com\n\n";
    let out = format_markdown_canonical(body_two_newlines, 80);
    assert!(
        out.ends_with("\n\n"),
        "trailing newline count not preserved: {out:?}"
    );

    let body_three_newlines = "Some prose.\n\n\n";
    let out = format_markdown_canonical(body_three_newlines, 80);
    assert!(
        out.ends_with("\n\n\n"),
        "trailing newline count not preserved: {out:?}"
    );
}

#[test]
fn canonical_mode_preserves_trailing_newline_of_input() {
    // Markdown files: keep trailing newline. Doc-comment bodies: don't add
    // one.
    let with_newline = "Some prose.\n";
    let out = format_markdown_canonical(with_newline, 80);
    assert!(out.ends_with('\n'), "trailing newline dropped: {out:?}");

    let without_newline = "Some prose.";
    let out = format_markdown_canonical(without_newline, 80);
    assert!(!out.ends_with('\n'), "trailing newline added: {out:?}");
}

// ---------------------------------------------------------------------------
// Reference-form link protection across canonical (`--format-markdown`)
// ---------------------------------------------------------------------------

#[test]
fn canonical_preserves_full_form_reference_link() {
    // Regression: comrak's `format_commonmark` would otherwise inline
    // `[text][label]` as `[text](url)` and drop the definition.
    let body = indoc! {"
        See [`foo`][foo-impl] for more.

        [foo-impl]: ../../crates/foo.rs
    "};
    let out = format_markdown_canonical(body, 80);
    assert!(
        out.contains("[`foo`][foo-impl]"),
        "full-form reference link was inlined: {out}"
    );
    assert!(
        out.contains("[foo-impl]: ../../crates/foo.rs"),
        "reference definition was dropped: {out}"
    );
}

#[test]
fn canonical_preserves_shortcut_form_reference_link() {
    let body = indoc! {"
        See [foo] for more.

        [foo]: https://example.com
    "};
    let out = format_markdown_canonical(body, 80);
    assert!(
        out.contains("See [foo] for more."),
        "shortcut form not preserved: {out}"
    );
    assert!(
        out.contains("[foo]: https://example.com"),
        "definition dropped: {out}"
    );
}

#[test]
fn canonical_still_inlines_actual_inline_links() {
    // Sanity: inline links are NOT protected (the user wrote them inline,
    // they stay inline). This is a guard against the protection logic
    // accidentally over-firing.
    let body = indoc! {"
        See [docs](https://example.com) for more.
    "};
    let out = format_markdown_canonical(body, 80);
    assert!(
        out.contains("[docs](https://example.com)"),
        "inline link got converted to reference: {out}"
    );
}

#[test]
fn canonical_handles_mixed_inline_and_reference_links() {
    let body = indoc! {"
        See [docs](https://example.com) and [`foo`][foo-impl] for more.

        [foo-impl]: ../../crates/foo.rs
    "};
    let out = format_markdown_canonical(body, 80);
    assert!(
        out.contains("[docs](https://example.com)"),
        "inline link mangled: {out}"
    );
    assert!(
        out.contains("[`foo`][foo-impl]"),
        "reference-form link inlined: {out}"
    );
    assert!(
        out.contains("[foo-impl]: ../../crates/foo.rs"),
        "definition dropped: {out}"
    );
}

#[test]
fn canonical_preserves_user_chosen_labels_with_reference_links_flag() {
    // The original bug: with BOTH `--format-markdown` and `--reference-links`,
    // the user's chosen short labels were destroyed. Verify they survive now.
    let body = indoc! {"
        See [`verify_file_checksum`][verify-impl] for the impl.

        [verify-impl]: ../../crates/jp_mcp/src/client.rs
    "};
    let opts = FormatOptions {
        max_width: 80,
        canonical: true,
        reference_links: true,
    };
    let out = format_markdown_with(body, &opts);
    assert!(
        out.contains("[verify-impl]: ../../crates/jp_mcp/src/client.rs"),
        "user's `verify-impl` label was rewritten: {out}"
    );
    assert!(
        out.contains("[`verify_file_checksum`][verify-impl]"),
        "reference form not preserved: {out}"
    );
}

#[test]
fn canonical_protection_only_affects_resolved_reference_links() {
    // Bare `[brackets]` with no matching definition aren't reference-form
    // links — comrak doesn't parse them as Link nodes — so our protection
    // doesn't touch them. Comrak itself escapes the brackets during
    // serialisation to disambiguate (a behaviour of `format_commonmark`,
    // not our protection), so the output has `\[...\]`. The point of this
    // test is the negative: our protection didn't spuriously stash these.
    let body = indoc! {"
        Use [square brackets] in prose freely.
    "};
    let out = format_markdown_canonical(body, 80);
    // No sentinel marker leaked into the output (would start with `XCMFRTLR`).
    assert!(
        !out.contains("XCMFRTLR"),
        "sentinel leaked into output: {out}"
    );
    // The visible text "square brackets" survives in some form.
    assert!(
        out.contains("square brackets"),
        "prose content disappeared: {out}"
    );
}

#[test]
fn canonical_protection_ignores_definitions_inside_code_fences() {
    let body = indoc! {"
        Real link: [foo].

        ```
        [example]: not-a-real-def
        ```

        [foo]: https://example.com
    "};
    let out = format_markdown_canonical(body, 80);
    // The fake def inside the fence stays in the fence.
    let fence_close = out.rfind("```").unwrap();
    let example_pos = out.find("[example]: not-a-real-def").unwrap();
    assert!(
        example_pos < fence_close,
        "fake def was extracted out of the fence: {out}"
    );
    // The real def survives.
    assert!(
        out.contains("[foo]: https://example.com"),
        "real definition dropped: {out}"
    );
}

// ---------------------------------------------------------------------------
// `--reference-links` (reference-link extraction) mode
// ---------------------------------------------------------------------------

fn ref_opts(max_width: usize) -> FormatOptions {
    FormatOptions {
        max_width,
        canonical: false,
        reference_links: true,
    }
}

#[test]
fn reference_links_default_off_preserves_inline_links() {
    // Without `--reference-links`, inline links pass through unchanged.
    let body = indoc! {"
        See [docs](https://example.com) for more.
    "};
    let out = reflow_markdown(body, 80);
    assert!(
        out.contains("[docs](https://example.com)"),
        "default mode rewrote the inline link: {out}"
    );
}

#[test]
fn reference_links_converts_inline_to_shortcut_form() {
    let body = indoc! {"
        See [docs](https://example.com) for more.
    "};
    let out = format_markdown_with(body, &ref_opts(80));
    assert!(
        out.contains("See [docs] for more."),
        "inline link not converted to shortcut form: {out}"
    );
    assert!(
        out.contains("[docs]: https://example.com"),
        "reference definition not appended: {out}"
    );
}

#[test]
fn reference_links_dedupes_same_url() {
    // Same URL referenced twice with different text: second link uses full
    // form referring back to the first's canonical label — only one
    // definition is emitted.
    let body = indoc! {"
        See [docs](https://example.com) and [more docs](https://example.com).
    "};
    let out = format_markdown_with(body, &ref_opts(80));
    assert!(
        out.contains("[docs]") && out.contains("[more docs][docs]"),
        "same-URL collision not handled with full-form fallback: {out}"
    );
    assert_eq!(
        out.matches("[docs]: https://example.com").count(),
        1,
        "shared URL got more than one definition: {out}"
    );
}

#[test]
fn reference_links_disambiguates_same_text_different_url() {
    // Same text, different URLs: second link gets a suffixed label.
    let body = indoc! {"
        See [docs](https://example.com) and [docs](https://other.com).
    "};
    let out = format_markdown_with(body, &ref_opts(80));
    assert!(
        out.contains("[docs]: https://example.com"),
        "first definition missing: {out}"
    );
    assert!(
        out.contains("[docs-2]: https://other.com"),
        "disambiguated definition missing: {out}"
    );
    assert!(
        out.contains("[docs][docs-2]"),
        "second link not in full form: {out}"
    );
}

#[test]
fn reference_links_skips_anchor_links() {
    let body = indoc! {"
        See [section](#foo) for more.
    "};
    let out = format_markdown_with(body, &ref_opts(80));
    assert!(
        out.contains("[section](#foo)"),
        "anchor link should not be converted: {out}"
    );
}

#[test]
fn reference_links_skips_image_links() {
    let body = indoc! {"
        See ![diagram](https://example.com/d.png) below.
    "};
    let out = format_markdown_with(body, &ref_opts(80));
    assert!(
        out.contains("![diagram](https://example.com/d.png)"),
        "image link should not be converted: {out}"
    );
}

#[test]
fn reference_links_aggregates_pre_existing_definitions() {
    // Pre-existing scattered definitions should also move to the bottom
    // and sort alphabetically with the newly converted ones.
    let body = indoc! {"
        See [zebra] and [alpha](https://alpha.example).

        [zebra]: https://zebra.example
    "};
    let out = format_markdown_with(body, &ref_opts(80));
    // Both definitions should be at the bottom, in alphabetical order.
    let alpha_pos = out.find("[alpha]: https://alpha.example").unwrap();
    let zebra_pos = out.find("[zebra]: https://zebra.example").unwrap();
    assert!(
        alpha_pos < zebra_pos,
        "definitions not sorted alphabetically: {out}"
    );
}

#[test]
fn reference_links_preserves_inline_code_with_link_syntax() {
    // Inline code containing `[link](url)` syntax must NOT be converted.
    let body = indoc! {"
        Use the syntax `[text](url)` to write links.
    "};
    let out = format_markdown_with(body, &ref_opts(80));
    assert!(
        out.contains("`[text](url)`"),
        "inline code with link syntax got mangled: {out}"
    );
    assert!(
        !out.contains("[text]: url"),
        "link inside inline code spuriously generated a definition: {out}"
    );
}

#[test]
fn reference_links_is_idempotent() {
    let body = indoc! {"
        See [docs](https://example.com) and [Rust](https://rust-lang.org).
    "};
    let once = format_markdown_with(body, &ref_opts(80));
    let twice = format_markdown_with(&once, &ref_opts(80));
    assert_eq!(once, twice, "reference-link mode must be idempotent");
}

#[test]
fn reference_links_works_with_rust_doc_comments() {
    // The original motivating example from the user.
    let src = indoc! {"
        /// Source language to format.
        /// With [`Auto`](Language::Auto), per-file detection (extension or
        /// `--stdin-filename`) determines the format.
        pub fn f() {}
    "};
    let out = format_rust_source_with(src, &ref_opts(80));
    assert!(
        out.contains("/// With [`Auto`],"),
        "link not converted in doc comment: {out}"
    );
    assert!(
        out.contains("/// [`Auto`]: Language::Auto"),
        "reference definition not at bottom of doc comment: {out}"
    );
    assert!(out.contains("pub fn f() {}"));
}

#[test]
fn reference_links_composes_with_canonical_mode() {
    // Both flags enabled: canonical pass runs first (normalising structure),
    // then reference-link extraction. Both transformations should apply.
    let body = indoc! {"
        * See [docs](https://example.com).
        * Another [item](https://other.com).
    "};
    let opts = FormatOptions {
        max_width: 80,
        canonical: true,
        reference_links: true,
    };
    let out = format_markdown_with(body, &opts);
    // Canonical: `*` → `-`.
    assert!(
        out.contains("- See [docs]"),
        "canonical pass didn't normalise list marker: {out}"
    );
    // Reference: definitions at the bottom.
    assert!(
        out.contains("[docs]: https://example.com") && out.contains("[item]: https://other.com"),
        "reference-link pass didn't run: {out}"
    );
}

#[test]
fn reference_links_preserves_inline_link_title() {
    // Regression: `[docs](url "Title")` used to round-trip as
    // `[docs] + [docs]: url`, silently dropping the title metadata.
    let body = indoc! {r#"
        See [docs](https://example.com "Docs Title") for more.
    "#};
    let out = format_markdown_with(body, &ref_opts(80));
    assert!(
        out.contains("See [docs] for more."),
        "inline link not converted to shortcut form: {out}"
    );
    assert!(
        out.contains(r#"[docs]: https://example.com "Docs Title""#),
        "reference definition lost its title: {out}"
    );
}

#[test]
fn reference_links_disambiguates_same_url_with_different_titles() {
    // Two links pointing at the same URL but carrying different titles
    // must get distinct definitions — otherwise the title of one is
    // silently dropped during dedup.
    let body = indoc! {r#"
        See [primary](https://example.com "Primary view") and
        [alternate](https://example.com "Alternate view").
    "#};
    let out = format_markdown_with(body, &ref_opts(80));
    assert!(
        out.contains(r#"[primary]: https://example.com "Primary view""#),
        "first definition missing or titleless: {out}"
    );
    assert!(
        out.contains(r#"[alternate]: https://example.com "Alternate view""#),
        "second definition missing or titleless: {out}"
    );
    // Both link sites should use shortcut form (each label was free).
    assert!(
        out.contains("[primary]") && out.contains("[alternate]"),
        "link sites didn't pick up their reference forms: {out}"
    );
}

#[test]
fn reference_links_dedupes_same_url_same_title() {
    // Same URL AND same title: a single definition, both link sites point
    // at the same canonical label (full form for the second to preserve
    // its different link text).
    let body = indoc! {r#"
        See [docs](https://example.com "Docs") and
        [more docs](https://example.com "Docs").
    "#};
    let out = format_markdown_with(body, &ref_opts(80));
    assert_eq!(
        out.matches(r#"[docs]: https://example.com "Docs""#).count(),
        1,
        "shared (url, title) got more than one definition: {out}"
    );
    assert!(
        out.contains("[more docs][docs]"),
        "second link not in full-form referring back to the first: {out}"
    );
}

#[test]
fn reference_links_preserves_existing_definition_with_title() {
    // A pre-existing scattered `[foo]: url "title"` definition must come
    // out the other end with its title intact (and moved to the bottom).
    let body = indoc! {r#"
        See [foo] for more.

        [foo]: https://example.com "Foo title"
    "#};
    let out = format_markdown_with(body, &ref_opts(80));
    assert!(
        out.contains(r#"[foo]: https://example.com "Foo title""#),
        "existing definition lost its title: {out}"
    );
}

#[test]
fn reference_links_with_titles_is_idempotent() {
    let body = indoc! {r#"
        See [docs](https://example.com "D") and [other](https://other.com "O").
    "#};
    let once = format_markdown_with(body, &ref_opts(80));
    let twice = format_markdown_with(&once, &ref_opts(80));
    assert_eq!(
        once, twice,
        "reference-link mode with titles must be idempotent"
    );
}

#[test]
fn reference_links_handles_case_insensitive_label_collisions() {
    // Regression: CommonMark reference labels match case-insensitively
    // (§4.7). An existing `[Foo]: /old` must collide with an inline
    // `[foo](/new)` even though the raw strings differ in case —
    // otherwise we'd emit two definitions with the same canonical label
    // and the renderer would resolve the converted shortcut to whichever
    // came first.
    let body = indoc! {"
        See [Foo] and [foo](/new).

        [Foo]: /old
    "};
    let out = format_markdown_with(body, &ref_opts(80));
    assert!(
        out.contains("[Foo]: /old"),
        "existing definition lost: {out}"
    );
    assert!(
        out.contains("[foo-2]: /new"),
        "disambiguated definition for new URL missing: {out}"
    );
    assert!(
        out.contains("[foo][foo-2]"),
        "new link doesn't reference the disambiguated label: {out}"
    );
}

#[test]
fn reference_links_handles_whitespace_normalized_label_collisions() {
    // CommonMark §4.7 normalises internal whitespace too: `[foo bar]` and
    // `[Foo   Bar]` are the same label.
    let body = indoc! {"
        See [Foo   Bar] and [foo bar](/new).

        [Foo   Bar]: /old
    "};
    let out = format_markdown_with(body, &ref_opts(80));
    // The new link's URL is different, so it must get a disambiguated
    // label even though `foo bar` looks free to a raw-string lookup.
    assert!(
        out.contains("[foo bar-2]: /new"),
        "whitespace-collision not disambiguated: {out}"
    );
}

#[test]
fn reference_links_does_not_extract_def_that_interrupts_paragraph() {
    // Regression: CommonMark forbids reference definitions from
    // interrupting a paragraph. `Foo\n[bar]: /baz` is one paragraph, and
    // the `[bar]: /baz` line is visible prose — not a definition. The
    // line-shape extractor used to take it out anyway and re-emit it
    // below, silently changing rendered content.
    let body = "Foo\n[bar]: /baz\n";
    let out = format_markdown_with(body, &ref_opts(80));
    assert!(
        !out.contains("\n\n[bar]: /baz"),
        "in-paragraph ref-def shape was extracted to a separate block: {out:?}"
    );
    assert!(
        out.contains("[bar]: /baz"),
        "the [bar]: /baz text disappeared from the output: {out:?}"
    );
}

#[test]
fn reference_links_still_extracts_legitimately_separated_definitions() {
    // Canary for the fix above: a definition that's NOT inside a paragraph
    // (separated by a blank line) must still be extracted and consolidated
    // at the bottom. The paragraph-protection rule has to be specific
    // enough not to swallow this case.
    let body = indoc! {"
        Some prose.

        [foo]: /bar
    "};
    let out = format_markdown_with(body, &ref_opts(80));
    assert!(
        out.contains("[foo]: /bar"),
        "legitimate ref-def lost: {out}"
    );
}

#[test]
fn reference_links_skips_definitions_inside_fenced_code() {
    // A `[label]: url` line inside a fenced code block must NOT be treated
    // as a reference definition (it's literal example text).
    let body = indoc! {"
        Real link: [docs](https://example.com).

        ```
        [example]: https://not-a-real-def.com
        ```
    "};
    let out = format_markdown_with(body, &ref_opts(80));
    // The fake def inside the fence should stay where it is.
    assert!(
        out.contains("[example]: https://not-a-real-def.com"),
        "fake definition inside fence got extracted: {out}"
    );
    // It should appear inside the fence, not at the bottom.
    let example_pos = out.find("[example]: https://not-a-real-def.com").unwrap();
    let fence_close = out.rfind("```").unwrap();
    assert!(
        example_pos < fence_close,
        "fake definition extracted out of fence: {out}"
    );
}

#[test]
fn no_doc_comments_means_byte_identical_output() {
    let src = indoc! {"
        fn main() {
            // ordinary comment
            let x = 42;
            println!(\"{x}\");
        }
    "};
    assert_eq!(format_source(src, DEFAULT_MAX_WIDTH), src);
}

#[test]
fn read_file_error_carries_the_path() {
    let err = Error::ReadFile {
        path: PathBuf::from("/tmp/nope.rs"),
        source: io::Error::new(io::ErrorKind::PermissionDenied, "denied"),
    };
    let msg = err.to_string();
    assert!(msg.contains("/tmp/nope.rs"), "missing path: {msg}");
    assert!(msg.contains("denied"), "missing source: {msg}");
}

#[test]
fn write_file_error_carries_the_path() {
    let err = Error::WriteFile {
        path: PathBuf::from("/tmp/nope.rs"),
        source: io::Error::new(io::ErrorKind::PermissionDenied, "denied"),
    };
    let msg = err.to_string();
    assert!(msg.contains("/tmp/nope.rs"), "missing path: {msg}");
    assert!(msg.contains("denied"), "missing source: {msg}");
}
