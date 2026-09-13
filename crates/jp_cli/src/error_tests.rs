use test_log::test;

use super::*;

#[derive(Debug, thiserror::Error)]
#[error("category")]
struct Category(#[source] Detail);

#[derive(Debug, thiserror::Error)]
#[error("the actual reason")]
struct Detail;

/// A wrapper that interpolates its source into its own `Display`.
#[derive(Debug, thiserror::Error)]
#[error("some.key: {0}")]
struct Interpolating(#[source] Category);

#[test]
fn test_error_chain_appends_sources() {
    // A category-only error is useless on its own; the reason lives one
    // level down.
    assert_eq!(
        error_chain(&Category(Detail)),
        "category: the actual reason"
    );
    assert_eq!(error_chain(&Detail), "the actual reason");
}

#[test]
fn test_error_chain_skips_already_interpolated_sources() {
    // The outer error already ends with its source's message, so repeating
    // it would read `some.key: category: category: the actual reason`.
    assert_eq!(
        error_chain(&Interpolating(Category(Detail))),
        "some.key: category: the actual reason"
    );
}
