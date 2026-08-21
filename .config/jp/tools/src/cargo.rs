use std::time::{Duration, Instant};

use camino::{Utf8Path, Utf8PathBuf};
use jp_tool::{AccessPolicy, Capability, Outcome};
use serde_json::Value;

use crate::{
    Context, Tool,
    fs::utils::{authorize, resolve_workspace_path},
    util::{OneOrMany, ToolResult, error, unknown_tool},
};

mod check;
mod expand;
mod format;
mod install_tools;
mod test;
mod update;

use check::cargo_check;
use expand::cargo_expand;
use format::cargo_format;
use install_tools::cargo_install_tools;
use test::cargo_test;
use update::cargo_update;

/// Cap for compiler and linter diagnostics embedded in a tool result.
///
/// A single failure in a widely-used macro can produce tens of thousands of
/// near-identical diagnostics; the head of the output carries the root cause,
/// the tail is noise.
const MAX_DIAGNOSTIC_BYTES: usize = 32_000;

pub async fn run(ctx: Context, t: Tool) -> ToolResult {
    // Opt-in to cargo's checksum-based freshness checks (see
    // rust-lang/cargo#14136). Requires nightly cargo, so it defaults to off
    // and is enabled per-tool via `options.checksum_freshness` in the tool
    // config.
    let checksum_freshness = t.option_or("checksum_freshness", false);

    // Which cargo workspace to operate in. Defaults to the root the tool was
    // invoked with; set `options.root` in the tool config to point the cargo
    // tooling at a different cargo workspace.
    //
    // A present `root` must be a string. `option_or` cannot be used here: it
    // reports a malformed value as an absent one, which would silently run
    // `cargo_format` or `cargo_update` against the host workspace — writing to
    // it, and reporting success — after the user asked for another one.
    let configured = match t.options.get("root") {
        None | Some(Value::Null) => None,
        Some(Value::String(value)) => Some(value.clone()),
        Some(other) => {
            return error(format!(
                "The `root` tool option must be a string, got `{other}`."
            ));
        }
    };

    let subcommand = t.name.trim_start_matches("cargo_");
    let root = match cargo_root(
        &ctx.root,
        configured.as_deref(),
        ctx.access.as_ref(),
        required_capabilities(subcommand),
    ) {
        Ok(root) => root,
        Err(message) => return error(message),
    };

    // Flags to append to `RUSTFLAGS`, set via `options.rustflags`. Malformed
    // values are refused for the same reason as `root`: silently ignoring them
    // would compile with flags the caller believes are in effect.
    let rustflags = match t.options.get("rustflags") {
        None | Some(Value::Null) => rustflags(&[]),
        Some(value) => match serde_json::from_value::<OneOrMany<String>>(value.clone()) {
            Ok(flags) => rustflags(&flags.into_vec()),
            Err(_) => {
                return error(format!(
                    "The `rustflags` tool option must be a string or an array of strings, got \
                     `{value}`."
                ));
            }
        },
    };

    let started = Instant::now();
    let outcome = match subcommand {
        "check" => cargo_check(&root, &rustflags, t.opt("package")?, checksum_freshness).await,
        "expand" => {
            cargo_expand(
                &root,
                &rustflags,
                t.req("item")?,
                t.opt("package")?,
                checksum_freshness,
            )
            .await
        }
        "test" => {
            cargo_test(
                &root,
                &rustflags,
                t.opt("package")?,
                t.opt("testname")?,
                t.opt("backtrace")?,
                checksum_freshness,
            )
            .await
        }
        "format" => cargo_format(&root, &rustflags, t.opt("package")?).await,
        "install_tools" => cargo_install_tools(&root).await,
        "update" => cargo_update(&root, t.req("packages")?).await,
        _ => return unknown_tool(t),
    };

    let outcome = note_duration(outcome, started.elapsed());

    if root == ctx.root {
        return outcome;
    }

    note_root(outcome, &root)
}

/// Append how long the cargo invocation took.
///
/// Wall-clock duration is the whole signal when tuning compile times, and a
/// caller reading only the tool's text cannot otherwise tell a warm cache from
/// a full rebuild — the two are indistinguishable when both end in "Check
/// succeeded".
/// Failures are timed too: a three-second failure and a three-minute one call
/// for different responses.
fn note_duration(outcome: ToolResult, elapsed: Duration) -> ToolResult {
    let note = format!("(took {})", format_duration(elapsed));

    match outcome {
        Ok(Outcome::Success { content }) => Ok(Outcome::Success {
            content: format!("{content}\n\n{note}"),
        }),
        Ok(Outcome::Error {
            message,
            trace,
            transient,
        }) => Ok(Outcome::Error {
            message: format!("{message}\n\n{note}"),
            trace,
            transient,
        }),
        Err(error) => Err(format!("{error}\n\n{note}").into()),
        other => other,
    }
}

/// Render a duration at a precision that matches how it will be read.
///
/// Sub-minute builds are compared against each other, where a tenth of a second
/// distinguishes a warm cache from a small rebuild; past a minute nobody cares
/// about the fraction.
fn format_duration(elapsed: Duration) -> String {
    let seconds = elapsed.as_secs();

    if seconds >= 60 {
        return format!("{}m {}s", seconds / 60, seconds % 60);
    }

    format!("{:.1}s", elapsed.as_secs_f64())
}

/// Warnings are reported, not fatal: these tools surface diagnostics rather
/// than failing on them, and CI runs its own `-D warnings` pass.
const BASE_RUSTFLAGS: &str = "-W warnings";

/// Build the `RUSTFLAGS` value, appending any configured flags to the base.
///
/// Setting `RUSTFLAGS` at all overrides `rustflags` from `.cargo/config.toml`
/// wholesale, so a workspace that relies on those (a linker choice, extra `-Z`
/// flags) has to restate them through `options.rustflags`.
///
/// Every compiling cargo tool sets the variable, so they agree on the flag set
/// and a shared target directory stays warm when alternating between them.
/// Configured flags come last, so they win over the base.
fn rustflags(extra: &[String]) -> String {
    if extra.is_empty() {
        return BASE_RUSTFLAGS.to_owned();
    }

    format!("{BASE_RUSTFLAGS} {}", extra.join(" "))
}

/// Capabilities a cargo subcommand needs on the directory it runs in.
///
/// These gate whether cargo is spawned at all; they cannot bound what it does
/// once running, because the subprocess is not sandboxed.
/// Each set therefore describes everything its command can reach, rather than a
/// confinement — and even the widest of them understates reality, since an
/// unsandboxed process can touch paths outside the directory entirely.
fn required_capabilities(subcommand: &str) -> &'static [Capability] {
    match subcommand {
        // Rewrites existing sources in place; never compiles, never creates.
        "format" => &[Capability::Read, Capability::Update],
        // Rewrites the lockfile, and creates it when the target has none.
        "update" => &[Capability::Read, Capability::Create, Capability::Update],
        // Compiling writes artifacts, removes stale ones, and runs build
        // scripts, proc macros and test binaries — arbitrary code carrying the
        // process's own filesystem access.
        _ => &[
            Capability::Read,
            Capability::Create,
            Capability::Update,
            Capability::Delete,
            Capability::Execute,
        ],
    }
}

/// Name the directory cargo ran in, for failures from a redirected root.
///
/// A redirected root turns an ordinary cargo failure into a baffling one
/// (`package ID specification ... did not match any packages`), because nothing
/// in cargo's own message hints that it ran somewhere other than the workspace.
/// Successes are left alone: the caller asked for the redirect, so it only
/// needs restating when something goes wrong.
fn note_root(outcome: ToolResult, root: &Utf8Path) -> ToolResult {
    let note = format!("(cargo ran in `{root}`, set by the `root` tool option.)");

    match outcome {
        Ok(Outcome::Error {
            message,
            trace,
            transient,
        }) => Ok(Outcome::Error {
            message: format!("{message}\n\n{note}"),
            trace,
            transient,
        }),
        Err(error) => Err(format!("{error}\n\n{note}").into()),
        ok => ok,
    }
}

/// Resolve which cargo workspace the cargo tools operate in.
///
/// `None`, an empty value, or `.` all keep `default`, the root the tool was
/// invoked with.
/// Anything else is a workspace-relative path, resolved under the same
/// confinement every other tool path gets: absolute paths and `..` escapes are
/// refused, and symlinks are canonicalized before the workspace check.
/// An approved `external` mount is the only sanctioned way to reach a checkout
/// outside the workspace.
///
/// The confinement matters more here than for a read: `cargo` runs build
/// scripts and proc macros, so an unvetted directory is arbitrary code
/// execution.
///
/// The target is required to be a directory holding a `Cargo.toml`, so a typo
/// (or naming the manifest instead of the directory holding it) surfaces here.
/// Without the manifest check, cargo would search parent directories and
/// silently operate on the enclosing workspace instead — rewriting sources or
/// the lockfile of a project the caller never named, and reporting success.
///
/// A configured target must also grant `capabilities` outright.
/// `external` only permits a path to resolve outside the workspace; it is not a
/// capability grant, so reaching a mount says nothing about what may be done
/// once there.
fn cargo_root(
    default: &Utf8Path,
    configured: Option<&str>,
    access: Option<&AccessPolicy>,
    capabilities: &[Capability],
) -> Result<Utf8PathBuf, String> {
    // An empty value or `.` resolves to the invocation root, so a config layer
    // can unload a previously-set root without naming the default.
    let configured = configured.filter(|value| !value.is_empty() && *value != ".");

    let Some(configured) = configured else {
        return Ok(default.to_owned());
    };

    let resolved = resolve_workspace_path(default, configured, access)?;

    if !resolved.absolute.is_dir() {
        return Err(format!(
            "The `root` option `{configured}` resolved to `{}`, which is not a directory.",
            resolved.absolute
        ));
    }

    if !resolved.absolute.join("Cargo.toml").is_file() {
        return Err(format!(
            "The `root` option `{configured}` resolved to `{}`, which has no `Cargo.toml`. Cargo \
             would search parent directories and operate on the enclosing workspace instead.",
            resolved.absolute
        ));
    }

    // Only a configured root is authorized. The invocation root is where these
    // tools have always run, so demanding grants for it here would revoke
    // access this option never handed out.
    for capability in capabilities {
        authorize(access, *capability, &resolved.relative)?;
    }

    Ok(resolved.absolute)
}

#[cfg(test)]
#[path = "cargo_tests.rs"]
mod tests;
