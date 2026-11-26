//! The `jp-serve-chat` executable: a thin shell around [`jp_serve_chat`].

use std::{
    io::{self, BufReader, IsTerminal as _},
    process,
};

use jp_plugin::stdio;
use jp_serve_chat::{HELP_TEXT, run};

#[expect(
    clippy::bufreader_stdin,
    reason = "`run` moves stdin to another thread, and a `StdinLock` is not `Send`"
)]
fn main() {
    let code = stdio::exit_code(
        "serve chat",
        io::stdin().is_terminal(),
        || HELP_TEXT.to_owned(),
        || run(BufReader::new(io::stdin()), io::stdout()),
        &mut io::stderr(),
    );

    process::exit(code);
}
