use camino::Utf8Path;
use jp_tool::Capability;
use serde_json::Value;

use crate::{
    Context, Tool,
    util::{
        ToolResult, error,
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

    let outcome = match subcommand {
        "check" => {
            cargo_check(
                &root,
                profile.as_deref(),
                t.opt("package")?,
                checksum_freshness,
            )
            .await
        }
        "expand" => {
            cargo_expand(
                &root,
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
                profile.as_deref(),
                t.opt("package")?,
                t.opt("testname")?,
                t.opt("backtrace")?,
                checksum_freshness,
            )
            .await
        }
        "format" => cargo_format(&root, t.opt("package")?).await,
        "install_tools" => cargo_install_tools(&root).await,
        "update" => cargo_update(&root, t.req("packages")?).await,
        _ => return unknown_tool(t),
    };

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
