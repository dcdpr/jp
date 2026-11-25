//! The `jp-ticket` executable: a thin shell around [`jp_ticket`].
//!
//! Everything the plugin does lives in the library, so it can be tested without
//! spawning a process.

use std::{
    io::{self, IsTerminal as _},
    process,
};

use jp_plugin::stdio;
use jp_ticket::{help_text, run};

fn main() {
    let code = stdio::exit_code(
        "ticket",
        io::stdin().is_terminal(),
        help_text,
        || run(io::stdin().lock(), io::stdout()),
        &mut io::stderr(),
    );

    process::exit(code);
}
