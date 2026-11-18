//! Workspace scaffolding for `jp review`: tool definitions (`jp review tools
//! init`) and the review pass configuration (`jp review init`).
//!
//! Existing files with different contents are overwritten only when git can
//! restore them or when `--force` is passed (see [`overwrite`]).

use camino::Utf8Path;
use tool_definitions::Definitions;

use crate::Output;

/// The tool definitions this plugin ships.
pub(crate) const DEFINITIONS: Definitions = Definitions {
    plugin: "review",
    files: &[
        ("comment.toml", include_str!("../tools/comment.toml")),
        ("reply.toml", include_str!("../tools/reply.toml")),
    ],
    offers: None,
};

/// The file `jp review init` writes.
pub(crate) const CONFIG_FILE: &str = "review.toml";

/// What `jp review init` writes into [`CONFIG_FILE`].
pub(crate) const CONFIG: &str = include_str!("../config/review.toml");

/// Write the tool definitions into `dir`, creating it if needed.
pub(crate) fn tools_init(dir: &Utf8Path, force: bool) -> Result<Output, String> {
    DEFINITIONS.init(dir, force).map(Output::from)
}

/// Write [`CONFIG_FILE`] into `dir`, creating it if needed.
pub(crate) fn init(dir: &Utf8Path, force: bool) -> Result<Output, String> {
    let command = format!("jp review init {dir}");
    let written = overwrite::write(dir, &[(CONFIG_FILE, CONFIG)], &command, force)?;

    let mut text = String::new();
    for (path, written) in &written {
        text.push_str(&format!("{written} {path}\n"));
    }
    text.push_str(
        "Add it to `plugins.command.review.options.cfg`, the way `--cfg` takes it, for passes to \
         run under it.\n",
    );

    Ok(text.into())
}

#[cfg(test)]
#[path = "definitions_tests.rs"]
mod tests;
