use camino::{Utf8Path, Utf8PathBuf};
use jp_tool::{AccessPolicy, Capability, Outcome};
use serde_json::Value;

use crate::{
    Context, Tool,
    fs::utils::{authorize, resolve_workspace_path},
    util::{ToolResult, error, unknown_tool},
};

mod check;
mod expand;
mod format;
mod test;
mod update;

use check::cargo_check;
use expand::cargo_expand;
use format::cargo_format;
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

    let outcome = match subcommand {
        "check" => cargo_check(&root, t.opt("package")?, checksum_freshness).await,
        "expand" => {
            cargo_expand(&root, t.req("item")?, t.opt("package")?, checksum_freshness).await
        }
        "test" => {
            cargo_test(
                &root,
                t.opt("package")?,
                t.opt("testname")?,
                t.opt("backtrace")?,
                checksum_freshness,
            )
            .await
        }
        "format" => cargo_format(&root, t.opt("package")?).await,
        "update" => cargo_update(&root, t.req("packages")?).await,
        _ => return unknown_tool(t),
    };

    if root == ctx.root {
        return outcome;
    }

    note_root(outcome, &root)
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
