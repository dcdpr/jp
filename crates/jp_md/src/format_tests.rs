use super::*;

struct TestCase {
    input: &'static str,
    output: &'static str,
}

#[expect(clippy::needless_pass_by_value)]
fn run_test(name: &str, case: TestCase) {
    let formatter = Formatter::new();
    let actual = formatter.format_terminal(case.input).unwrap();
    assert_eq!(actual, case.output, "failed case: {name}");
}

#[test]
fn test_terminal_strong() {
    let cases = vec![
        ("simple", TestCase {
            input: "Hello **World**!",
            output: "Hello \u{1b}[1m**World**\u{1b}[22m!\n",
        }),
        ("multiple", TestCase {
            input: "Hi **One** and Hi **Two**!",
            output: "Hi \u{1b}[1m**One**\u{1b}[22m and Hi \u{1b}[1m**Two**\u{1b}[22m!\n",
        }),
        ("nested", TestCase {
            input: "***Hello***!",
            output: "\u{1b}[3m*\u{1b}[1m**Hello**\u{1b}[22m*\u{1b}[23m!\n",
        }),
    ];

    for (name, case) in cases {
        run_test(name, case);
    }
}

#[test]
fn test_terminal_emphasized() {
    let cases = vec![
        ("simple", TestCase {
            input: "Hello _World_!",
            output: "Hello \u{1b}[3m*World*\u{1b}[23m!\n",
        }),
        ("multiple", TestCase {
            input: "Hi _One_ and Hi *Two*!",
            output: "Hi \u{1b}[3m*One*\u{1b}[23m and Hi \u{1b}[3m*Two*\u{1b}[23m!\n",
        }),
        ("nested", TestCase {
            input: "*Hello*!",
            output: "\u{1b}[3m*Hello*\u{1b}[23m!\n",
        }),
    ];

    for (name, case) in cases {
        run_test(name, case);
    }
}

#[test]
fn test_terminal_underlined() {
    let cases = vec![("simple", TestCase {
        input: "Hello __World__!",
        output: "Hello \u{1b}[4m__World__\u{1b}[24m!\n",
    })];

    for (name, case) in cases {
        run_test(name, case);
    }
}

#[test]
fn test_terminal_strikethrough() {
    let cases = vec![("simple", TestCase {
        input: "Hello ~~World~~!",
        output: "Hello \u{1b}[9m~~World~~\u{1b}[29m!\n",
    })];

    for (name, case) in cases {
        run_test(name, case);
    }
}

#[test]
fn test_terminal_code() {
    let cases = vec![("simple", TestCase {
        input: "Hello `World`!",
        output: "Hello \x1b[48;2;34;34;34m`World`\x1b[49m!\n",
    })];

    for (name, case) in cases {
        run_test(name, case);
    }
}

#[test]
fn test_terminal_blockquote() {
    let cases = vec![("simple", TestCase {
        input: "> Hello World!",
        output: "\u{1b}[38;2;131;148;150m> Hello World!\u{1b}[39m\n",
    })];

    for (name, case) in cases {
        run_test(name, case);
    }
}

#[test]
fn test_no_escaped_special_characters() {
    let cases = vec![
        ("exclamation_mark", TestCase {
            input: "Hello World!!",
            output: "Hello World!!\n",
        }),
        ("question_mark", TestCase {
            input: "Hello World??",
            output: "Hello World??\n",
        }),
        ("parentheses", TestCase {
            input: "Hello World()?",
            output: "Hello World()?\n",
        }),
        ("square_brackets", TestCase {
            input: "Hello World[]?",
            output: "Hello World[]?\n",
        }),
        ("curly_braces", TestCase {
            input: "Hello World{}?",
            output: "Hello World{}?\n",
        }),
        ("angle_brackets", TestCase {
            input: "Hello World<>?",
            output: "Hello World<>?\n",
        }),
    ];

    for (name, case) in cases {
        run_test(name, case);
    }
}

#[test]
fn test_no_trailing_background_after_wrapped_inline_code() {
    // Bug: when comrak soft-wraps a paragraph at `width: 80` and an
    // inline code span falls near the wrap boundary, the ANSI escape
    // codes injected as Raw nodes are counted as visible characters by
    // comrak's line-width calculation. This causes:
    //
    // 1. Premature line wrapping (comrak thinks the line is wider than
    //    it visually is).
    // 2. Trailing spaces on the wrapped line that inherit the active
    //    background color from the inline code escape sequence.
    //
    // The user sees colored whitespace extending past the text content
    // on lines that follow a wrapped inline code span.
    let formatter = Formatter::new();

    // Use the user's exact example: a paragraph with multiple inline
    // code spans where wrapping causes the background to bleed.
    // First example from the user's bug report:
    let input = "**`use` statement split into two blocks** — A standard `use` block appears at \
                 the top, then `signal_stream` is defined, then a second `use super::{ ... }` \
                 block appears. All `use` statements should be at the top per Rust convention.";

    assert_no_bg_bleed(&formatter, input, "use statement example");

    // Second example from the user's bug report:
    let input = "**Inline `ToolConfig` construction is verbose** — Each test constructs \
                 `ToolConfig { source: ..., command: ..., run: ..., enable: None, description: \
                 None, parameters: IndexMap::new(), result: None, style: None, questions: \
                 IndexMap::new() }`. A builder or `ToolConfig::test(source, run_mode)` helper \
                 would eliminate the 6 `None`/default fields.";

    assert_no_bg_bleed(&formatter, input, "ToolConfig example");

    // Third example from the user's bug report:
    let input = "Looking at the user's example more carefully, this IS a blockquote with \
                 continuation lines. When comrak renders those lines, it adds the `>` prefix. But \
                 that's only 2 characters, nowhere near 91.";

    assert_no_bg_bleed(&formatter, input, "blockquote example");
}

/// Assert that no line in the formatted terminal output has an unclosed
/// background color escape (which would cause the terminal to render
/// the background color for the remainder of the line).
#[track_caller]
fn assert_no_bg_bleed(formatter: &Formatter, input: &str, case: &str) {
    let output = formatter.format_terminal(input).unwrap();
    let bg_end = "\x1b[49m";
    let full_reset = "\x1b[0m";
    for (i, line) in output.lines().enumerate() {
        // Count any background-set escape (\x1b[48;...).
        let starts = line.matches("\x1b[48;").count();
        // Both \x1b[49m and \x1b[0m close the background.
        let ends = line.matches(bg_end).count() + line.matches(full_reset).count();
        assert!(
            starts <= ends,
            "[{case}] Line {i} has {starts} bg start(s) but {ends} bg end(s) — background color \
             bleeds to end of terminal line.\nLine: {line:?}\nFull output: {output:?}"
        );
    }
}

#[test]
fn test_table_wrapping_end_to_end() {
    let formatter = Formatter::new(); // default max_column_width = 40
    let input = "| Name | Description |\n| --- | --- |\n| short | brief |\n| long | This is an \
                 extremely long description that definitely exceeds the forty character column \
                 width limit |\n";
    let output = formatter.format_terminal(input).unwrap();

    // No truncation markers — content is wrapped, not truncated.
    assert!(
        !output.contains('…'),
        "should not truncate, should wrap:\n{output}"
    );

    // Full text must be preserved.
    let plain: String = output
        .lines()
        .flat_map(|l| l.chars())
        .filter(|c| !c.is_control() && *c != '|')
        .collect();
    let normalized: String = plain.split_whitespace().collect::<Vec<_>>().join(" ");
    assert!(
        normalized.contains("column width limit"),
        "full cell text should be preserved:\n{output}"
    );

    // No line should be excessively wide.
    for line in output.lines() {
        let vw: usize = {
            let mut len = 0;
            let mut in_esc = false;
            for c in line.chars() {
                if in_esc {
                    if c.is_ascii_alphabetic() || c == '~' {
                        in_esc = false;
                    }
                } else if c == '\x1b' {
                    in_esc = true;
                } else if !c.is_control() {
                    len += 1;
                }
            }
            len
        };
        assert!(vw <= 100, "line too wide ({vw} chars): {line:?}");
    }
}

#[test]
fn test_terminal_blockquote_nested() {
    // This test captures a bug where nested blockquotes cause the outer
    // blockquote's styling to be prematurely terminated.
    //
    // The issue: when exiting the inner blockquote, the formatter emits
    // FG_END (\x1b[39m) which resets the foreground color, even though
    // we're still inside the outer blockquote.
    let input = "> This is a blockquote.\n>\n> > It can be nested.\n>\n> Still in outer.\n";

    let formatter = Formatter::new();
    let actual = formatter.format_terminal(input).unwrap();

    // The text "Still in outer." should still have the blockquote styling
    // applied (gray foreground), but due to the bug, the FG_END from the
    // nested blockquote resets it prematurely.
    //
    // We check that the gray color code appears AFTER the nested blockquote
    // content, indicating the outer blockquote styling is restored.
    let nested_end_pos = actual.find("It can be nested").unwrap();
    let still_in_outer_pos = actual.find("Still in outer").unwrap();

    // Find color codes between the nested blockquote and "Still in outer"
    let between = &actual[nested_end_pos..still_in_outer_pos];

    // After the nested blockquote ends, we should re-apply the outer
    // blockquote's foreground color. Currently this fails because the
    // formatter doesn't track nesting depth.
    assert!(
        between.contains("\x1b[38;"),
        "Expected outer blockquote color to be restored after nested blockquote.\nActual \
         output:\n{actual:?}"
    );
}

#[test]
fn test_default_background_terminal_fill() {
    let opts = TerminalOptions {
        default_background: Some(DefaultBackground {
            color: 236,
            fill: BackgroundFill::Terminal,
        }),
    };

    // With wrapping enabled (width > 0)
    let actual = Formatter::new()
        .format_terminal_with("Deep thought", &opts)
        .unwrap();
    assert!(
        actual.contains("\x1b[48;5;236m"),
        "Should set background. Got: {actual:?}"
    );
    assert!(
        actual.contains("\x1b[K"),
        "Should contain erase-to-EOL. Got: {actual:?}"
    );
    assert!(
        actual.contains("\x1b[0m"),
        "Should contain RESET. Got: {actual:?}"
    );

    // With wrapping disabled (width == 0)
    let actual = Formatter::with_width(0)
        .format_terminal_with("Deep thought", &opts)
        .unwrap();
    assert!(
        actual.contains("\x1b[48;5;236m"),
        "width=0: Should set background. Got: {actual:?}"
    );
    assert!(
        actual.contains("\x1b[K"),
        "width=0: Should contain erase-to-EOL. Got: {actual:?}"
    );
    assert!(
        actual.contains("\x1b[0m"),
        "width=0: Should contain RESET. Got: {actual:?}"
    );
}

#[test]
fn test_thematic_break_default_line_style() {
    // Default hr_style is Line — renders a unicode horizontal line.
    let formatter = Formatter::new();
    let actual = formatter.format_terminal("above\n\n---\n\nbelow").unwrap();
    let line: String = "─".repeat(80);
    assert!(
        actual.contains(&line),
        "Expected 80-char unicode line (default width).\nActual: {actual:?}"
    );
}

#[test]
fn test_thematic_break_markdown_style() {
    // HrStyle::Markdown renders the standard `-----`.
    let mut formatter = Formatter::new();
    formatter.hr_style = HrStyle::Markdown;
    let actual = formatter.format_terminal("above\n\n---\n\nbelow").unwrap();
    assert!(
        actual.contains("-----"),
        "Expected markdown-style thematic break.\nActual: {actual:?}"
    );
}

#[test]
fn test_thematic_break_line_style_uses_configured_width() {
    // HrStyle::Line should produce a line of `─` characters using
    // the configured wrap width when no terminal_width is set.
    let mut formatter = Formatter::with_width(40);
    formatter.hr_style = HrStyle::Line;

    let actual = formatter.format_terminal("above\n\n---\n\nbelow").unwrap();
    let line: String = "─".repeat(40);
    assert!(
        actual.contains(&line),
        "Expected 40-char unicode line.\nActual: {actual:?}"
    );
}

#[test]
fn test_thematic_break_line_style_uses_terminal_width() {
    // When terminal_width is set, it takes precedence over the
    // configured wrap width.
    let mut formatter = Formatter::with_width(40).terminal_width(120);
    formatter.hr_style = HrStyle::Line;

    let actual = formatter.format_terminal("above\n\n---\n\nbelow").unwrap();
    let line: String = "─".repeat(120);
    assert!(
        actual.contains(&line),
        "Expected 120-char unicode line.\nActual: {actual:?}"
    );
    // Should NOT contain a 40-char line (unless it's a substring, but
    // we check exact length by verifying no extra `─` beyond 120).
    assert!(
        !actual.contains(&"─".repeat(121)),
        "Line should not exceed terminal width.\nActual: {actual:?}"
    );
}
