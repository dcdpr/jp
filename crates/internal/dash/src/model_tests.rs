use camino::Utf8PathBuf;
use chrono::TimeZone as _;

use super::*;

fn at(minute: u32) -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 9, 23, 10, minute, 0).unwrap()
}

fn ran(cwd: &str, last_event_at: Option<DateTime<Utc>>) -> JpRecent {
    JpRecent {
        cwd: Utf8PathBuf::from(cwd),
        last_event_at,
    }
}

fn worktree(path: &str, branch: &str) -> Worktree {
    Worktree {
        path: Utf8PathBuf::from(path),
        branch: Some(branch.to_owned()),
    }
}

fn terminal(id: &str, cwd: &str) -> Terminal {
    Terminal {
        id: id.to_owned(),
        cwd: Utf8PathBuf::from(cwd),
        name: "fish".to_owned(),
        tty: None,
    }
}

fn on_tty(id: &str, cwd: &str, tty: &str) -> Terminal {
    Terminal {
        tty: Some(tty.to_owned()),
        ..terminal(id, cwd)
    }
}

fn ids(row: &Row) -> Vec<&str> {
    row.terminals.iter().map(|t| t.id.as_str()).collect()
}

fn run(pid: u32, cwd: &str, conversation: &str) -> JpRun {
    JpRun {
        pid,
        cwd: Utf8PathBuf::from(cwd),
        conversation: conversation.to_owned(),
        waiting_since: None,
    }
}

fn session(pid: u32, cwd: &str) -> JpSession {
    JpSession {
        pid,
        shell: pid + 1,
        ghostty: true,
        tty: None,
        cwd: Utf8PathBuf::from(cwd),
        conversations: vec![],
        more: 0,
    }
}

fn pr(number: u64, head: &str) -> PullRequest {
    PullRequest {
        number,
        title: format!("PR {number}"),
        url: format!("https://github.com/dcdpr/jp/pull/{number}"),
        head: head.to_owned(),
        sha: format!("sha-{number}"),
        draft: false,
        checks: Checks::default(),
        threads: 0,
        answered: 0,
        labeled: false,
        base: None,
        stack: None,
        remote_branch: None,
        mergeable: None,
        queue: None,
    }
}

#[test]
fn test_rows_attach_every_source_to_its_worktree() {
    let worktrees = [
        worktree("/repo/queue", "gh-merge-queue"),
        worktree("/repo/main", "main"),
    ];
    let terminals = [terminal("T1", "/repo/queue")];
    let runs = [run(42, "/repo/queue", "Merge queue")];
    let prs = [pr(1200, "gh-merge-queue")];

    let rows = rows(&worktrees, &terminals, &runs, &[], &[], &prs);

    assert_eq!(rows, vec![
        Row {
            id: RowId::Worktree(Utf8PathBuf::from("/repo/main")),
            name: "main".to_owned(),
            branch: Some("main".to_owned()),
            terminals: vec![],
            runs: vec![],
            sessions: vec![],
            last_run: None,
            pr: None,
        },
        Row {
            id: RowId::Worktree(Utf8PathBuf::from("/repo/queue")),
            name: "queue".to_owned(),
            branch: Some("gh-merge-queue".to_owned()),
            terminals: vec![terminal("T1", "/repo/queue")],
            runs: vec![run(42, "/repo/queue", "Merge queue")],
            sessions: vec![],
            last_run: None,
            pr: Some(pr(1200, "gh-merge-queue")),
        },
    ]);
}

#[test]
fn test_rows_count_a_terminal_in_a_subdirectory() {
    let worktrees = [worktree("/repo/queue", "q")];
    let terminals = [terminal("T1", "/repo/queue/crates/jp_cli")];

    let rows = rows(&worktrees, &terminals, &[], &[], &[], &[]);

    assert_eq!(ids(&rows[0]), ["T1"]);
}

#[test]
fn test_rows_give_a_path_to_the_deepest_worktree() {
    // A worktree nested inside another directory that is itself a worktree
    // must win over its parent.
    let worktrees = [worktree("/repo", "main"), worktree("/repo/nested", "n")];
    let terminals = [terminal("T1", "/repo/nested/src")];

    let rows = rows(&worktrees, &terminals, &[], &[], &[], &[]);

    assert_eq!(rows[0].name, "nested");
    assert_eq!(ids(&rows[0]), ["T1"]);
    assert_eq!(rows[1].name, "repo");
    assert_eq!(ids(&rows[1]), Vec::<&str>::new());
}

#[test]
fn test_rows_do_not_match_a_sibling_sharing_a_name_prefix() {
    // `/repo/fix` is a string prefix of `/repo/fix-2`, but not a path prefix.
    let worktrees = [worktree("/repo/fix", "fix")];
    let terminals = [terminal("T1", "/repo/fix-2")];

    let rows = rows(&worktrees, &terminals, &[], &[], &[], &[]);

    assert_eq!(ids(&rows[0]), Vec::<&str>::new());
}

#[test]
fn test_rows_drop_terminals_and_runs_no_worktree_owns() {
    let worktrees = [worktree("/repo/queue", "q")];
    let terminals = [terminal("T1", "/elsewhere")];
    let runs = [run(1, "/elsewhere", "x")];

    let rows = rows(&worktrees, &terminals, &runs, &[], &[], &[]);

    assert_eq!(rows, vec![Row {
        id: RowId::Worktree(Utf8PathBuf::from("/repo/queue")),
        name: "queue".to_owned(),
        branch: Some("q".to_owned()),
        terminals: vec![],
        runs: vec![],
        sessions: vec![],
        last_run: None,
        pr: None,
    }]);
}

#[test]
fn test_rows_list_pull_requests_without_a_worktree_last() {
    let worktrees = [worktree("/repo/zed", "zed"), worktree("/repo/alpha", "a")];
    let prs = [pr(9, "nine"), pr(1200, "zed"), pr(3, "three")];

    let rows = rows(&worktrees, &[], &[], &[], &[], &prs);

    assert_eq!(rows.iter().map(|row| row.id.clone()).collect::<Vec<_>>(), [
        RowId::Worktree(Utf8PathBuf::from("/repo/alpha")),
        RowId::Worktree(Utf8PathBuf::from("/repo/zed")),
        RowId::PullRequest(3),
        RowId::PullRequest(9),
    ]);
    assert_eq!(rows[2], Row {
        id: RowId::PullRequest(3),
        name: String::new(),
        branch: Some("three".to_owned()),
        terminals: vec![],
        runs: vec![],
        sessions: vec![],
        last_run: None,
        pr: Some(pr(3, "three")),
    });
    assert!(rows[2].is_active());
    assert!(!rows[2].is_worktree());
}

#[test]
fn test_rows_leave_a_detached_worktree_without_a_pull_request() {
    let worktrees = [Worktree {
        path: Utf8PathBuf::from("/repo/detached"),
        branch: None,
    }];
    let prs = [pr(7, "detached")];

    let rows = rows(&worktrees, &[], &[], &[], &[], &prs);

    assert_eq!(rows[0].pr, None);
    assert_eq!(rows[1].id, RowId::PullRequest(7));
}

#[test]
fn test_rows_attach_sessions_by_shell_directory() {
    let worktrees = [worktree("/repo/queue", "q"), worktree("/repo/main", "main")];
    let sessions = [
        session(1, "/repo/queue/crates"),
        session(2, "/repo/main"),
        session(3, "/elsewhere"),
    ];

    let rows = rows(&worktrees, &[], &[], &sessions, &[], &[]);

    assert_eq!(rows[0].name, "main");
    assert_eq!(rows[0].sessions, [session(2, "/repo/main")]);
    assert_eq!(rows[1].sessions, [session(1, "/repo/queue/crates")]);
}

#[test]
fn test_rows_keep_the_latest_run_in_a_worktree() {
    // Multiple subdirectories in `/repo/a` take the latest timestamp; paths
    // outside all worktrees are ignored.
    let worktrees = [worktree("/repo/a", "a"), worktree("/repo/b", "b")];
    let recent = [
        ran("/repo/a", Some(at(5))),
        ran("/repo/a/crates", Some(at(9))),
        ran("/elsewhere", Some(at(30))),
        ran("/repo/b", None),
    ];

    let rows = rows(&worktrees, &[], &[], &[], &recent, &[]);

    assert_eq!(rows[0].last_run, Some(at(9)));
    assert_eq!(rows[1].last_run, None);
}

#[test]
fn test_last_activity_is_the_latest_of_the_sessions_and_the_last_run() {
    let worktrees = [worktree("/repo/a", "a")];
    let conversation = |minute| SessionConversation {
        id: format!("jp-c{minute}"),
        title: None,
        active: false,
        pinned: false,
        events: 1,
        last_event_at: Some(at(minute)),
        status: Status::Idle,
    };
    let sessions = [JpSession {
        conversations: vec![conversation(3), conversation(7)],
        ..session(10, "/repo/a")
    }];

    let listed = rows(&worktrees, &[], &[], &sessions, &[], &[]);
    assert_eq!(listed[0].last_activity(), Some(at(7)));

    // A recent run with a newer timestamp than any session conversation wins.
    let both = rows(
        &worktrees,
        &[],
        &[],
        &sessions,
        &[ran("/repo/a", Some(at(8)))],
        &[],
    );
    assert_eq!(both[0].last_activity(), Some(at(8)));

    let neither = rows(&worktrees, &[], &[], &[], &[], &[]);
    assert_eq!(neither[0].last_activity(), None);
}

#[test]
fn test_place_puts_each_session_in_the_tab_in_its_directory() {
    let worktrees = [worktree("/repo/a", "a")];
    let terminals = [
        terminal("T1", "/repo/a"),
        terminal("T2", "/repo/a/src"),
        terminal("T3", "/repo/a"),
    ];
    // Two sessions in `/repo/a` are distributed across its two tabs in PID
    // order; the session in `/repo/a/docs` has no matching tab.
    let sessions = [
        session(30, "/repo/a"),
        session(20, "/repo/a/src"),
        session(10, "/repo/a"),
        session(40, "/repo/a/docs"),
    ];
    let rows = rows(&worktrees, &terminals, &[], &sessions, &[], &[]);

    let placed = place(&rows[0]);

    let pids = |sessions: &Vec<&JpSession>| -> Vec<u32> {
        sessions.iter().map(|session| session.pid).collect()
    };
    assert_eq!(placed.tabs.iter().map(pids).collect::<Vec<_>>(), [
        vec![10],
        vec![20],
        vec![30]
    ]);
    assert_eq!(pids(&placed.other), [40]);
}

#[test]
fn test_place_puts_a_shell_outside_ghostty_in_no_tab() {
    let worktrees = [worktree("/repo/a", "a")];
    let terminals = [terminal("T1", "/repo/a")];
    let sessions = [JpSession {
        ghostty: false,
        ..session(10, "/repo/a")
    }];
    let rows = rows(&worktrees, &terminals, &[], &sessions, &[], &[]);

    let placed = place(&rows[0]);

    assert_eq!(placed.tabs, [Vec::<&JpSession>::new()]);
    assert_eq!(placed.other.len(), 1);
}

#[test]
fn test_owns_only_the_sole_tab_of_the_sole_ghostty_shell_in_a_directory() {
    let worktrees = [worktree("/repo/a", "a")];
    let owns = |terminals: &[Terminal], sessions: &[JpSession]| {
        let rows = rows(&worktrees, terminals, &[], sessions, &[], &[]);
        rows[0].owns(&rows[0].terminals[0], &rows[0].sessions[0])
    };
    let outside = JpSession {
        ghostty: false,
        ..session(20, "/repo/a")
    };

    assert!(owns(&[terminal("T1", "/repo/a")], &[session(
        10, "/repo/a"
    )]));
    // A shell outside Ghostty in the same directory does not share the tab.
    assert!(owns(&[terminal("T1", "/repo/a")], &[
        session(10, "/repo/a"),
        outside.clone()
    ]));

    // Multiple tabs or multiple Ghostty shells in the same directory are
    // ambiguous.
    assert!(!owns(
        &[terminal("T1", "/repo/a"), terminal("T2", "/repo/a")],
        &[session(10, "/repo/a")]
    ));
    assert!(!owns(&[terminal("T1", "/repo/a")], &[
        session(10, "/repo/a"),
        session(20, "/repo/a")
    ]));
    // Sessions in a different directory or outside Ghostty do not match.
    assert!(!owns(&[terminal("T1", "/repo/a")], &[session(
        10,
        "/repo/a/src"
    )]));
    assert!(!owns(&[terminal("T1", "/repo/a")], &[outside]));
}

#[test]
fn test_place_matches_by_tty_whatever_the_directories() {
    // When TTYs are reported, sessions match their exact tab even when sharing
    // a directory.
    let worktrees = [worktree("/repo/a", "a")];
    let terminals = [
        on_tty("T1", "/repo/a", "ttys001"),
        on_tty("T2", "/repo/a", "ttys002"),
    ];
    let in_tty = |pid: u32, tty: &str| JpSession {
        tty: Some(tty.to_owned()),
        ..session(pid, "/repo/a")
    };
    // Place the lower-PID session in the second tab to verify TTY matching
    // overrides PID order.
    let sessions = [in_tty(10, "ttys002"), in_tty(20, "ttys001")];
    let rows = rows(&worktrees, &terminals, &[], &sessions, &[], &[]);
    let row = &rows[0];

    let placed = place(row);
    let pids = |sessions: &Vec<&JpSession>| -> Vec<u32> {
        sessions.iter().map(|session| session.pid).collect()
    };
    assert_eq!(placed.tabs.iter().map(pids).collect::<Vec<_>>(), [
        vec![20],
        vec![10]
    ]);

    assert!(row.owns(&row.terminals[1], &row.sessions[0]));
    assert!(!row.owns(&row.terminals[0], &row.sessions[0]));
}

#[test]
fn test_place_puts_a_session_no_tty_matches_in_no_tab() {
    // When a tab reports a TTY that does not match the session's TTY,
    // directory fallback is not used.
    let worktrees = [worktree("/repo/a", "a")];
    let terminals = [on_tty("T1", "/repo/a", "ttys001")];
    let sessions = [JpSession {
        tty: Some("ttys009".to_owned()),
        ..session(10, "/repo/a")
    }];
    let rows = rows(&worktrees, &terminals, &[], &sessions, &[], &[]);

    let placed = place(&rows[0]);

    assert_eq!(placed.tabs, [Vec::<&JpSession>::new()]);
    assert_eq!(placed.other.len(), 1);
    assert!(!rows[0].owns(&rows[0].terminals[0], &rows[0].sessions[0]));
}

#[test]
fn test_row_is_active_with_any_one_source() {
    let idle = rows(&[worktree("/repo/a", "a")], &[], &[], &[], &[], &[]);
    let with_terminal = rows(
        &[worktree("/repo/a", "a")],
        &[terminal("T", "/repo/a")],
        &[],
        &[],
        &[],
        &[],
    );
    let with_run = rows(
        &[worktree("/repo/a", "a")],
        &[],
        &[run(1, "/repo/a", "c")],
        &[],
        &[],
        &[],
    );
    let with_pr = rows(&[worktree("/repo/a", "a")], &[], &[], &[], &[], &[pr(
        1, "a",
    )]);

    assert!(!idle[0].is_active());
    assert!(with_terminal[0].is_active());
    assert!(with_run[0].is_active());
    assert!(with_pr[0].is_active());
}
