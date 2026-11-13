//! Entry point for the `jp-release` command plugin binary.

use std::process::ExitCode;

use jp_release::{help_text, run};

fn main() -> ExitCode {
    jp_plugin::client::main("release", help_text, run)
}
