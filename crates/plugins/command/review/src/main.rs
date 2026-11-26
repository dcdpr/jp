//! The `jp-review` executable: a thin shell around [`jp_review`].
//!
//! Everything the plugin does lives in the library, so it can be tested without
//! spawning a process.

use std::{
    io::{self, BufReader, IsTerminal as _},
    process,
};

use jp_plugin::stdio;
use jp_review::{help_text, run};

#[expect(
    clippy::bufreader_stdin,
    reason = "`run` moves stdin to another thread, and a `StdinLock` is not `Send`"
)]
fn main() {
    let code = stdio::exit_code(
        "review",
        io::stdin().is_terminal(),
        help_text,
        || run(BufReader::new(io::stdin()), io::stdout()),
        &mut io::stderr(),
    );

    process::exit(code);
}
