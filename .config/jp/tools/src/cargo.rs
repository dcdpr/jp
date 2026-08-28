use jp_tool::Capability;
use serde_json::Value;

use crate::{
    Context, Tool,
    util::{
        ToolResult, error,
        root::{CARGO_MANIFEST, configured_root, note_root, resolve_root},
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
