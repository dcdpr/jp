use duct::cmd;

use crate::{Result, Workspace};

pub(crate) async fn cargo_expand(
    workspace: &Workspace,
    item: String,
    package: Option<String>,
) -> Result<String> {
    let package = package.map(|v| format!("--package={v}"));
    let mut args = vec!["--quiet", "expand", "--color=never"];
    if let Some(package) = package.as_deref() {
        args.push(package);
    }
    args.push(&item);

    let result = cmd("cargo", &args)
        .stdout_capture()
        .stderr_capture()
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
        {}
        ```
    ", content.trim()})
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use super::*;

    #[tokio::test]
    async fn test_cargo_expand() {
        let dir = tempfile::tempdir().unwrap();
        let workspace = Workspace {
            path: dir.path().to_owned(),
        };

        std::fs::write(dir.path().join("Cargo.toml"), indoc::indoc! {r#"
            [package]
            name = "cargo_expand"
        "#})
        .unwrap();

        std::fs::create_dir_all(dir.path().join("src")).unwrap();
        std::fs::write(dir.path().join("src/main.rs"), indoc::indoc! {r#"
            fn main() {
                println!("hello world");
            }
        "#})
        .unwrap();

        let result = cargo_expand(&workspace, "main".into(), None).await.unwrap();

        assert_eq!(result, indoc::indoc! {r#"
            ```rust
            fn main() {
                {
                    ::std::io::_print(format_args!("hello world\n"));
                };
            }
            ```
        "#});
    }
}
