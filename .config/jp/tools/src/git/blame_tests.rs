use camino_tempfile::tempdir;

use super::*;
use crate::util::runner::MockProcessRunner;

const SHA_ALICE: &str = "abc1234567890abcdef1234567890abcdef12345";
const SHA_BOB: &str = "def5678901234567890abcdef1234567890abcde";
const SHA_PREV_ALICE: &str = "0000aaaa0000aaaa0000aaaa0000aaaa0000aaaa";

/// Sample porcelain output covering three lines: two from Alice (contiguous)
/// followed by one from Bob. Bob's commit is the first to introduce its line
/// (no `previous` field).
fn sample_porcelain() -> String {
    format!(
        "\
{SHA_ALICE} 10 42 1
author Alice
author-mail <alice@example.com>
author-time 1717228800
author-tz +0000
committer Alice
committer-mail <alice@example.com>
committer-time 1717228800
committer-tz +0000
summary feat: rework streaming loop
previous {SHA_PREV_ALICE} src/foo.rs
filename src/foo.rs
\tlet stream = self.build_stream();
{SHA_ALICE} 11 43 1
\tpin_mut!(stream);
{SHA_BOB} 5 44 1
author Bob
author-mail <bob@example.com>
author-time 1716000000
author-tz +0200
committer Bob
committer-mail <bob@example.com>
committer-time 1716000000
committer-tz +0200
summary fix: handle EOF
filename src/foo.rs
\twhile let Some(event) = stream.next().await {{
"
    )
}

#[test]
fn parses_porcelain_into_commits_and_lines() {
    let blame = parse_porcelain(&sample_porcelain(), "src/foo.rs", None, 42, 44).unwrap();

    assert_eq!(blame.file, "src/foo.rs");
    assert_eq!(blame.start_line, 42);
    assert_eq!(blame.end_line, 44);
    assert_eq!(blame.revision, None);

    assert_eq!(blame.lines.len(), 3);
    assert_eq!(blame.lines[0], BlameLine {
        sha: SHA_ALICE.into(),
        final_line: 42,
        content: "let stream = self.build_stream();".into(),
    });
    assert_eq!(blame.lines[1].final_line, 43);
    assert_eq!(blame.lines[2].sha, SHA_BOB);

    let alice = blame.commits.get(SHA_ALICE).unwrap();
    assert_eq!(alice.author, "Alice");
    assert_eq!(alice.summary, "feat: rework streaming loop");
    assert_eq!(alice.previous.as_deref(), Some(SHA_PREV_ALICE));
    assert_eq!(alice.date, "2024-06-01T08:00:00+00:00");

    let bob = blame.commits.get(SHA_BOB).unwrap();
    assert_eq!(bob.author, "Bob");
    assert_eq!(bob.summary, "fix: handle EOF");
    assert!(
        bob.previous.is_none(),
        "bob is initial commit for this line"
    );
    // 1716000000 epoch (2024-05-18T02:40:00Z) at +0200 → 04:40:00+02:00.
    assert_eq!(bob.date, "2024-05-18T04:40:00+02:00");
}

#[test]
fn parses_uncommitted_zero_sha() {
    let porcelain = "\
0000000000000000000000000000000000000000 1 1 1
author Not Committed Yet
author-mail <not.committed.yet>
author-time 1717228800
author-tz +0000
committer Not Committed Yet
committer-mail <not.committed.yet>
committer-time 1717228800
committer-tz +0000
summary Version of foo.rs from foo.rs
filename foo.rs
\tlocal edit
";
    let blame = parse_porcelain(porcelain, "foo.rs", None, 1, 1).unwrap();
    assert_eq!(blame.lines.len(), 1);
    let zero_sha = "0".repeat(40);
    let meta = blame.commits.get(&zero_sha).unwrap();
    assert_eq!(meta.author, "Not Committed Yet");
    assert!(meta.previous.is_none());
}

#[test]
fn group_lines_collapses_contiguous_same_sha() {
    let blame = parse_porcelain(&sample_porcelain(), "src/foo.rs", None, 42, 44).unwrap();
    let groups = group_lines(&blame.lines);

    assert_eq!(groups.len(), 2);
    assert_eq!(groups[0].sha, SHA_ALICE);
    assert_eq!(groups[0].lines.len(), 2);
    assert_eq!(groups[1].sha, SHA_BOB);
    assert_eq!(groups[1].lines.len(), 1);
}

#[test]
fn group_lines_splits_on_line_gap() {
    let lines = vec![
        BlameLine {
            sha: SHA_ALICE.into(),
            final_line: 10,
            content: "a".into(),
        },
        BlameLine {
            sha: SHA_ALICE.into(),
            final_line: 12,
            content: "b".into(),
        },
    ];

    let groups = group_lines(&lines);
    assert_eq!(groups.len(), 2, "non-contiguous lines should split");
}

#[test]
fn format_blame_includes_metadata_and_lines() {
    let blame = parse_porcelain(&sample_porcelain(), "src/foo.rs", None, 42, 44).unwrap();
    let output = format_blame(&blame).unwrap();

    assert!(output.starts_with("<git_blame>\n"));
    assert!(output.ends_with("</git_blame>"));
    assert!(output.contains("  <file>src/foo.rs</file>"));
    assert!(output.contains("  <revision>working tree</revision>"));
    assert!(output.contains("  <range>42-44</range>"));

    assert!(output.contains(&format!("    hash: {SHA_ALICE}")));
    assert!(output.contains("    short_hash: abc1234"));
    assert!(output.contains(&format!("    previous: {SHA_PREV_ALICE}")));
    assert!(output.contains("    author: Alice"));
    assert!(output.contains("    summary: feat: rework streaming loop"));
    assert!(output.contains("      42: let stream = self.build_stream();"));
    assert!(output.contains("      43: pin_mut!(stream);"));

    // Bob's hunk has no `previous` line — verify the field is omitted, not
    // rendered as an empty value.
    let bob_hunk_pos = output.find(&format!("hash: {SHA_BOB}")).unwrap();
    let after_bob = &output[bob_hunk_pos..];
    assert!(
        !after_bob.starts_with(&format!("hash: {SHA_BOB}\n    previous:")),
        "missing `previous` field should be omitted entirely"
    );
}

#[test]
fn format_uses_revision_when_set() {
    let mut blame =
        parse_porcelain(&sample_porcelain(), "src/foo.rs", Some("HEAD~3"), 42, 44).unwrap();
    blame.revision = Some("HEAD~3".into());
    let output = format_blame(&blame).unwrap();
    assert!(output.contains("  <revision>HEAD~3</revision>"));
}

#[test]
fn rejects_zero_start_line() {
    let dir = tempdir().unwrap();
    let runner = MockProcessRunner::never_called();
    let outcome =
        git_blame_impl(dir.path(), "src/foo.rs", 0, 10, None, false, &runner, &[]).unwrap();
    assert!(outcome.into_content().is_none());
}

#[test]
fn rejects_inverted_range() {
    let dir = tempdir().unwrap();
    let runner = MockProcessRunner::never_called();
    let outcome =
        git_blame_impl(dir.path(), "src/foo.rs", 20, 10, None, false, &runner, &[]).unwrap();
    assert!(outcome.into_content().is_none());
}

#[test]
fn rejects_oversized_range() {
    let dir = tempdir().unwrap();
    let runner = MockProcessRunner::never_called();
    let outcome = git_blame_impl(
        dir.path(),
        "src/foo.rs",
        1,
        MAX_RANGE + 1,
        None,
        false,
        &runner,
        &[],
    )
    .unwrap();
    assert!(outcome.into_content().is_none());
}

#[test]
fn basic_blame_call() {
    let dir = tempdir().unwrap();
    let runner = MockProcessRunner::builder()
        .expect("git")
        .args(&["blame", "--porcelain", "-L42,44", "--", "src/foo.rs"])
        .returns_success(sample_porcelain());

    let content = git_blame_impl(dir.path(), "src/foo.rs", 42, 44, None, false, &runner, &[])
        .unwrap()
        .into_content()
        .unwrap();

    assert!(content.contains("<git_blame>"));
    assert!(content.contains(&format!("hash: {SHA_ALICE}")));
    assert!(content.contains(&format!("hash: {SHA_BOB}")));
}

#[test]
fn blame_with_revision_and_whitespace_flag() {
    let dir = tempdir().unwrap();
    let runner = MockProcessRunner::builder()
        .expect("git")
        .args(&[
            "blame",
            "--porcelain",
            "-L42,44",
            "-w",
            "HEAD~3",
            "--",
            "src/foo.rs",
        ])
        .returns_success(sample_porcelain());

    let content = git_blame_impl(
        dir.path(),
        "src/foo.rs",
        42,
        44,
        Some("HEAD~3"),
        true,
        &runner,
        &[],
    )
    .unwrap()
    .into_content()
    .unwrap();

    assert!(content.contains("<revision>HEAD~3</revision>"));
}

#[test]
fn blame_git_error() {
    let dir = tempdir().unwrap();
    let runner = MockProcessRunner::error("fatal: no such path 'missing.rs' in HEAD");
    let outcome =
        git_blame_impl(dir.path(), "missing.rs", 1, 10, None, false, &runner, &[]).unwrap();
    assert!(outcome.into_content().is_none(), "expected error outcome");
}

#[test]
fn empty_blame_output() {
    let dir = tempdir().unwrap();
    let runner = MockProcessRunner::success("");
    let content = git_blame_impl(dir.path(), "src/foo.rs", 1, 10, None, false, &runner, &[])
        .unwrap()
        .into_content()
        .unwrap();
    assert_eq!(
        content,
        "No blame information returned for the specified range."
    );
}

#[test]
fn is_sha_validates_length_and_hex() {
    assert!(is_sha(SHA_ALICE));
    assert!(is_sha(&"0".repeat(40)));
    assert!(!is_sha("short"));
    assert!(!is_sha(&"g".repeat(40)));
    assert!(!is_sha(&"a".repeat(41)));
}

#[test]
fn parses_positive_and_negative_tz_offsets() {
    let plus = format_author_date(1_717_228_800, "+0200");
    assert_eq!(plus, "2024-06-01T10:00:00+02:00");
    let minus = format_author_date(1_717_228_800, "-0500");
    assert_eq!(minus, "2024-06-01T03:00:00-05:00");
}

#[test]
fn malformed_tz_falls_back_to_raw() {
    let out = format_author_date(1_717_228_800, "garbage");
    assert_eq!(out, "1717228800 garbage");
}
