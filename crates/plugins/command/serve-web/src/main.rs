//! The `jp-serve-web` executable: a thin shell around [`jp_serve_web`].
//!
//! Everything the plugin does lives in the library, so it can be tested without
//! spawning a process.

use std::{
    io::{self, BufReader, IsTerminal as _},
    process,
};

use jp_plugin::{log, stdio};
use jp_serve_web::{HELP_TEXT, run};

#[expect(
    clippy::bufreader_stdin,
    reason = "`run` moves stdin to another thread, and a `StdinLock` is not `Send`"
)]
fn main() {
    let log_handle = log::install("jp_serve_web");

    let code = stdio::exit_code(
        "serve web",
        io::stdin().is_terminal(),
        || HELP_TEXT.to_owned(),
        || run(BufReader::new(io::stdin()), io::stdout(), &log_handle),
        &mut io::stderr(),
    );

    process::exit(code);
}
