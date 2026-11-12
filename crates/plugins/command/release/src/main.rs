//! The `jp-release` executable: a thin shell around [`jp_release`].
//!
//! Everything the plugin does lives in the library, so it can be tested without
//! spawning a process.

use std::process::ExitCode;

use jp_release::{help_text, run};

fn main() -> ExitCode {
    jp_plugin::client::main("release", help_text, run)
}
