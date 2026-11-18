//! The `jp-gui` executable: a thin shell around [`jp_gui`].
//!
//! Everything the plugin does lives in the library, so it can be tested without
//! spawning a process.

use std::{
    io::{self, BufReader, IsTerminal as _},
    process,
};

use jp_gui::{HELP_TEXT, launch::SystemLauncher, run};
use jp_plugin::stdio;

fn main() {
    let code = stdio::exit_code(
        "gui",
        io::stdin().is_terminal(),
        || HELP_TEXT.to_owned(),
        || run(BufReader::new(io::stdin()), io::stdout(), &SystemLauncher),
        &mut io::stderr(),
    );

    process::exit(code);
}
