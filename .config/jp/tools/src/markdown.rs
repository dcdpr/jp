//! The `markdown_format` tool: reflow standalone Markdown files with `comfort`.
//!
//! Covers the `.md` files that live outside Rust sources — RFDs, tickets,
//! READMEs, the docs site.
//! Doc comments inside `.rs` files are `cargo_format`'s territory.
//!
//! The `comfort` flags match the `fmt-markdown-ci` recipe in the justfile, so a
//! file this tool leaves alone is a file CI accepts.

use std::collections::BTreeSet;

use jp_tool::Context;

use crate::{
    Tool,
    fs::utils::resolve_workspace_path,
    util::{
        OneOrMany, ToolResult, error,
        runner::{DuctProcessRunner, ProcessOutput, ProcessRunner},
        truncate, unknown_tool,
    },
};

/// Cap for the formatted-file listing embedded in a tool result.
///
/// A workspace-wide run after a broad edit can touch hundreds of files, and the
/// head of the list is what the reader needs.
const MAX_LISTING_BYTES: usize = 32_000;

#[expect(
    clippy::needless_pass_by_value,
    reason = "consistent with other module run fns"
)]
pub fn run(ctx: Context, t: Tool) -> ToolResult {
    match t.name.trim_start_matches("markdown_") {
        "format" => markdown_format(&ctx, t.opt("paths")?),
        _ => unknown_tool(t),
    }
}

fn markdown_format(ctx: &Context, paths: Option<OneOrMany<String>>) -> ToolResult {
    markdown_format_impl(ctx, paths, &DuctProcessRunner)
}

fn markdown_format_impl<R: ProcessRunner>(
    ctx: &Context,
    paths: Option<OneOrMany<String>>,
    runner: &R,
) -> ToolResult {
    let selected = match select_paths(ctx, paths.unwrap_or_default().as_slice()) {
        Ok(selected) => selected,
        Err(message) => return error(message),
    };

    let mut args = vec![
        "--list-changed",
        "--format-markdown",
        "--reference-links",
        "--prune-reference-links",
        "--language",
        "markdown",
    ];
    if selected.is_empty() {
        args.push("--workspace");
    } else {
        args.extend(selected.iter().map(String::as_str));
    }

    let ProcessOutput {
        stdout,
        stderr,
        status,
    } = runner.run("comfort", &args, &ctx.root)?;

    if !status.is_success() {
        return error(format!(
            "comfort failed: {}",
            truncate(&stderr, MAX_LISTING_BYTES)
        ));
    }

    // Workspace runs report absolute paths; strip the root so the listing reads
    // the way the rest of the tools spell a path.
    let mut files: BTreeSet<String> = BTreeSet::new();
    for line in stdout.lines() {
        let trimmed = line.trim();
        if !trimmed.is_empty() {
            files.insert(
                trimmed
                    .trim_start_matches(ctx.root.as_str())
                    .trim_start_matches('/')
                    .to_owned(),
            );
        }
    }

    if files.is_empty() {
        Ok("No files to format.".into())
    } else {
        let listing = files.into_iter().collect::<Vec<_>>().join("\n- ");
        Ok(format!(
            "Formatted files:\n- {}",
            truncate(&listing, MAX_LISTING_BYTES)
        )
        .into())
    }
}

/// Validate each requested path and return its workspace-relative form.
///
/// An empty request yields an empty selection, which the caller turns into a
/// whole-workspace run.
///
/// `comfort` walks a directory and filters it down to Markdown extensions, but
/// takes a named file as-is whatever its extension — so a file that isn't
/// Markdown is refused here rather than rewritten as Markdown.
fn select_paths(ctx: &Context, paths: &[String]) -> Result<Vec<String>, String> {
    paths
        .iter()
        .map(|path| {
            let resolved = resolve_workspace_path(&ctx.root, path, ctx.access.as_ref())?;
            let markdown = matches!(resolved.relative.extension(), Some("md" | "markdown"));
            if !markdown && !resolved.absolute.is_dir() {
                return Err(format!(
                    "'{path}' is not a Markdown file or a directory. Only `.md` and `.markdown` \
                     files are formatted."
                ));
            }
            Ok(resolved.relative.into_string())
        })
        .collect()
}

#[cfg(test)]
#[path = "markdown_tests.rs"]
mod tests;
