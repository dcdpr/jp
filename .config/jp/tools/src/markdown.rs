//! The `markdown_format` tool: reflow standalone Markdown files with `comfort`.
//!
//! Covers the standalone Markdown files (`.md` and `.markdown`) that live
//! outside Rust sources — RFDs, tickets, READMEs, the docs site.
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

/// Cap for each chunk of `comfort` output embedded in a tool result.
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
    // An omitted `paths` means the whole workspace; an empty list means the
    // caller selected nothing, and silently widening that to every Markdown
    // file in the workspace rewrites files nobody asked about.
    let selected = match paths {
        None => vec![],
        Some(paths) if paths.as_slice().is_empty() => {
            return error(
                "`paths` is empty. Name the files or directories to format, or omit `paths` \
                 entirely to format every Markdown file in the workspace.",
            );
        }
        Some(paths) => match select_paths(ctx, paths.as_slice()) {
            Ok(selected) => selected,
            Err(message) => return error(message),
        },
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

    let changed = changed_files(&stdout, ctx.root.as_str());

    if !status.is_success() {
        let mut message = format!("comfort failed: {}", truncate(&stderr, MAX_LISTING_BYTES));
        if !changed.is_empty() {
            // `comfort` writes each file as it goes, so whatever it reported
            // before it stopped is already on disk.
            message.push_str("\n\nAlready reformatted before the failure:\n- ");
            message.push_str(&truncate(&changed.join("\n- "), MAX_LISTING_BYTES));
        }
        return error(message);
    }

    if changed.is_empty() {
        Ok("No files to format.".into())
    } else {
        Ok(format!(
            "Formatted files:\n- {}",
            truncate(&changed.join("\n- "), MAX_LISTING_BYTES)
        )
        .into())
    }
}

/// The files `comfort` reported as changed, deduplicated and sorted.
///
/// Workspace runs report absolute paths; the root is stripped so the listing
/// reads the way the rest of the tools spell a path.
fn changed_files(stdout: &str, root: &str) -> Vec<String> {
    stdout
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(|line| {
            line.trim_start_matches(root)
                .trim_start_matches('/')
                .to_owned()
        })
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

/// Validate each requested path and return its workspace-relative form.
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
