use duct::cmd;

use crate::{Result, Workspace};

pub(crate) async fn cargo_check(workspace: &Workspace, package: Option<String>) -> Result<String> {
    let package = package.map_or("--workspace".to_owned(), |v| format!("--package={v}"));
    let result = cmd!("cargo", "check", "--color=never", &package, "--quiet")
        // Prevent warnings from being treated as errors, e.g. on CI.
        .env("RUSTFLAGS", "-W warnings")
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
        {}
        ```
    ", content.trim()})
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use super::*;

    #[tokio::test]
    async fn test_cargo_check() {
        let dir = tempfile::tempdir().unwrap();
        let workspace = Workspace {
            path: dir.path().to_owned(),
        };

        std::fs::write(dir.path().join("Cargo.toml"), indoc::indoc! {r#"
            [package]
            name = "cargo_check"
        "#})
        .unwrap();

        std::fs::create_dir_all(dir.path().join("src")).unwrap();
        std::fs::write(dir.path().join("src/main.rs"), indoc::indoc! {r#"
            fn main() {
                std::env::var("FOO");
            }
        "#})
        .unwrap();

        let result = cargo_check(&workspace, None).await.unwrap();

        assert_eq!(result, indoc::indoc! {r#"
            ```
            warning: unused `Result` that must be used
             --> src/main.rs:2:5
              |
            2 |     std::env::var("FOO");
              |     ^^^^^^^^^^^^^^^^^^^^
              |
              = note: this `Result` may be an `Err` variant, which should be handled
              = note: `#[warn(unused_must_use)]` (part of `#[warn(unused)]`) on by default
            help: use `let _ = ...` to ignore the resulting value
              |
            2 |     let _ = std::env::var("FOO");
              |     +++++++
            ```
        "#});
    }
}
