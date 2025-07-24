use duct::cmd;

use crate::{Result, Workspace};

pub(crate) async fn cargo_check(workspace: &Workspace, package: Option<String>) -> Result<String> {
    let package = package.map_or("--workspace".to_owned(), |v| format!("--package={v}"));
    let result = cmd!("cargo", "check", &package, "--quiet")
        .stdout_capture()
        .stderr_capture()
        .dir(&workspace.path)
        .unchecked()
        .run()?;

    let code = result.status.code().unwrap_or(0);
    if code != 0 && code != 101 {
        return Err(format!(
            "Cargo command failed ({}): {}",
            result.status.code().unwrap_or(1),
            String::from_utf8_lossy(&result.stderr)
        )
        .into());
    }

    let content = String::from_utf8_lossy(&result.stderr);
    Ok(indoc::formatdoc! {"
        ```
        {content}
        ```
    "})
}
