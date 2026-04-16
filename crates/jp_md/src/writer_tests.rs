use std::fmt::Write;

use super::*;

/// Regression test: a 1-byte prefix used to underflow in `write_prefix`
/// because `prefix.len() - 2` wraps to `usize::MAX`.
#[test]
fn write_prefix_single_byte_no_panic() {
    let mut buf = String::new();
    let mut w = TerminalWriter::new(&mut buf, 80, None);

    write!(w.prefix, ">").unwrap();

    // Force a newline so the next output triggers write_prefix.
    w.output("a\nb", true).unwrap();
    w.finish().unwrap();

    assert!(buf.contains('>'), "prefix byte should appear in output");
}

/// Sanity check: empty prefix still works (was already guarded by early return).
#[test]
fn write_prefix_empty() {
    let mut buf = String::new();
    let mut w = TerminalWriter::new(&mut buf, 80, None);

    w.output("hello\nworld", true).unwrap();
    w.finish().unwrap();

    assert_eq!(buf, "hello\nworld\n");
}
