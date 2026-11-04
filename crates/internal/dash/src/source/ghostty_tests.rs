use camino::Utf8PathBuf;

use super::*;

#[test]
fn test_parse_keeps_terminals_with_a_working_directory() {
    let json = r#"[
        {"id": "F5D4465E-CCDE-4137-AFBD-006C13274B63", "cwd": "/repo/git-issue-tracking", "name": "~/P/P/j/git-issue-tracking", "tty": "/dev/ttys001"},
        {"id": "4FBB19EF-9FD5-4512-8A9B-A10D8BC8FDEF", "cwd": ""},
        {"id": "C5F624EA-4D21-4F25-952E-2B151185F88B", "cwd": null}
    ]"#;

    assert_eq!(parse(json).unwrap(), vec![Terminal {
        id: "F5D4465E-CCDE-4137-AFBD-006C13274B63".to_owned(),
        cwd: Utf8PathBuf::from("/repo/git-issue-tracking"),
        name: "~/P/P/j/git-issue-tracking".to_owned(),
        tty: Some("ttys001".to_owned()),
    }]);
}

#[test]
fn test_parse_without_a_tty_from_an_older_build() {
    let json = r#"[
        {"id": "T1", "cwd": "/repo/a", "name": "fish", "tty": ""},
        {"id": "T2", "cwd": "/repo/a", "name": "fish"}
    ]"#;

    let ttys: Vec<Option<String>> = parse(json).unwrap().into_iter().map(|t| t.tty).collect();

    assert_eq!(ttys, [None, None]);
}

#[test]
fn test_tty_name_matches_what_ps_prints() {
    assert_eq!(tty_name("/dev/ttys016").as_deref(), Some("ttys016"));
    assert_eq!(tty_name("ttys016").as_deref(), Some("ttys016"));
    // `ps` prints `??` for a process without a controlling terminal.
    assert_eq!(tty_name("??"), None);
    assert_eq!(tty_name(""), None);
}

#[test]
fn test_parse_ghostty_not_running() {
    assert_eq!(parse("[]\n").unwrap(), vec![]);
}

#[test]
fn test_parse_rejects_output_that_is_not_the_list() {
    let error = parse("execution error: Ghostty got an error").unwrap_err();

    assert!(matches!(error, Error::Parse {
        program: "osascript",
        ..
    }));
}
