//! The `jp-gui` executable: a thin shell around [`jp_gui`].
//!
//! Everything the plugin does lives in the library, so it can be tested without
//! spawning a process.

use std::io::{self, BufReader, IsTerminal as _, Write};

use jp_gui::{HELP_TEXT, launch::SystemLauncher, run};

fn main() {
    if io::stdin().is_terminal() {
        let mut err = io::stderr().lock();
        drop(writeln!(err, "{HELP_TEXT}"));
        drop(writeln!(err));
        drop(writeln!(
            err,
            "Note: this binary is a JP plugin. Run it via `jp gui`."
        ));
        std::process::exit(0);
    }

    let stdin = BufReader::new(io::stdin());
    let stdout = io::stdout();

    let code = match run(stdin, stdout, &SystemLauncher) {
        Ok(()) => 0,
        Err(e) => {
            let mut err = io::stderr().lock();
            drop(writeln!(err, "Fatal: {e}"));
            1
        }
    };

    std::process::exit(code);
}
