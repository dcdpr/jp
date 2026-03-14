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
