//! The `jp-rfd` executable: a thin shell around [`jp_rfd`].
//!
//! Everything the plugin does lives in the library, so it can be tested without
//! spawning a process.

use std::{
    io::{self, BufReader, IsTerminal as _},
    process,
};

use jp_plugin::stdio;
use jp_rfd::{help_text, run};

fn main() {
    let code = stdio::exit_code(
        "rfd",
        io::stdin().is_terminal(),
        help_text,
        || run(BufReader::new(io::stdin()), io::stdout()),
        &mut io::stderr(),
    );

    process::exit(code);
}
