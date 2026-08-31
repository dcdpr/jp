use std::time::{Duration, Instant};

use camino::Utf8Path;
use jp_tool::{Capability, Outcome};
use serde_json::Value;

use crate::{
    Context, Tool,
    util::{
        OneOrMany, ToolResult, error,
        root::{CARGO_MANIFEST, configured_root, note_root, resolve_root},
        runner::{DuctProcessRunner, ProcessOutput, ProcessRunner},
        unknown_tool,
    },
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
    let configured = match configured_root(&t.options) {
        Ok(configured) => configured,
        Err(message) => return error(message),
    };

    let subcommand = t.name.trim_start_matches("cargo_");
    let root = match resolve_root(
        &ctx.root,
        configured,
        ctx.access.as_ref(),
        required_capabilities(subcommand),
        &CARGO_MANIFEST,
    ) {
        Ok(root) => root,
        Err(message) => return error(message),
    };

    if root != ctx.root
        && let Err(message) = ensure_workspace_root(&root, &DuctProcessRunner)
    {
        return error(message);
    }

    // Cargo profile to build under, set via `options.profile`. A profile of its
    // own gives these tools their own directory under `target/`, which is the
    // only way to keep their artifacts, fingerprints and build lock apart from
    // a developer's concurrent `cargo run` in the same workspace.
    //
    // Refused when malformed rather than ignored: a silently dropped value puts
    // the build back in the shared profile without saying so.
    let profile = match t.options.get("profile") {
        None | Some(Value::Null) => None,
        Some(Value::String(value)) => Some(value.clone()),
        Some(other) => {
            return error(format!(
                "The `profile` tool option must be a string, got `{other}`."
            ));
        }
    };

    // `format` and `update` never compile, and `install_tools` builds through a
    // recipe that pins its own profile. Accepting the option for them would
    // promise a target directory of its own and hand back the shared one, which
    // is the outcome the option exists to avoid.
    if profile.is_some() && !matches!(subcommand, "check" | "expand" | "test") {
        return error(format!(
            "The `profile` tool option has no effect on `cargo_{subcommand}`."
        ));
    }

    // Flags to append to `RUSTFLAGS`, set via `options.rustflags`. Malformed
    // values are refused for the same reason as `root`: silently ignoring them
    // would compile with flags the caller believes are in effect.
    let configured_flags = match t.options.get("rustflags") {
        None | Some(Value::Null) => None,
        Some(value) => match serde_json::from_value::<OneOrMany<String>>(value.clone()) {
            Ok(flags) => Some(flags.into_vec()),
            Err(_) => {
                return error(format!(
                    "The `rustflags` tool option must be a string or an array of strings, got \
                     `{value}`."
                ));
            }
        },
    };

    // `format` and `update` never invoke rustc, and `install_tools` builds
    // through a recipe that owns its flags. Accepting the option for them would
    // report success over a build the flags never reached — for `install_tools`
    // that build installs a binary, which then serves later calls.
    if configured_flags.is_some() && !matches!(subcommand, "check" | "expand" | "test") {
        return error(format!(
            "The `rustflags` tool option has no effect on `cargo_{subcommand}`."
        ));
    }

    let rustflags = rustflags(configured_flags.as_deref().unwrap_or_default());

    let started = Instant::now();
    let outcome = match subcommand {
        "check" => {
            cargo_check(
                &root,
                &rustflags,
                profile.as_deref(),
                t.opt("package")?,
                checksum_freshness,
            )
            .await
        }
        "expand" => {
            cargo_expand(
                &root,
                &rustflags,
                profile.as_deref(),
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
                profile.as_deref(),
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

    note_root(outcome, &root, "cargo")
}

/// Refuse a redirected root that cargo treats as a member of a larger
/// workspace.
///
/// The manifest check that precedes this one only proves a `Cargo.toml` is
/// present, and a member has one too.
/// Cargo resolves the workspace from that manifest, so a member root puts
/// `cargo check` and `cargo test` on `--workspace`, `cargo fmt` on `--all`, and
/// `cargo update` on the enclosing lockfile — reaching a project the caller
/// never named and the access policy never covered.
///
/// # Errors
///
/// Returns an error naming both directories when they differ, and when cargo
/// cannot resolve the workspace at all.
fn ensure_workspace_root<R: ProcessRunner>(root: &Utf8Path, runner: &R) -> Result<(), String> {
    // Cargo is asked rather than the manifest parsed, so this answer cannot
    // drift from the one the build commands themselves will resolve.
    let ProcessOutput {
        stdout,
        stderr,
        status,
    } = runner
        .run(
            "cargo",
            &[
                "locate-project",
                "--workspace",
                "--message-format=plain",
                "--manifest-path",
                root.join("Cargo.toml").as_str(),
            ],
            root,
        )
        .map_err(|error| format!("Could not resolve the cargo workspace of `{root}`: {error}"))?;

    if !status.is_success() {
        return Err(format!(
            "The `root` option resolved to `{root}`, whose cargo workspace could not be resolved: \
             {}",
            stderr.trim()
        ));
    }

    let located = Utf8Path::new(stdout.trim());
    let workspace = located.parent().unwrap_or(located);
    // `root` arrives canonicalized, so the comparison only holds if this side is
    // too. A path cargo reports for a directory that has since gone is left as
    // printed, and fails the comparison below.
    let workspace = workspace
        .canonicalize_utf8()
        .unwrap_or_else(|_| workspace.to_owned());

    if workspace == root {
        return Ok(());
    }

    Err(format!(
        "The `root` option resolved to `{root}`, which is a member of the cargo workspace at \
         `{workspace}`. Cargo would operate on that workspace instead. Name the workspace root, \
         and scope the command with the `package` argument."
    ))
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
/// `check`, `expand` and `test` all set the variable, so they agree on the flag
/// set and a shared target directory stays warm when alternating between them.
/// `format` gets the base value alone, which keeps CI's `-D warnings` off the
/// tools it spawns.
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

#[cfg(test)]
#[path = "cargo_tests.rs"]
mod tests;
