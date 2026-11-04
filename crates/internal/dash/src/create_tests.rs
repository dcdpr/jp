use super::*;

#[test]
fn test_directory_name_keeps_plain_branch_names() {
    assert_eq!(
        directory_name("gh-merge-queue").as_deref(),
        Some("gh-merge-queue")
    );
    assert_eq!(
        directory_name("rgrant_dev.vllm").as_deref(),
        Some("rgrant_dev.vllm")
    );
}

#[test]
fn test_directory_name_flattens_separators() {
    assert_eq!(
        directory_name("feature/stacks").as_deref(),
        Some("feature-stacks")
    );
    assert_eq!(
        directory_name("fix: \"quotes\"").as_deref(),
        Some("fix---quotes")
    );
}

#[test]
fn test_directory_name_is_never_hidden_or_empty() {
    assert_eq!(directory_name(".hidden").as_deref(), Some("hidden"));
    assert_eq!(directory_name("/"), None);
    assert_eq!(directory_name(".."), None);
}
