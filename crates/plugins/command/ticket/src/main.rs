//! The `jp-ticket` executable: a thin shell around [`jp_ticket`].
//!
//! Everything the plugin does lives in the library, so it can be tested without
//! spawning a process.

use std::io::{self, BufReader, IsTerminal as _, Write};

use jp_ticket::{help_text, run};

fn main() {
    // A human running the binary directly gets the help text rather than a
    // hung read on a protocol that is never going to speak.
    if io::stdin().is_terminal() {
        let mut err = io::stderr().lock();
        drop(writeln!(err, "{}", help_text()));
        drop(writeln!(err));
        drop(writeln!(
            err,
            "Note: this binary is a JP plugin. Run it via `jp ticket`."
        ));
        std::process::exit(0);
    }

    let code = match run(BufReader::new(io::stdin()), io::stdout()) {
        Ok(()) => 0,
        Err(error) => {
            let mut err = io::stderr().lock();
            drop(writeln!(err, "Fatal: {error}"));
            1
        }
    };

    std::process::exit(code);
}
