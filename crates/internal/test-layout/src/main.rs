//! Report test code outside test files across the workspace.
//!
//! Prints one line per breach, `file:line: message`, to stdout.
//! Exits 0 when the workspace follows the layout, 1 when it breaches it, and 2
//! when the check cannot run.

use std::{
    io::{self, Write as _},
    process::ExitCode,
};

fn main() -> ExitCode {
    let violations = match test_layout::check_workspace() {
        Ok(violations) => violations,
        Err(error) => {
            drop(writeln!(io::stderr(), "test-layout: {error}"));
            return ExitCode::from(2);
        }
    };

    if violations.is_empty() {
        return ExitCode::SUCCESS;
    }

    let mut out = io::stdout().lock();
    for violation in &violations {
        drop(writeln!(out, "{violation}"));
    }
    drop(writeln!(
        io::stderr(),
        "test-layout: {} breaches of the test layout",
        violations.len()
    ));

    ExitCode::from(1)
}
