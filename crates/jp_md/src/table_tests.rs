use super::*;
use crate::format::HrStyle;

#[test]
fn test_pad_cell_left() {
    assert_eq!(pad_cell("hi", 10, TableAlignment::Left), "hi        ");
}

#[test]
fn test_pad_cell_right() {
    assert_eq!(pad_cell("hi", 10, TableAlignment::Right), "        hi");
}

#[test]
fn test_pad_cell_center() {
    assert_eq!(pad_cell("hi", 10, TableAlignment::Center), "    hi    ");
}

#[test]
fn test_wrap_fits_no_wrap() {
    let lines = wrap_to_visual_width("hello", 10);
    assert_eq!(lines, vec!["hello"]);
}

#[test]
fn test_wrap_unlimited() {
    let lines = wrap_to_visual_width("hello world", 0);
    assert_eq!(lines, vec!["hello world"]);
}

#[test]
fn test_wrap_word_boundary() {
    let lines = wrap_to_visual_width("hello world", 7);
    assert_eq!(lines, vec!["hello", "world"]);
}

#[test]
fn test_wrap_multiple_lines() {
    let lines = wrap_to_visual_width("aa bb cc dd", 5);
    assert_eq!(lines, vec!["aa bb", "cc dd"]);
}

#[test]
fn test_wrap_hard_break() {
    let lines = wrap_to_visual_width("abcdefghij", 4);
    assert_eq!(lines, vec!["abcd", "efgh", "ij"]);
}

#[test]
fn test_wrap_preserves_ansi() {
    // Bold word that fits on one line — no wrapping needed.
    let input = "\x1b[1m**bold**\x1b[22m rest";
    let lines = wrap_to_visual_width(input, 40);
    assert_eq!(lines.len(), 1);
    assert_eq!(lines[0], input);
}

#[test]
fn test_wrap_ansi_across_lines() {
    // Bold spans a word boundary that forces a wrap.
    // "aaaa " is 5 chars, then "\x1b[1m**bb**\x1b[22m" is 6 visible.
    // With max_width=8, "aaaa " + 6 > 8, so bold word goes to next line.
    let input = "aaaa \x1b[1m**bb**\x1b[22m";
    let lines = wrap_to_visual_width(input, 8);
    assert_eq!(lines.len(), 2);
    // First line should just be "aaaa" (trailing space trimmed or not).
    assert_eq!(ansi::visual_width(&lines[0]), 4);
    // Second line should contain the bold word.
    assert!(lines[1].contains("**bb**"));
}

#[test]
fn test_wrap_ansi_state_continues() {
    // A bold span wraps across lines — the continuation line should
    // re-open the bold escape and the first line should reset it.
    let input = "\x1b[1m**aa bb**\x1b[22m";
    let lines = wrap_to_visual_width(input, 6);
    assert_eq!(lines.len(), 2);
    // First line ends with reset.
    assert!(
        lines[0].contains("\x1b[0m"),
        "first line should reset ANSI: {:?}",
        lines[0]
    );
    // Second line re-opens bold.
    assert!(
        lines[1].starts_with("\x1b[1m"),
        "second line should reopen bold: {:?}",
        lines[1]
    );
}

#[test]
fn test_format_simple_table() {
    let arena = comrak::Arena::new();
    let options = comrak::Options {
        extension: comrak::options::Extension {
            table: true,
            ..Default::default()
        },
        ..Default::default()
    };
    let input = "| A | B | C |\n| --- | --- | --- |\n| 1 | 22 | 333 |\n| xx | y | zzzz |\n";
    let root = comrak::parse_document(&arena, input, &options);

    // Find the table node.
    let table_node = root.first_child().expect("should have table");
    let theme = crate::theme::resolve(None);
    let opts = TableOptions::new(0);
    let hr_opts = crate::render::HrOptions {
        style: HrStyle::Markdown,
        terminal_width: None,
    };
    let result = format_table(table_node, &opts, &hr_opts, &theme, None).expect("should format");

    // Verify alignment: all columns should have consistent pipe positions.
    let lines: Vec<&str> = result.trim().lines().collect();
    assert!(lines.len() >= 3, "should have header + sep + rows");

    // All lines should start and end with |.
    for line in &lines {
        assert!(line.starts_with('|'), "line should start with |: {line}");
        assert!(line.ends_with('|'), "line should end with |: {line}");
    }

    // Pipe positions should be consistent across all content rows.
    let pipe_positions: Vec<Vec<usize>> = lines
        .iter()
        .enumerate()
        .filter(|(i, _)| *i != 1) // skip separator
        .map(|(_, line)| {
            line.char_indices()
                .filter(|(_, c)| *c == '|')
                .map(|(i, _)| i)
                .collect()
        })
        .collect();
    for (i, pos) in pipe_positions.iter().enumerate().skip(1) {
        assert_eq!(*pos, pipe_positions[0], "pipe positions differ at row {i}");
    }
}

#[test]
fn test_format_table_with_wrapping() {
    let arena = comrak::Arena::new();
    let options = comrak::Options {
        extension: comrak::options::Extension {
            table: true,
            ..Default::default()
        },
        ..Default::default()
    };
    let input = "| Name | Description |\n| --- | --- |\n| short | brief |\n| long | This is a \
                 very long description that should be wrapped at the max column width |\n";
    let root = comrak::parse_document(&arena, input, &options);
    let table_node = root.first_child().expect("should have table");
    let theme = crate::theme::resolve(None);
    let opts = TableOptions::new(20);
    let hr_opts = crate::render::HrOptions {
        style: HrStyle::Markdown,
        terminal_width: None,
    };
    let result = format_table(table_node, &opts, &hr_opts, &theme, None).expect("should format");

    // Every line must respect the width cap.
    for line in result.lines() {
        let vw = ansi::visual_width(line);
        // | + sp + col1(20) + sp + | + sp + col2(20) + sp + | = 49
        assert!(vw <= 49, "line too wide ({vw} chars): {line:?}");
    }

    // The long cell should have produced extra visual rows.
    // header + separator + short row + long row (multiple lines)
    let line_count = result.lines().count();
    assert!(
        line_count > 4,
        "expected wrapped rows, got {line_count} lines:\n{result}"
    );

    // The full text must still be present (no truncation).
    // Normalize whitespace since padding spaces break up the content.
    let plain: String = result
        .lines()
        .flat_map(|l| l.chars())
        .filter(|c| !c.is_control() && *c != '|')
        .collect();
    let normalized: String = plain.split_whitespace().collect::<Vec<_>>().join(" ");
    assert!(
        normalized.contains("should be wrapped at the max column width"),
        "full cell text should be preserved:\n{result}"
    );
}

#[test]
fn test_format_table_wrapping_respects_alignment() {
    let arena = comrak::Arena::new();
    let options = comrak::Options {
        extension: comrak::options::Extension {
            table: true,
            ..Default::default()
        },
        ..Default::default()
    };
    // Right-aligned second column.
    let input = "| H1 | H2 |\n| --- | ---: |\n| a | short |\n| b | wrap me here please |\n";
    let root = comrak::parse_document(&arena, input, &options);
    let table_node = root.first_child().expect("should have table");
    let theme = crate::theme::resolve(None);
    let opts = TableOptions::new(10);
    let hr_opts = crate::render::HrOptions {
        style: HrStyle::Markdown,
        terminal_width: None,
    };
    let result = format_table(table_node, &opts, &hr_opts, &theme, None).expect("should format");

    // All data lines should have consistent pipe positions.
    let data_lines: Vec<&str> = result
        .lines()
        .enumerate()
        .filter(|(i, _)| *i != 1) // skip separator
        .map(|(_, l)| l)
        .collect();
    let first_pipes: Vec<usize> = data_lines[0]
        .char_indices()
        .filter(|(_, c)| *c == '|')
        .map(|(i, _)| i)
        .collect();
    for (i, line) in data_lines.iter().enumerate().skip(1) {
        let pipes: Vec<usize> = line
            .char_indices()
            .filter(|(_, c)| *c == '|')
            .map(|(i, _)| i)
            .collect();
        assert_eq!(
            pipes, first_pipes,
            "pipe positions differ at data line {i}: {line:?}"
        );
    }

    // The wrapped continuation line for the right-aligned column
    // should have leading spaces (right-padding).
    let continuation = result
        .lines()
        .find(|l| l.contains("please"))
        .expect("should find continuation line with 'please'");
    // Extract the right-aligned cell content (between last two pipes).
    let segments: Vec<&str> = continuation.split('|').collect();
    let right_cell = segments[segments.len() - 2]; // last cell before trailing empty
    // Right-aligned: content should be right-justified (leading spaces).
    assert!(
        right_cell.starts_with("  "),
        "right-aligned continuation should have leading spaces: {right_cell:?}"
    );
}

#[test]
fn test_format_aligned_table() {
    let arena = comrak::Arena::new();
    let options = comrak::Options {
        extension: comrak::options::Extension {
            table: true,
            ..Default::default()
        },
        ..Default::default()
    };
    let input =
        "| Left | Center | Right |\n| :--- | :---: | ---: |\n| a | b | c |\n| dd | ee | ff |\n";
    let root = comrak::parse_document(&arena, input, &options);
    let table_node = root.first_child().expect("should have table");
    let theme = crate::theme::resolve(None);
    let opts = TableOptions::new(0);
    let hr_opts = crate::render::HrOptions {
        style: HrStyle::Markdown,
        terminal_width: None,
    };
    let result = format_table(table_node, &opts, &hr_opts, &theme, None).expect("should format");

    // Separator should contain alignment markers.
    let lines: Vec<&str> = result.trim().lines().collect();
    let sep = lines[1];
    assert!(sep.contains(":--"), "should have left align marker: {sep}");
    assert!(sep.contains(":-"), "should have center left marker: {sep}");
    assert!(sep.contains("-:"), "should have right marker: {sep}");
}
