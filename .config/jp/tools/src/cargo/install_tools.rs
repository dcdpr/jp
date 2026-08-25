use camino::Utf8Path;

use super::MAX_DIAGNOSTIC_BYTES;
use crate::util::{
    ToolResult, error,
    runner::{DuctProcessRunner, ProcessOutput, ProcessRunner},
    truncate,
};

pub(crate) async fn cargo_install_tools(root: &Utf8Path) -> ToolResult {
    cargo_install_tools_impl(root, &DuctProcessRunner)
}

fn cargo_install_tools_impl<R: ProcessRunner>(root: &Utf8Path, runner: &R) -> ToolResult {
    // Going through `just` keeps the install flags defined in one place, so this
    // and a hand-run `just install-tools` cannot drift apart.
    let ProcessOutput {
        stdout,
        stderr,
        status,
    } = runner.run("just", &["install-tools"], root)?;

    if !status.is_success() {
        let diagnostics = strip(&stderr);
        return error(format!(
            "Rebuilding the tools binary failed:\n\n```\n{}\n```",
            if diagnostics.is_empty() {
                strip(&stdout)
            } else {
                diagnostics
            }
        ));
    }

    // `cargo install` renames the new binary over the old one, so the process
    // serving this call keeps running from the file it opened. The rebuilt code
    // is what answers the *next* call, which is worth saying out loud: a tool
    // whose behavior was just changed still behaves the old way in this reply.
    Ok(
        "Rebuilt and installed the `jp-tools` binary. The new code takes effect on the next tool \
         call, not this one."
            .to_owned()
            .into(),
    )
}

/// Strip ANSI escapes and trim, capping the result.
fn strip(output: &str) -> String {
    let stripped = strip_ansi_escapes::strip_str(output);
    truncate(stripped.trim(), MAX_DIAGNOSTIC_BYTES)
}

#[cfg(test)]
#[path = "install_tools_tests.rs"]
mod tests;
