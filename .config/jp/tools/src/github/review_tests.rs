use super::*;

#[test]
fn extract_window_marks_single_line() {
    let content = "line1\nline2\nline3\nline4\nline5\nline6\nline7";
    let result = extract_window(content, 4, None);

    let marked: Vec<&str> = result
        .lines()
        .filter(|l| l.trim_start().starts_with('>'))
        .collect();

    assert_eq!(marked.len(), 1);
    assert!(marked[0].contains("line4"));
}

#[test]
fn extract_window_marks_multiline_range() {
    let content = (1..=20)
        .map(|n| format!("line{n}"))
        .collect::<Vec<_>>()
        .join("\n");
    let result = extract_window(&content, 10, Some(5));

    let marked: Vec<&str> = result
        .lines()
        .filter(|l| l.trim_start().starts_with('>'))
        .collect();

    // start_line=5, line=10 → 6 marked lines (5, 6, 7, 8, 9, 10).
    assert_eq!(marked.len(), 6);
    assert!(marked.iter().any(|l| l.contains("line5")));
    assert!(marked.iter().any(|l| l.contains("line10")));
    // line11 is in the trailing context window but should NOT be marked.
    assert!(
        result
            .lines()
            .filter(|l| l.contains("line11"))
            .any(|l| !l.trim_start().starts_with('>'))
    );
}

#[test]
fn extract_window_clamps_to_first_line() {
    let content = "line1\nline2\nline3";
    let result = extract_window(content, 1, None);
    assert!(
        result
            .lines()
            .any(|l| l.trim_start().starts_with('>') && l.contains("line1"))
    );
}

#[test]
fn extract_window_clamps_to_last_line() {
    let content = "line1\nline2\nline3";
    let result = extract_window(content, 3, None);
    assert!(
        result
            .lines()
            .any(|l| l.trim_start().starts_with('>') && l.contains("line3"))
    );
}

#[test]
fn extract_window_marks_two_line_range() {
    // Adjacent multi-line comment.
    let content = "a\nb\nc\nd\ne";
    let result = extract_window(content, 3, Some(2));

    let marked: Vec<&str> = result
        .lines()
        .filter(|l| l.trim_start().starts_with('>'))
        .collect();

    assert_eq!(marked.len(), 2);
    assert!(marked.iter().any(|l| l.contains('b')));
    assert!(marked.iter().any(|l| l.contains('c')));
}

#[test]
fn extract_window_handles_empty_file() {
    assert_eq!(extract_window("", 1, None), "(empty file)");
}

#[test]
fn extract_window_handles_start_line_past_end() {
    // Validation should keep us out of this case in practice, but the
    // function shouldn't panic if it happens.
    let content = "a\nb\nc";
    let result = extract_window(content, 100, Some(50));
    // Falls back to clamping: line_idx = 2 (last), start_idx = 2.
    let marked: Vec<&str> = result
        .lines()
        .filter(|l| l.trim_start().starts_with('>'))
        .collect();
    assert_eq!(marked.len(), 1);
    assert!(marked[0].contains('c'));
}
