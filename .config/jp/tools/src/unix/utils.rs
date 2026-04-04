use std::path::Path;

use camino::Utf8Path;
use clean_path::clean;
use jp_tool::Context;
use serde::Serialize;

use crate::{
    to_xml,
    util::{
        OneOrMany, ToolResult, error,
        runner::{DuctProcessRunner, ProcessOutput, ProcessRunner, RunnerOpts},
    },
};

const ALLOWED_UTILS: &[&str] = &[
    "base64", "bc", "date", "file", "head", "jq", "shasum", "sort", "tail", "uname", "uniq",
    "uuidgen", "wc",
];

/// Truncate output beyond this limit to avoid burning tokens on huge results.
const MAX_OUTPUT_BYTES: usize = 100_000;

/// Characters that tools commonly use to separate multiple values within a
/// single argument. Splitting on these before scanning ensures that
/// `/etc/passwd:/etc/shadow` or `--path=/a;/b` are checked individually.
const ARG_DELIMITERS: &[char] = &['=', ':', ';', ',', ' ', '\t', '\n', '\0'];

pub(crate) fn unix_utils(
    ctx: &Context,
    util: &str,
    args: Option<OneOrMany<String>>,
    stdin: Option<&str>,
) -> ToolResult {
    unix_utils_impl(ctx, util, args, stdin, &DuctProcessRunner)
}

fn unix_utils_impl<R: ProcessRunner>(
    ctx: &Context,
    util: &str,
    args: Option<OneOrMany<String>>,
    stdin: Option<&str>,
    runner: &R,
) -> ToolResult {
    if !ALLOWED_UTILS.contains(&util) {
        return error(format!(
            "Unknown util '{util}'. Allowed: {}",
            ALLOWED_UTILS.join(", ")
        ));
    }

    let args: Vec<String> = args.map(OneOrMany::into_vec).unwrap_or_default();

    if let Err(msg) = validate_args(&ctx.root, &args, Path::exists) {
        return error(msg);
    }

    let arg_refs: Vec<&str> = args.iter().map(String::as_str).collect();

    let resolved = resolve_binary(util).map_err(|e| format!("Failed to resolve '{util}': {e}"))?;
    let exec_str = resolved.exec_path.to_string_lossy();

    let sandbox_profile = sandbox_profile(&ctx.root, util, &resolved)
        .map_err(|e| format!("Failed to build sandbox: {e}"))?;
    let sandbox_env = sandbox_env(util);
    let env_refs: Vec<(&str, &str)> = sandbox_env
        .iter()
        .map(|(k, v)| (k.as_str(), v.as_str()))
        .collect();
    let opts = RunnerOpts {
        stdin,
        env: &env_refs,
        clean_env: true,
        macos_sandbox_profile: sandbox_profile.as_deref(),
    };

    let ProcessOutput {
        stdout,
        stderr,
        status,
    } = runner.run_with_opts(&exec_str, &arg_refs, &ctx.root, &opts)?;

    let output = CommandOutput {
        stdout: truncate(stdout.trim_end()),
        stderr: truncate(stderr.trim_end()),
        status: status.to_string(),
    };

    Ok(to_xml(output)?.into())
}

// ---------------------------------------------------------------------------
// Output truncation
// ---------------------------------------------------------------------------

fn truncate(s: &str) -> String {
    if s.len() <= MAX_OUTPUT_BYTES {
        return s.to_owned();
    }

    let end = s.floor_char_boundary(MAX_OUTPUT_BYTES);
    format!(
        "{}\n\n[Truncated: showing {end} of {} bytes]",
        &s[..end],
        s.len()
    )
}

// ---------------------------------------------------------------------------
// Argument validation
// ---------------------------------------------------------------------------

/// Scan every argument for embedded path references outside the workspace.
///
/// Each argument goes through two passes:
///
/// 1. **Whole-argument pass** — the argument is normalized with
///    `clean_path::clean` and scanned as-is. This catches paths that
///    contain delimiter characters (e.g. a file literally named
///    `foo:bar`).
/// 2. **Fragment pass** — the argument is split on common delimiters
///    (`=`, `:`, `;`, `,`, whitespace, null), each fragment is
///    normalized independently, and scanned. This catches multi-path
///    arguments like `/etc/passwd:/etc/shadow` and resolves `..`
///    sequences that only simplify within an individual fragment.
///
/// Within each pass, every byte position is checked for path-start
/// characters (`/`, `~`, `.`).
///
/// The `exists` function is injected for testability — production passes
/// `Path::exists`, tests pass a closure controlling which paths "exist".
fn validate_args(
    root: &Utf8Path,
    args: &[String],
    exists: impl Fn(&Path) -> bool,
) -> Result<(), String> {
    let root_std = root.as_std_path();

    for arg in args {
        // Pass 1: scan the full argument (normalized).
        let whole = clean(Path::new(arg.as_str()));
        let whole_str = whole.to_string_lossy();
        scan_fragment(root_std, &whole_str, arg, &exists)?;

        // Pass 2: split on delimiters, normalize each fragment, scan.
        for fragment in arg.split(ARG_DELIMITERS).filter(|s| !s.is_empty()) {
            let normalized = clean(Path::new(fragment));
            let normalized_str = normalized.to_string_lossy();
            scan_fragment(root_std, &normalized_str, arg, &exists)?;
        }
    }

    Ok(())
}

/// Whether a byte is a path separator on the current platform.
///
/// On Unix only `/` counts; on Windows both `/` and `\` do.
fn is_sep(b: u8) -> bool {
    std::path::is_separator(b as char)
}

/// Scan a single fragment byte-by-byte for path references outside the
/// workspace.
fn scan_fragment(
    root: &Path,
    fragment: &str,
    original_arg: &str,
    exists: &dyn Fn(&Path) -> bool,
) -> Result<(), String> {
    let bytes = fragment.as_bytes();

    for i in 0..bytes.len() {
        let ch = bytes[i];
        if !is_sep(ch) && ch != b'~' && ch != b'.' {
            continue;
        }

        let candidate = &fragment[i..];

        // Tilde: always reject (tools may expand ~ internally).
        if ch == b'~'
            && (candidate == "~" || (candidate.len() > 1 && is_sep(candidate.as_bytes()[1])))
        {
            return Err(format!(
                "Home directory references are not allowed: '{original_arg}'"
            ));
        }

        // Absolute path: reject if it resolves to an existing path
        // outside the workspace.
        if is_sep(ch) {
            let normalized = clean(Path::new(candidate));
            if exists(&normalized) && !normalized.starts_with(root) {
                return Err(format!(
                    "Argument references a path outside the workspace: '{original_arg}'"
                ));
            }
        }

        // Dot as path start: at position 0 or right after a path separator.
        // Treats the substring as relative to the workspace root and rejects if
        // it escapes — regardless of whether the target exists.
        if ch == b'.' && (i == 0 || is_sep(bytes[i - 1])) {
            let joined = root.join(candidate);
            let normalized = clean(&joined);
            if !normalized.starts_with(root) {
                return Err(format!("Path escapes the workspace: '{original_arg}'"));
            }
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// macOS sandbox profile
// ---------------------------------------------------------------------------

/// Build a macOS Seatbelt sandbox profile for a resolved binary.
///
/// Deny-default with a dynamically constructed allow-list:
/// - `(literal "/")` — root directory traversal.
/// - `/dev` — stdio file descriptors.
/// - Workspace root — the files the tool should operate on.
/// - Symlink chain ancestors — every directory from the `which` path to
///   the canonical binary, including resolved directory symlinks (e.g.
///   `/etc` → `/private/etc`). Required for `execvp` traversal.
/// - Canonical binary directory — the real executable.
/// - Transitive library directories — from `otool -L`.
/// - Per-util extra paths — e.g. timezone data for `date`.
///
/// Everything else (writes, network, `/Users`, `/tmp`, `/home`, etc.) is
/// denied.
fn sandbox_profile(
    workspace_root: &Utf8Path,
    util: &str,
    binary: &ResolvedBinary,
) -> Result<Option<String>, std::io::Error> {
    if !cfg!(target_os = "macos") {
        return Ok(None);
    }

    let mut read_paths: Vec<String> = vec!["/dev".to_owned(), workspace_root.to_string()];

    // Symlink chain: every ancestor of every link in the chain.
    read_paths.extend(collect_symlink_dirs(&binary.exec_path));

    // Canonical binary directory.
    if let Some(dir) = binary.canonical_path.parent() {
        read_paths.push(dir.to_string_lossy().into_owned());
    }

    // Transitive shared library directories.
    read_paths.extend(resolve_library_dirs(&binary.canonical_path)?);

    // Per-util extra paths that can't be inferred dynamically.
    read_paths.extend(extra_read_paths(util).iter().map(|s| (*s).to_owned()));

    read_paths.sort();
    read_paths.dedup();

    let mut subpaths = String::new();
    for p in &read_paths {
        subpaths.push_str(&format!("    (subpath \"{p}\")\n"));
    }

    Ok(Some(format!(
        "(version 1)\n\
         (deny default)\n\
         (allow process*)\n\
         (allow sysctl*)\n\
         (allow file-read*\n\
         \x20   (literal \"/\")\n\
         {subpaths})"
    )))
}

/// Additional read paths required by specific utilities that cannot be
/// discovered via `otool -L` or symlink resolution.
fn extra_read_paths(util: &str) -> &'static [&'static str] {
    match util {
        // Timezone data for correct local time display.
        "date" => &["/var/db/timezone", "/private/var/db/timezone"],
        _ => &[],
    }
}

/// Build a minimal environment for the sandboxed process.
///
/// The parent's environment is NOT inherited (`clean_env: true`). Only
/// the variables returned here are set. This prevents leaking secrets
/// like API keys, SSH agent sockets, or home directory paths.
fn sandbox_env(util: &str) -> Vec<(String, String)> {
    let mut env = vec![
        // PATH is needed by some tools that spawn sub-processes.
        (
            "PATH".to_owned(),
            "/usr/bin:/bin:/usr/sbin:/sbin".to_owned(),
        ),
    ];

    // Locale for text-processing utilities.
    if let Ok(lang) = std::env::var("LANG") {
        env.push(("LANG".to_owned(), lang));
    }
    if let Ok(lc) = std::env::var("LC_ALL") {
        env.push(("LC_ALL".to_owned(), lc));
    }

    // Timezone for date.
    if util == "date"
        && let Ok(tz) = std::env::var("TZ")
    {
        env.push(("TZ".to_owned(), tz));
    }

    env
}

// ---------------------------------------------------------------------------
// Binary and library resolution
// ---------------------------------------------------------------------------

/// Resolved binary paths for a utility.
struct ResolvedBinary {
    /// The symlink path from `which` — used for execution so that multicall
    /// binaries (e.g. coreutils) see the correct program name in `argv[0]`.
    exec_path: std::path::PathBuf,

    /// The canonical path with symlinks resolved — used for the sandbox
    /// profile and `otool -L`.
    canonical_path: std::path::PathBuf,
}

fn resolve_binary(util: &str) -> Result<ResolvedBinary, std::io::Error> {
    let output = std::process::Command::new("which").arg(util).output()?;

    if !output.status.success() {
        return Err(std::io::Error::other(format!(
            "Could not find '{util}' on PATH"
        )));
    }

    let exec_path = std::path::PathBuf::from(String::from_utf8_lossy(&output.stdout).trim());
    let canonical_path = std::fs::canonicalize(&exec_path)?;
    Ok(ResolvedBinary {
        exec_path,
        canonical_path,
    })
}

/// Walk the symlink chain from `path` to the real file, collecting every
/// directory the sandbox needs to traverse.
///
/// For each link in the chain, adds every ancestor directory up to `/`
/// (since `sandbox-exec` needs `subpath` access to each intermediate
/// directory for `execvp` traversal). Also resolves directory symlinks
/// (e.g. `/etc` → `/private/etc`) so their real paths are included.
fn collect_symlink_dirs(path: &Path) -> Vec<String> {
    let mut dirs = Vec::new();
    let mut current = path.to_path_buf();

    for _ in 0..32 {
        let mut p: &Path = &current;
        while let Some(parent) = p.parent() {
            if parent.as_os_str().is_empty() {
                break;
            }
            dirs.push(parent.to_string_lossy().into_owned());

            if let Ok(resolved) = std::fs::canonicalize(parent)
                && resolved != parent
            {
                dirs.push(resolved.to_string_lossy().into_owned());
            }
            p = parent;
        }

        match std::fs::read_link(&current) {
            Ok(target) => {
                current = if target.is_relative() {
                    current.parent().map(|p| p.join(&target)).unwrap_or(target)
                } else {
                    target
                };
            }
            Err(_) => break,
        }
    }

    dirs
}

/// Return the unique parent directories of all shared libraries the binary
/// links against, resolved recursively via `otool -L`.
fn resolve_library_dirs(binary: &Path) -> Result<Vec<String>, std::io::Error> {
    use std::collections::BTreeSet;

    let mut dirs = BTreeSet::new();
    let mut seen_libs = BTreeSet::new();
    let mut queue = vec![binary.to_path_buf()];

    while let Some(target) = queue.pop() {
        let output = std::process::Command::new("otool")
            .args(["-L", &target.to_string_lossy()])
            .output()?;

        if !output.status.success() {
            dirs.insert("/usr/lib".to_owned());
            break;
        }

        let stdout = String::from_utf8_lossy(&output.stdout);

        for line in stdout.lines().skip(1) {
            let lib_path = line.split_whitespace().next().unwrap_or("");
            if lib_path.is_empty() {
                continue;
            }

            let resolved = std::fs::canonicalize(lib_path)
                .unwrap_or_else(|_| std::path::PathBuf::from(lib_path));

            if !seen_libs.insert(resolved.clone()) {
                continue;
            }

            if let Some(dir) = resolved.parent() {
                dirs.insert(dir.to_string_lossy().into_owned());
            }

            if resolved.exists() && resolved != target {
                queue.push(resolved);
            }
        }
    }

    Ok(dirs.into_iter().collect())
}

// ---------------------------------------------------------------------------
// Output serialization
// ---------------------------------------------------------------------------

#[derive(Serialize)]
struct CommandOutput {
    #[serde(skip_serializing_if = "String::is_empty")]
    stdout: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    stderr: String,
    #[serde(skip_serializing_if = "is_zero")]
    status: String,
}

fn is_zero(s: &str) -> bool {
    s == "0"
}

#[cfg(test)]
#[path = "utils_tests.rs"]
mod tests;
