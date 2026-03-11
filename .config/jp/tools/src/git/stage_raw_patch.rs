use jp_tool::Context;

use crate::util::{
    ToolResult,
    runner::{DuctProcessRunner, ProcessRunner},
};

pub(crate) async fn git_stage_raw_patch(ctx: Context, path: String, diff: String) -> ToolResult {
    git_stage_raw_patch_impl(&ctx, &path, &diff, &DuctProcessRunner)
}

fn git_stage_raw_patch_impl<R: ProcessRunner>(
    ctx: &Context,
    path: &str,
    diff: &str,
    runner: &R,
) -> ToolResult {
    let patch = build_patch(path, diff);

    if ctx.action.is_format_arguments() {
        return Ok(patch.into());
    }

    let output = runner.run_with_env_and_stdin(
        "git",
        &["apply", "--cached", "--unidiff-zero", "-"],
        &ctx.root,
        &[],
        Some(&patch),
    )?;

    if !output.success() {
        return Err(format!("Failed to apply patch: {}", output.stderr).into());
    }

    Ok("Patch applied.".into())
}

fn build_patch(path: &str, diff: &str) -> String {
    indoc::formatdoc! {"
        diff --git a/{path} b/{path}
        --- a/{path}
        +++ b/{path}
        {diff}
    "}
}

#[cfg(test)]
mod tests {
    use camino_tempfile::tempdir;
    use jp_tool::Action;

    use super::*;
    use crate::util::runner::MockProcessRunner;

    fn ctx(root: &camino::Utf8Path) -> Context {
        Context {
            root: root.to_owned(),
            action: Action::Run,
        }
    }

    #[test]
    fn test_stage_raw_patch_success() {
        let dir = tempdir().unwrap();
        let ctx = ctx(dir.path());

        let diff = "@@ -5 +5 @@\n-old line\n+new line\n";

        let runner = MockProcessRunner::builder()
            .expect("git")
            .args(&["apply", "--cached", "--unidiff-zero", "-"])
            .returns_success("");

        let content = git_stage_raw_patch_impl(&ctx, "src/lib.rs", diff, &runner)
            .unwrap()
            .into_content()
            .unwrap();

        assert_eq!(content, "Patch applied.");
    }

    #[test]
    fn test_stage_raw_patch_apply_failure() {
        let dir = tempdir().unwrap();
        let ctx = ctx(dir.path());

        let runner = MockProcessRunner::builder()
            .expect("git")
            .args(&["apply", "--cached", "--unidiff-zero", "-"])
            .returns_error("error: patch failed");

        let err = git_stage_raw_patch_impl(&ctx, "src/lib.rs", "@@ -1 +1 @@\n-a\n+b\n", &runner)
            .unwrap_err();

        assert!(err.to_string().contains("Failed to apply patch"));
    }

    #[test]
    fn test_stage_raw_patch_format_arguments() {
        let dir = tempdir().unwrap();
        let ctx = Context {
            root: dir.path().to_owned(),
            action: Action::FormatArguments,
        };

        let runner = MockProcessRunner::never_called();

        let content =
            git_stage_raw_patch_impl(&ctx, "src/lib.rs", "@@ -5 +5 @@\n-old\n+new\n", &runner)
                .unwrap()
                .into_content()
                .unwrap();

        assert!(content.contains("diff --git a/src/lib.rs b/src/lib.rs"));
        assert!(content.contains("--- a/src/lib.rs"));
        assert!(content.contains("+++ b/src/lib.rs"));
        assert!(content.contains("@@ -5 +5 @@"));
    }

    #[test]
    fn test_build_patch() {
        let patch = build_patch("src/lib.rs", "@@ -1 +1 @@\n-old\n+new");

        assert_eq!(patch, indoc::indoc! {"
            diff --git a/src/lib.rs b/src/lib.rs
            --- a/src/lib.rs
            +++ b/src/lib.rs
            @@ -1 +1 @@
            -old
            +new
        "});
    }
}
