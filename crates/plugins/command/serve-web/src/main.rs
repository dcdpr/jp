//! The `jp-serve-web` executable: a thin shell around [`jp_serve_web`].
//!
//! Everything the plugin does lives in the library, so it can be tested without
//! spawning a process.

use std::io::{self, BufReader, IsTerminal as _, Write};

use jp_serve_web::{HELP_TEXT, init_tracing, run};

fn main() {
    let log_handle = init_tracing();

    // If stdin is a TTY, the binary was invoked directly (not via the plugin
    // protocol). Print help and exit.
    if io::stdin().is_terminal() {
        let mut err = io::stderr().lock();
        drop(writeln!(err, "{HELP_TEXT}"));
        drop(writeln!(err));
        drop(writeln!(
            err,
            "Note: this binary is a JP plugin. Run it via `jp serve-web`."
        ));
        std::process::exit(0);
    }

    let stdin = BufReader::new(io::stdin());
    let stdout = io::stdout();

    let code = match run(stdin, stdout, &log_handle) {
        Ok(()) => 0,
        Err(e) => {
            let mut err = io::stderr().lock();
            drop(writeln!(err, "Fatal: {e}"));
            1
        }
    };

    std::process::exit(code);
}
