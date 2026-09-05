//! The spawn path, exercised end to end against a real pty.
//!
//! On Windows this is the only way in — ConPTY is reachable through a child
//! process and nothing else — so these cases are what covers that platform.
//! They match on content rather than on exact rows: what ConPTY hands back is a
//! repaint of its own screen rather than the bytes the child wrote, so where a
//! line lands is the console's business.

use std::time::Duration;

use jp_pty::{Child, CommandBuilder, Size, Terminal};

/// The probe binary, built by cargo alongside this test.
const PROBE: &str = env!("CARGO_BIN_EXE_pty_probe");

/// How long a case waits on a child process.
///
/// Longer than the harness default: this waits on process startup under
/// whatever else CI is running at the time.
const TIMEOUT: Duration = Duration::from_secs(20);

/// A terminal of `size` with the probe running in it.
///
/// The child is returned rather than dropped, because dropping it kills it.
fn spawn_probe(size: Size) -> (Terminal, Child) {
    let terminal = Terminal::pty(size).expect("a pty").with_timeout(TIMEOUT);
    let child = terminal
        .spawn(CommandBuilder::new(PROBE))
        .expect("the probe to start");

    (terminal, child)
}

#[test]
fn a_child_is_given_the_size_the_terminal_was_opened_with() {
    let (terminal, mut child) = spawn_probe(Size::new(24, 80));

    // The probe measures its own tty, so this is the kernel's answer rather
    // than the harness repeating back what it was told.
    terminal
        .wait_for("the probe to report 24x80", |screen| {
            screen.contains("size 24x80")
        })
        .expect("the probe to measure the size it was spawned with");

    terminal.send("quit\n").expect("the keystrokes to arrive");
    assert!(child.wait().expect("the probe to exit"));
}

#[test]
fn keystrokes_reach_the_child() {
    let (terminal, _child) = spawn_probe(Size::new(24, 80));
    terminal
        .wait_for("the probe to start", |screen| screen.contains("size "))
        .expect("the probe to report a size");

    terminal.send("hello\n").expect("the keystrokes to arrive");

    terminal
        .wait_for("the probe to echo the line", |screen| {
            screen.contains("you typed: hello")
        })
        .expect("the probe to read what was typed");
}

#[test]
fn a_resize_reaches_a_running_child() {
    // The case with no in-process equivalent: the new size has to travel
    // through the kernel to a process that is already running.
    let (terminal, _child) = spawn_probe(Size::new(24, 80));
    terminal
        .wait_for("the probe to start", |screen| screen.contains("size 24x80"))
        .expect("the probe to report a size");

    terminal
        .resize(Size::new(10, 40))
        .expect("the resize to apply");
    terminal.send("again\n").expect("the keystrokes to arrive");

    terminal
        .wait_for("the probe to report the new size", |screen| {
            screen.contains("size 10x40")
        })
        .expect("the resize to reach the probe");
}
