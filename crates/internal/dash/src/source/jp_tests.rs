use camino::Utf8PathBuf;

use super::*;

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
    // The shell's group is its terminal's foreground group: it sits at its
    // prompt.
    assert_eq!(parse_foreground(" 4101  4101\n"), Some(true));
    // `jp` runs in front of it.
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
    // A process lsof may not inspect reports its PID and command only; the
    // next process's cwd must not be attributed to it.
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
