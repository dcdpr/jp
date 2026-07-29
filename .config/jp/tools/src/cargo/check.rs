use std::collections::BTreeSet;

use jp_tool::Context;

use super::MAX_DIAGNOSTIC_BYTES;
use crate::util::{
    ToolResult, error,
    runner::{DuctProcessRunner, ProcessOutput, ProcessRunner},
    truncate,
};

/// Rustdoc lints that are denied, kept in lockstep with `just docs-ci`.
///
/// These fire on documentation content rather than on code, so `cargo clippy`
/// and `cargo check` never see them and they otherwise surface only on CI.
const RUSTDOC_LINTS: &[&str] = &[
    "-D rustdoc::broken-intra-doc-links",
    "-D rustdoc::private-intra-doc-links",
    "-D rustdoc::invalid-codeblock-attributes",
    "-D rustdoc::invalid-html-tags",
    "-D rustdoc::invalid-rust-codeblocks",
    "-D rustdoc::bare-urls",
    "-D rustdoc::unescaped-backticks",
    "-D rustdoc::redundant-explicit-links",
];

pub(crate) async fn cargo_check(
    ctx: &Context,
    package: Option<String>,
    checksum_freshness: bool,
    docs: bool,
) -> ToolResult {
    cargo_check_impl(
        ctx,
        package.as_deref(),
        checksum_freshness,
        docs,
        &DuctProcessRunner,
    )
}

fn cargo_check_impl<R: ProcessRunner>(
    ctx: &Context,
    package: Option<&str>,
    checksum_freshness: bool,
    docs: bool,
    runner: &R,
) -> ToolResult {
    let clippy_scope = package.map_or("--workspace".to_owned(), |v| format!("--package={v}"));

    // Prevent warnings from being treated as errors, e.g. on CI.
    let mut env = vec![("RUSTFLAGS", "-W warnings")];
    if checksum_freshness {
        // Use content checksums instead of file mtimes for cargo's freshness
        // checks, so that sibling checkouts (git worktrees) sharing a target
        // dir cannot serve each other's stale artifacts. Matches CI. Requires
        // nightly cargo. See rust-lang/cargo#14136.
        env.push(("CARGO_UNSTABLE_CHECKSUM_FRESHNESS", "true"));
    }

    let ProcessOutput { stderr, status, .. } = runner.run_with_env(
        "cargo",
        &[
            "clippy",
            "--color=never",
            &clippy_scope,
            "--quiet",
            "--all-targets",
            // Matches `just lint-ci`. Code behind an optional feature is not
            // compiled without this, so its lints surface only on CI.
            "--all-features",
        ],
        &ctx.root,
        &env,
    )?;

    if !status.is_success() {
        return error(format!(
            "Cargo command failed: {}",
            truncate(&stderr, MAX_DIAGNOSTIC_BYTES)
        ));
    }

    // Strip ANSI escape codes
    let clippy = strip_ansi_escapes::strip_str(stderr);
    let clippy = truncate(clippy.trim(), MAX_DIAGNOSTIC_BYTES);

    let doc_note = match doc_check(ctx, package, checksum_freshness, docs, runner)? {
        DocCheck::Skipped | DocCheck::Clean => None,
        // Deliberately silent on *why* it failed: exit 101 also covers cargo
        // errors, `cfg(doc)` compile errors and rustdoc crashes, and nothing in
        // the exit status distinguishes those from a denied lint. The
        // diagnostics below say which it was.
        DocCheck::Failed(diagnostics) => Some(format!(
            "`cargo doc` failed. This pass runs the documentation lints CI denies (`just \
             docs-ci`), which clippy does not report:\n\n```\n{diagnostics}\n```"
        )),
    };

    let comfort_note = match comfort_check(ctx, package, runner)? {
        ComfortCheck::Clean => None,
        ComfortCheck::Drift(note) => Some(note),
        ComfortCheck::Failed(stderr) => {
            return error(format!(
                "comfort failed: {}",
                truncate(&stderr, MAX_DIAGNOSTIC_BYTES)
            ));
        }
    };

    // Hardest-to-ignore first: a failed doc pass blocks CI, comfort drift is
    // auto-fixable.
    let extra: Vec<String> = doc_note.into_iter().chain(comfort_note).collect();

    if extra.is_empty() {
        return Ok(if clippy.is_empty() {
            "Check succeeded. No warnings or errors found."
                .to_owned()
                .into()
        } else {
            format!("```\n{clippy}\n```\n").into()
        });
    }

    // Something below failed, so the header is scoped to what clippy alone
    // found. A bare "Check succeeded" would contradict the sections that follow.
    let header = if clippy.is_empty() {
        "`cargo clippy` found no warnings or errors.".to_owned()
    } else {
        format!("```\n{clippy}\n```")
    };

    let mut sections = vec![header];
    sections.extend(extra);
    Ok(sections.join("\n\n").into())
}

enum DocCheck {
    /// The caller opted out of the documentation pass.
    Skipped,
    /// `cargo doc` succeeded, so no denied lint fired.
    Clean,
    /// `cargo doc` exited non-zero; carries whatever it reported.
    ///
    /// A denied lint is the expected cause, but the exit status alone cannot
    /// rule out a cargo error, a `cfg(doc)` compile error or a rustdoc crash,
    /// so this variant does not claim to know which.
    Failed(String),
}

/// Run `cargo doc` with the rustdoc lints CI denies.
///
/// `--document-private-items` is required, not cosmetic: without it
/// `private-intra-doc-links` cannot fire at all, which is the lint that catches
/// a public doc comment linking to a private item.
///
/// Shares cargo's default profile and feature set with the clippy pass above,
/// rather than the `docs` profile CI runs under: that pass already holds the
/// profile's build lock (`target/<profile>/.cargo-lock`) and has already built
/// every dependency unit, so this leaves rustdoc over the workspace crates as
/// the only new work and adds no contention the same invocation wasn't causing
/// already.
/// Profile choice does not affect which documentation lints fire.
fn doc_check<R: ProcessRunner>(
    ctx: &Context,
    package: Option<&str>,
    checksum_freshness: bool,
    enabled: bool,
    runner: &R,
) -> Result<DocCheck, std::io::Error> {
    if !enabled {
        return Ok(DocCheck::Skipped);
    }

    let scope = package.map_or("--workspace".to_owned(), |v| format!("--package={v}"));
    let rustdocflags = RUSTDOC_LINTS.join(" ");

    let mut env = vec![("RUSTDOCFLAGS", rustdocflags.as_str())];
    if checksum_freshness {
        env.push(("CARGO_UNSTABLE_CHECKSUM_FRESHNESS", "true"));
    }

    let ProcessOutput { stderr, status, .. } = runner.run_with_env(
        "cargo",
        &[
            "doc",
            "--color=never",
            &scope,
            "--quiet",
            // `just docs-ci` documents every feature. Without this, code behind
            // an optional feature is never compiled, so its doc comments go
            // unlinted here and fail on CI instead.
            "--all-features",
            "--no-deps",
            "--document-private-items",
            // Report every crate's diagnostics in one pass rather than stopping
            // at the first failing crate.
            "--keep-going",
        ],
        &ctx.root,
        &env,
    )?;

    if status.is_success() {
        return Ok(DocCheck::Clean);
    }

    let diagnostics = strip_ansi_escapes::strip_str(stderr);
    let diagnostics = diagnostics.trim();

    // An empty stderr leaves nothing to report but the status, which is still
    // worth surfacing.
    Ok(DocCheck::Failed(if diagnostics.is_empty() {
        format!("`cargo doc` failed with exit status {status} and no diagnostics.")
    } else {
        truncate(diagnostics, MAX_DIAGNOSTIC_BYTES)
    }))
}

enum ComfortCheck {
    /// All doc comments are well-formatted.
    Clean,
    /// Some doc comments would be reformatted; carries the user-facing note
    /// listing the offending files.
    Drift(String),
    /// comfort itself failed (parse error, bad package name); carries stderr.
    Failed(String),
}

/// Run comfort in `--check` mode to surface badly formatted doc comments.
///
/// Drift is not a failure: `cargo_fmt` auto-fixes it, so it comes back as a
/// [`ComfortCheck::Drift`] note rather than an error.
fn comfort_check<R: ProcessRunner>(
    ctx: &Context,
    package: Option<&str>,
    runner: &R,
) -> Result<ComfortCheck, std::io::Error> {
    let mut comfort_args = vec![
        "--check",
        "--list-changed",
        "--format-markdown",
        "--reference-links",
        "--prune-reference-links",
        "--language",
        "rust",
    ];
    if let Some(pkg) = package {
        comfort_args.push("--package");
        comfort_args.push(pkg);
    } else {
        comfort_args.push("--workspace");
    }

    let ProcessOutput {
        stderr,
        status,
        stdout,
    } = runner.run_with_env("comfort", &comfort_args, &ctx.root, &[])?;

    let strip_root = |line: &str| -> String {
        line.trim_start_matches(ctx.root.as_str())
            .trim_start_matches('/')
            .to_owned()
    };

    let files: BTreeSet<String> = stdout
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(strip_root)
        .collect();

    if files.is_empty() {
        // In `--check` mode comfort exits non-zero with the drifting files on
        // stdout. A non-zero exit with no files listed is a genuine failure.
        if status.is_success() {
            return Ok(ComfortCheck::Clean);
        }
        return Ok(ComfortCheck::Failed(stderr));
    }

    let listing = files.into_iter().collect::<Vec<_>>().join("\n- ");
    Ok(ComfortCheck::Drift(format!(
        "Doc comments in the following files are badly formatted. Run `cargo_fmt` to auto-fix \
         them:\n- {}",
        truncate(&listing, MAX_DIAGNOSTIC_BYTES)
    )))
}

#[cfg(test)]
#[path = "check_tests.rs"]
mod tests;
