use duct::cmd;

use crate::{Result, Workspace};

pub(crate) async fn cargo_expand(
    workspace: &Workspace,
    item: String,
    package: Option<String>,
) -> Result<String> {
    let package = package
        .map(|v| format!("--package={v}"))
        .unwrap_or_default();

    let result = cmd!("cargo", "expand", "--color=never", package, item)
        .dir(&workspace.path)
        .env("RUST_BACKTRACE", "1")
        .unchecked()
        .run()?;

    if !result.status.success() {
        return Err(format!(
            "Cargo command failed ({}): {}",
            result.status.code().unwrap_or(1),
            String::from_utf8_lossy(&result.stderr)
        )
        .into());
    }

    let content = String::from_utf8_lossy(&result.stdout);

    Ok(indoc::formatdoc! {"
        ```rust
        {content}
        ```
    "})
}
