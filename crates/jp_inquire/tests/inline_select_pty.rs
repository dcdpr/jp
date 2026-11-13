//! PTY integration tests for [`jp_inquire::InlineSelect`].
#![cfg(unix)]

use std::time::Duration;

use jp_pty::{Child, CommandBuilder, Screen, Size, Terminal};

const PROBE: &str = env!("CARGO_BIN_EXE_prompt_probe");
const TIMEOUT: Duration = Duration::from_secs(20);

fn open(args: &[&str]) -> (Terminal, Child) {
    let terminal = Terminal::pty(Size::new(12, 80))
        .expect("a pty")
        .with_timeout(TIMEOUT);
    let mut command = CommandBuilder::new(PROBE);
    command.args(args);
    let probe = terminal.spawn(command).expect("the probe to start");

    terminal
        .wait_for("the prompt", |screen| screen.contains("Remove it?"))
        .expect("the prompt to be drawn");

    (terminal, probe)
}

fn outcome(terminal: &Terminal, mut probe: Child) -> Screen {
    let screen = terminal
        .wait_for("the outcome", |screen| screen.contains("outcome:"))
        .expect("the prompt to return");

    // `"outcome:"` appears before `main` returns. Wait for the process to exit
    // so `Child::drop` does not kill it while `atexit` flushes coverage data.
    assert_eq!(
        probe.wait_within(TIMEOUT).expect("the exit check"),
        Some(true),
        "the probe exits once its prompt has returned"
    );

    screen
}

#[test]
fn a_chosen_option_replaces_the_prompt_with_its_answer() {
    let (terminal, probe) = open(&["select"]);

    terminal.send("y").expect("the key to be typed");
    let screen = outcome(&terminal, probe);

    assert_eq!(
        screen.used(),
        ["above", "> Remove it? [y,n,?] y", "outcome: Ok('y')"],
        "{screen}"
    );
}

#[test]
fn enter_chooses_the_default_and_clears_the_help_message() {
    let (terminal, probe) = open(&["select-default"]);

    let screen = terminal
        .wait_for("the help message", |screen| {
            screen.contains("cannot be undone")
        })
        .expect("the help message to be drawn");
    assert_eq!(
        screen.used(),
        ["above", "? Remove it? [y,n,?] (n)", "[cannot be undone]"],
        "{screen}"
    );
    // Waiting at the end of the prompt's own row, not below the help message.
    assert_eq!(screen.cursor().0, 1, "{screen}");

    terminal.send("\r").expect("Enter to be typed");
    let screen = outcome(&terminal, probe);

    assert_eq!(
        screen.used(),
        ["above", "> Remove it? [y,n,?] n", "outcome: Ok('n')"],
        "{screen}"
    );
}

#[test]
fn escape_cancels_the_prompt() {
    let (terminal, probe) = open(&["select"]);

    terminal.send("\x1b").expect("Escape to be typed");
    let screen = outcome(&terminal, probe);

    assert_eq!(
        screen.used(),
        [
            "above",
            "? Remove it? [y,n,?] <canceled>",
            "outcome: Err(OperationCanceled)"
        ],
        "{screen}"
    );
}

#[test]
fn ctrl_c_interrupts_the_prompt_and_leaves_it_on_screen() {
    let (terminal, probe) = open(&["select"]);

    terminal.send("\x03").expect("Ctrl-C to be typed");
    let screen = outcome(&terminal, probe);

    assert_eq!(
        screen.used(),
        [
            "above",
            "? Remove it? [y,n,?]",
            "outcome: Err(OperationInterrupted)"
        ],
        "{screen}"
    );
}

#[test]
fn a_question_mark_lists_the_options_and_asks_again() {
    let (terminal, probe) = open(&["select"]);

    terminal.send("?").expect("the key to be typed");
    let screen = terminal
        .wait_for("the second prompt", |screen| {
            screen.contains("print help") && screen.row(5).contains("Remove it?")
        })
        .expect("the prompt to be drawn again");
    assert_eq!(
        screen.used(),
        [
            "above",
            "> Remove it? [y,n,?] ?",
            "y - remove it",
            "n - keep it",
            "? - print help",
            "? Remove it? [y,n,?]"
        ],
        "{screen}"
    );

    terminal.send("n").expect("the key to be typed");
    let screen = outcome(&terminal, probe);

    assert_eq!(screen.row(5), "> Remove it? [y,n,?] n", "{screen}");
    assert_eq!(screen.row(6), "outcome: Ok('n')", "{screen}");
}

#[test]
fn a_key_that_is_no_option_waits_to_be_corrected() {
    let (terminal, probe) = open(&["select"]);

    terminal.send("x").expect("the key to be typed");
    let screen = terminal
        .wait_for("the typed key", |screen| screen.row(1).ends_with(" x"))
        .expect("the key to be drawn");
    assert_eq!(
        screen.used(),
        ["above", "? Remove it? [y,n,?] x"],
        "{screen}"
    );

    terminal.send("\r").expect("Enter to be typed");
    let screen = terminal
        .wait_for("the error", |screen| screen.contains("print help"))
        .expect("the error to be drawn");
    assert_eq!(
        screen.used(),
        [
            "above",
            "# Invalid option:",
            "y - remove it",
            "n - keep it",
            "? - print help",
            "? Remove it? [y,n,?] x"
        ],
        "{screen}"
    );

    // Backspace, then a key that names an option.
    terminal.send("\x7fy").expect("the keys to be typed");
    let screen = outcome(&terminal, probe);

    assert_eq!(
        screen.used(),
        ["above", "> Remove it? [y,n,?] y", "outcome: Ok('y')"],
        "{screen}"
    );
}
