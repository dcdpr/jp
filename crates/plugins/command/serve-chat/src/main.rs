//! The `jp-serve-chat` executable: a thin shell around [`jp_serve_chat`].

use std::io::{self, BufReader, IsTerminal as _, Write};

use jp_serve_chat::{HELP_TEXT, run};

fn main() {
    // A terminal on stdin means someone ran the binary directly, not `jp`.
    if io::stdin().is_terminal() {
        let mut err = io::stderr().lock();
        drop(writeln!(err, "{HELP_TEXT}\n"));
        drop(writeln!(
            err,
            "Note: this binary is a JP plugin. Run it via `jp serve chat`."
        ));
        std::process::exit(0);
    }

    let code = match run(BufReader::new(io::stdin()), io::stdout()) {
        Ok(()) => 0,
        Err(error) => {
            drop(writeln!(io::stderr().lock(), "Fatal: {error}"));
            1
        }
    };

    std::process::exit(code);
}
