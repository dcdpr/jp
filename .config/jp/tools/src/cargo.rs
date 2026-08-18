use camino::{Utf8Path, Utf8PathBuf};

use crate::{
    Context, Tool,
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
    let configured: Option<String> = t.option_or("root", None);
    let root = match cargo_root(&ctx.root, configured.as_deref()) {
        Ok(root) => root,
        Err(message) => return error(message),
    };

    match t.name.trim_start_matches("cargo_") {
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
        _ => unknown_tool(t),
    }
}

/// Resolve which cargo workspace the cargo tools operate in.
///
/// `None` keeps `default`, the root the tool was invoked with.
/// A relative path is joined onto that root; an absolute path is taken as-is,
/// so a checkout outside the workspace can be targeted deliberately.
///
/// The directory is required to exist, so a typo surfaces here rather than as a
/// confusing cargo failure in the wrong place.
fn cargo_root(default: &Utf8Path, configured: Option<&str>) -> Result<Utf8PathBuf, String> {
    // An empty value or `.` resolves to the invocation root, so a config layer
    // can unload a previously-set root without naming the default.
    let configured = configured.filter(|value| !value.is_empty() && *value != ".");

    let Some(configured) = configured else {
        return Ok(default.to_owned());
    };

    let path = Utf8Path::new(configured);
    let root = if path.is_absolute() {
        path.to_owned()
    } else {
        default.join(path)
    };

    if !root.is_dir() {
        return Err(format!(
            "The `root` option `{configured}` resolved to `{root}`, which is not a directory."
        ));
    }

    Ok(root)
}

#[cfg(test)]
#[path = "cargo_tests.rs"]
mod tests;
