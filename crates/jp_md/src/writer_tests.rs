use super::*;
use crate::format::{BackgroundFill, DefaultBackground};

/// Regression test: a 1-byte prefix used to underflow in `write_prefix` because
/// `prefix.len() - 2` wraps to `usize::MAX`.
#[test]
fn write_prefix_single_byte_no_panic() {
    let mut buf = String::new();
    let mut w = TerminalWriter::new(&mut buf, 80, None, 0);

    write!(w.prefix, ">").unwrap();

    // Force a newline so the next output triggers write_prefix.
    w.output("a\nb", true).unwrap();
    w.finish().unwrap();

    assert!(buf.contains('>'), "prefix byte should appear in output");
}

/// Sanity check: empty prefix still works (was already guarded by early
/// return).
#[test]
fn write_prefix_empty() {
    let mut buf = String::new();
    let mut w = TerminalWriter::new(&mut buf, 80, None, 0);

    w.output("hello\nworld", true).unwrap();
    w.finish().unwrap();

    assert_eq!(buf, "hello\nworld\n");
}

/// Characterizes the writer's default-background handling, which is driven by
/// `AnsiState`'s tracking and restore.
/// Locks the behaviour in so the compound-SGR parsing change to `AnsiState`
/// cannot silently regress the markdown path's full-width shading.
#[test]
fn default_background_is_restored_across_a_line_break() {
    let mut buf = String::new();
    let bg = DefaultBackground {
        param: "48;5;236".to_string(),
        fill: BackgroundFill::Terminal,
    };
    let mut w = TerminalWriter::new(&mut buf, 0, Some(&bg), 0);

    w.output("one\ntwo", true).unwrap();
    w.finish().unwrap();

    assert!(
        buf.contains("\x1b[K"),
        "lines are erased to the edge: {buf:?}"
    );
    assert!(
        buf.contains("one") && buf.contains("two"),
        "content present: {buf:?}"
    );
    // The background must be re-established after the newline, so the second
    // line still renders shaded.
    let after_newline = buf.split_once('\n').map_or("", |(_, rest)| rest);
    assert!(
        after_newline.contains("48;5;236"),
        "background restored on the next line: {buf:?}"
    );
}
