use camino::Utf8PathBuf;
use chrono::TimeZone as _;

use super::*;

fn at(minute: u32) -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 9, 23, 10, minute, 0).unwrap()
}

fn conversation(minute: u32) -> ConversationId {
    ConversationId::try_from(at(minute)).unwrap()
}

#[test]
fn test_remember_keeps_the_conversation_last_run_in_each_directory() {
    let mut ran = BTreeMap::new();

    assert!(remember(&mut ran, [
        ("/repo/a", conversation(1)),
        ("/repo/b", conversation(2))
    ]));
    // Seeing the same run again is a no-op; a new run in `/repo/a` updates the
    // entry.
    assert!(!remember(&mut ran, [("/repo/a", conversation(1))]));
    assert!(remember(&mut ran, [("/repo/a", conversation(3))]));
    // An empty poll preserves existing entries.
    assert!(!remember(&mut ran, []));

    assert_eq!(
        ran,
        BTreeMap::from([
            ("/repo/a".to_owned(), conversation(3)),
            ("/repo/b".to_owned(), conversation(2)),
        ])
    );
}

#[test]
fn test_recent_reports_the_last_event_of_each_directorys_conversation() {
    let ran = BTreeMap::from([
        ("/repo/a".to_owned(), conversation(1)),
        ("/repo/b".to_owned(), conversation(2)),
    ]);
    // `/repo/b`'s conversation has no metadata: deleted, or not read yet.
    let metadata = HashMap::from([(conversation(1), Metadata {
        title: None,
        pinned: false,
        events: 3,
        last_event_at: Some(at(30)),
        read_at: Instant::now(),
        running: false,
    })]);

    assert_eq!(recent(&ran, &metadata), [
        JpRecent {
            cwd: Utf8PathBuf::from("/repo/a"),
            last_event_at: Some(at(30)),
        },
        JpRecent {
            cwd: Utf8PathBuf::from("/repo/b"),
            last_event_at: None,
        },
    ]);
}

#[test]
fn test_parse_lsof_reads_command_and_cwd_per_process() {
    let out = "\
p4242
cjp
fcwd
n/repo/gh-merge-queue
p5151
cbash
fcwd
n/repo/main
";

    assert_eq!(
        parse_lsof(out),
        HashMap::from([
            (4242, Process {
                command: "jp".to_owned(),
                cwd: Utf8PathBuf::from("/repo/gh-merge-queue"),
            }),
            (5151, Process {
                command: "bash".to_owned(),
                cwd: Utf8PathBuf::from("/repo/main"),
            }),
        ])
    );
}

fn entry(ppid: u32, command: &str) -> Entry {
    on_tty(ppid, "??", command)
}

fn on_tty(ppid: u32, tty: &str, command: &str) -> Entry {
    Entry {
        ppid,
        tty: tty.to_owned(),
        command: command.to_owned(),
    }
}

#[test]
fn test_parse_ps_reads_the_tty_and_keeps_spaces_in_the_command() {
    let out = "\
  812     1 ??       /Applications/Ghostty.app/Contents/MacOS/ghostty
 4100   812 ttys001  /usr/bin/login
 4101  4100 ttys001  -fish
 4200  4101 ttys001  /Users/jean/.cargo/bin/jp query --edit
";

    assert_eq!(
        parse_ps(out),
        HashMap::from([
            (
                812,
                on_tty(1, "??", "/Applications/Ghostty.app/Contents/MacOS/ghostty")
            ),
            (4100, on_tty(812, "ttys001", "/usr/bin/login")),
            (4101, on_tty(4100, "ttys001", "-fish")),
            (
                4200,
                on_tty(4101, "ttys001", "/Users/jean/.cargo/bin/jp query --edit")
            ),
        ])
    );
}

#[test]
fn test_shell_of_a_login_session_is_its_child() {
    let table = HashMap::from([
        (4100, entry(812, "/usr/bin/login")),
        (4101, entry(4100, "-fish")),
        (4200, entry(4101, "jp")),
    ]);

    assert_eq!(shell_of(&table, 4100), Some(4101));
}

#[test]
fn test_shell_of_a_session_the_shell_leads_is_the_leader() {
    let table = HashMap::from([(5100, entry(812, "-zsh")), (5200, entry(5100, "jp"))]);

    assert_eq!(shell_of(&table, 5100), Some(5100));
    // A leader that exited has no shell.
    assert_eq!(shell_of(&table, 6000), None);
}

#[test]
fn test_under_ghostty_walks_the_parents() {
    let table = HashMap::from([
        (1, entry(0, "/sbin/launchd")),
        (
            812,
            entry(1, "/Applications/Ghostty.app/Contents/MacOS/ghostty"),
        ),
        (4100, entry(812, "/usr/bin/login")),
        (4101, entry(4100, "-fish")),
        (
            5100,
            entry(1, "/Applications/Terminal.app/Contents/MacOS/Terminal"),
        ),
        (5101, entry(5100, "-zsh")),
    ]);

    assert!(under_ghostty(&table, 4101));
    assert!(under_ghostty(&table, 4100));
    assert!(!under_ghostty(&table, 5101));
    assert!(!under_ghostty(&table, 9999));
}

#[test]
fn test_parse_foreground() {
    // Shell process group equals terminal foreground process group (idle at
    // prompt).
    assert_eq!(parse_foreground(" 4101  4101\n"), Some(true));
    // Another process group (e.g. `jp`) owns the foreground.
    assert_eq!(parse_foreground(" 4101  4200\n"), Some(false));
    // The process is gone.
    assert_eq!(parse_foreground(""), None);
}

#[test]
fn test_parse_lsof_no_live_processes() {
    assert_eq!(parse_lsof(""), HashMap::new());
}

#[test]
fn test_parse_lsof_skips_a_process_without_a_cwd_line() {
    // When `lsof` cannot inspect a process's cwd, it emits only `p` and `c`;
    // the next process's `n` line must not be attached to the first process.
    let out = "\
p1
cjp
p2
cjp
fcwd
n/repo/b
";

    assert_eq!(
        parse_lsof(out),
        HashMap::from([(2, Process {
            command: "jp".to_owned(),
            cwd: Utf8PathBuf::from("/repo/b"),
        })])
    );
}
