use camino::Utf8PathBuf;

use super::*;

#[test]
fn test_parse_keeps_branches_and_detached_heads() {
    let porcelain = "\
worktree /repo/.bare
bare

worktree /repo/main
HEAD b9de1cfbe88066775d2aa6bdafa29b4d75f6d3b1
branch refs/heads/main

worktree /repo/review
HEAD 6d55a720113ffb7692141f5288883f5e0cec8b30
detached

worktree /repo/gone
HEAD 58f36525aa529aefab357438862ee792878609ae
branch refs/heads/gone
prunable gitdir file points to non-existent location

worktree /repo/locked
HEAD 2017653d43750065972b46171cef492575a62b72
branch refs/heads/feature/locked
locked
";

    assert_eq!(parse(porcelain), vec![
        Worktree {
            path: Utf8PathBuf::from("/repo/main"),
            branch: Some("main".to_owned()),
        },
        Worktree {
            path: Utf8PathBuf::from("/repo/review"),
            branch: None,
        },
        Worktree {
            path: Utf8PathBuf::from("/repo/locked"),
            branch: Some("feature/locked".to_owned()),
        },
    ]);
}

#[test]
fn test_parse_empty_output() {
    assert_eq!(parse(""), vec![]);
}
