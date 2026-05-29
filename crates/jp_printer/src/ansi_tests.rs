use super::*;

/// Feed each chunk through a fresh stripper and return the stripped output.
fn strip_chunks(chunks: &[&[u8]]) -> String {
    let mut stripper = AnsiStripper::new(Vec::new());
    for chunk in chunks {
        stripper.write_all(chunk).unwrap();
    }
    String::from_utf8(stripper.sink.inner).unwrap()
}

#[test]
fn strips_complete_sequence_in_one_write() {
    assert_eq!(strip_chunks(&[b"\x1b[32mgreen\x1b[0m"]), "green");
}

#[test]
fn strips_sequence_split_across_writes() {
    assert_eq!(
        strip_chunks(&[b"\x1b[", b"38;", b"5;11", b"m", b"git_diff", b"\x1b[0m"]),
        "git_diff"
    );
}

#[test]
fn forwards_text_around_an_escape_without_buffering_to_newline() {
    // `foo ` and `bar` are emitted as parsed; only the escape bytes are held.
    assert_eq!(strip_chunks(&[b"foo ", b"\x1b[31m", b"bar"]), "foo bar");
}

#[test]
fn keeps_line_feeds_drops_other_c0_controls() {
    assert_eq!(strip_chunks(&[b"a\tb\r\nc"]), "ab\nc");
}
