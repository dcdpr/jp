//! Orchestration layer: parses CLI intent, walks filesystem, dispatches to the
//! pure format pipeline, handles `--check` diffing and exit codes.
//!
//! This is the imperative shell.
//! The functional core lives in [`format`] and [`extract`].
//!
//! [`extract`]: super::extract
//! [`format`]: super::format

use std::{
    io::{self, IsTerminal, Read, Write},
    path::{Path, PathBuf},
};

use similar::{ChangeTag, TextDiff};

use crate::{
    Error,
    cli::{Cli, Format, Invocation, Language},
    format::{FormatOptions, format_markdown_with, format_rust_source_with},
    walk::{expand_path, workspace_files},
};

/// Top-level entry point.
/// Returns an [`Error`] for I/O failures; returns [`Error::CheckFailed`] when
/// `--check` finds drift.
pub fn run(cli: &Cli, invocation: Invocation) -> Result<(), Error> {
    // Source selection. The intent ladder:
    //   1. Workspace mode (explicit `--workspace`, or `-p`/`--exclude`
    //      restricting which packages to walk).
    //   2. Explicit paths process those paths.
    //   3. No paths + cargo invocation: workspace (all packages).
    //   4. No paths + direct invocation: stdin/stdout.
    let opts = FormatOptions {
        max_width: cli.max_width,
        canonical: cli.format_markdown,
        reference_links: cli.reference_links,
        prune_reference_links: cli.prune_reference_links,
    };
    let workspace_mode = cli.workspace || !cli.packages.is_empty() || !cli.exclude.is_empty();
    if workspace_mode {
        let files = workspace_files(&cli.packages, &cli.exclude, cli.language)?;
        return run_files(files, cli.language, cli.check, cli.list_changed, &opts);
    }
    if !cli.paths.is_empty() {
        let mut files = Vec::new();
        for path in &cli.paths {
            files.extend(expand_path(path, cli.language)?);
        }
        return run_files(files, cli.language, cli.check, cli.list_changed, &opts);
    }
    if invocation == Invocation::Cargo {
        let files = workspace_files(&[], &[], cli.language)?;
        return run_files(files, cli.language, cli.check, cli.list_changed, &opts);
    }

    // Default for direct invocation: stdin → stdout (or stdin → check-diff).
    if io::stdin().is_terminal() {
        let mut stderr = io::stderr().lock();
        writeln!(
            stderr,
            "comfort: no input. Pass paths, use --workspace, or pipe source on stdin."
        )?;
        return Ok(());
    }
    run_stdin(
        cli.language,
        cli.check,
        cli.list_changed,
        cli.stdin_filename.as_deref(),
        &opts,
    )
}

fn run_stdin(
    language: Language,
    check: bool,
    list_changed: bool,
    stdin_filename: Option<&Path>,
    opts: &FormatOptions,
) -> Result<(), Error> {
    let mut buf = String::new();
    io::stdin().read_to_string(&mut buf)?;

    let format = language.resolve(stdin_filename);
    let formatted = format_for(&buf, format, opts);

    let label = stdin_filename.unwrap_or(Path::new("<stdin>"));

    if check {
        if formatted != buf {
            if list_changed {
                writeln!(io::stdout().lock(), "{}", label.display())?;
            } else {
                print_diff(label, &buf, &formatted)?;
            }
            return Err(Error::CheckFailed(1));
        }
        return Ok(());
    }

    // Write mode + `--list-changed`: announce the label on stderr so it
    // doesn't corrupt the formatted-content stream on stdout. (In check
    // mode there's no payload on stdout, so the label goes there to match
    // the file-walk path.)
    if list_changed && formatted != buf {
        writeln!(io::stderr().lock(), "{}", label.display())?;
    }

    let mut stdout = io::stdout().lock();
    stdout.write_all(formatted.as_bytes())?;
    Ok(())
}

fn run_files(
    files: Vec<PathBuf>,
    language: Language,
    check: bool,
    list_changed: bool,
    opts: &FormatOptions,
) -> Result<(), Error> {
    let mut changed = 0_usize;
    let mut stdout = io::stdout().lock();

    for path in files {
        let source = std::fs::read_to_string(&path).map_err(|source| Error::ReadFile {
            path: path.clone(),
            source,
        })?;
        let format = language.resolve(Some(&path));
        let formatted = format_for(&source, format, opts);
        if formatted == source {
            continue;
        }

        changed += 1;
        if list_changed {
            writeln!(stdout, "{}", path.display())?;
        } else if check {
            print_diff(&path, &source, &formatted)?;
        }
        if !check {
            std::fs::write(&path, formatted).map_err(|source| Error::WriteFile {
                path: path.clone(),
                source,
            })?;
        }
    }

    if check && changed > 0 {
        return Err(Error::CheckFailed(changed));
    }
    Ok(())
}

/// Dispatch to the right pipeline for the resolved format.
/// Both optional transformations (`--format-markdown` for structural
/// canonicalisation, `--reference-links` for link extraction) compose
/// orthogonally on top of the always-on sembr reflow.
fn format_for(source: &str, format: Format, opts: &FormatOptions) -> String {
    match format {
        Format::Rust => format_rust_source_with(source, opts),
        Format::Markdown => format_markdown_with(source, opts),
    }
}

fn print_diff(label: &Path, old: &str, new: &str) -> Result<(), io::Error> {
    let diff = TextDiff::from_lines(old, new);
    let mut out = io::stdout().lock();

    writeln!(out, "--- {}", label.display())?;
    writeln!(out, "+++ {} (formatted)", label.display())?;

    for hunk in diff.unified_diff().iter_hunks() {
        writeln!(out, "{}", hunk.header())?;
        for change in hunk.iter_changes() {
            let sigil = match change.tag() {
                ChangeTag::Delete => '-',
                ChangeTag::Insert => '+',
                ChangeTag::Equal => ' ',
            };
            write!(out, "{sigil}{}", change.value())?;
            if !change.value().ends_with('\n') {
                writeln!(out)?;
            }
        }
    }
    Ok(())
}
