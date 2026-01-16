use duct::cmd;
use jp_tool::Context;

use crate::util::{ToolResult, error};

pub(crate) async fn cargo_check(ctx: &Context, package: Option<String>) -> ToolResult {
    let package = package.map_or("--workspace".to_owned(), |v| format!("--package={v}"));
    let result = cmd!(
        "cargo",
        "clippy",
        "--color=never",
        &package,
        "--quiet",
        "--all-targets"
    )
    // Prevent warnings from being treated as errors, e.g. on CI.
    .env("RUSTFLAGS", "-W warnings")
    .stdout_capture()
    .stderr_capture()
    .dir(&ctx.root)
    .unchecked()
    .run()?;

    let code = result.status.code().unwrap_or(0);
    if code != 0 {
        return error(format!(
            "Cargo command failed ({}): {}",
            result.status.code().unwrap_or(1),
            String::from_utf8_lossy(&result.stderr)
        ));
    }

    let content = String::from_utf8_lossy(&result.stderr);

    // Strip ANSI escape codes
    let content = strip_ansi_escapes::strip_str(&content);
    let content = content.trim();

    if content.is_empty() {
        Ok("Check succeeded. No warnings or errors found.".into())
    } else {
        Ok(indoc::formatdoc! {"
        ```
        {content}
        ```
    "}
        .into())
    }
}

#[cfg(test)]
mod tests {
    use jp_tool::Action;
    use pretty_assertions::assert_eq;

    use super::*;

    #[tokio::test]
    #[test_log::test]
    async fn test_cargo_check() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = Context {
            root: dir.path().to_owned(),
            action: Action::Run,
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

        let result = cargo_check(&ctx, None).await.unwrap();

        assert_eq!(result.into_content().unwrap(), indoc::indoc! {r#"
            ```
            warning: unused `std::result::Result` that must be used
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
