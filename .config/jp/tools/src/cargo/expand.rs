use jp_tool::Context;

use crate::util::{
    ToolResult,
    runner::{DuctProcessRunner, ProcessOutput, ProcessRunner},
};

pub(crate) async fn cargo_expand(
    ctx: &Context,
    item: String,
    package: Option<String>,
) -> ToolResult {
    cargo_expand_impl(ctx, &item, package, &DuctProcessRunner)
}

fn cargo_expand_impl<R: ProcessRunner>(
    ctx: &Context,
    item: &str,
    package: Option<String>,
    runner: &R,
) -> ToolResult {
    let package = package.map(|v| format!("--package={v}"));
    let mut args = vec!["--quiet", "expand", "--color=never"];
    if let Some(package) = package.as_deref() {
        args.push(package);
    }
    args.push(item);

    let ProcessOutput {
        stdout,
        stderr,
        status,
    } = runner.run_with_env("cargo", &args, &ctx.root, &[("RUST_BACKTRACE", "1")])?;

    if !status.is_success() {
        return Err(format!("Cargo command failed: {stderr}").into());
    }

    Ok(format!("```rust\n{}\n```\n", stdout.trim()).into())
}

#[cfg(test)]
mod tests {
    use camino_tempfile::tempdir;
    use jp_tool::Action;
    use pretty_assertions::assert_eq;

    use super::*;
    use crate::util::runner::MockProcessRunner;

    #[test]
    fn test_cargo_expand_success() {
        let dir = tempdir().unwrap();
        let ctx = Context {
            root: dir.path().to_owned(),
            action: Action::Run,
        };

        let stdout = indoc::indoc! { r#"
            fn main() {
                {
                    ::std::io::_print(format_args!("hello world\n"));
                };
            }"#};

        let runner = MockProcessRunner::success(stdout);

        let result = cargo_expand_impl(&ctx, "main", None, &runner).unwrap();

        assert_eq!(result.into_content().unwrap(), indoc::indoc! {r#"
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
