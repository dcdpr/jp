//! Entry point for the `jp-release-github` command plugin binary.

use std::process::ExitCode;

use jp_release_github::{help_text, run};

fn main() -> ExitCode {
    jp_plugin::client::main("release github", help_text, run)
}
