use super::*;

#[test]
fn a_key_is_first_once() {
    let seen = SeenNotices::default();

    assert!(seen.first("skipping api_key: environment variable X is not set"));
    assert!(!seen.first("skipping api_key: environment variable X is not set"));
    assert!(seen.first("skipping api_key: environment variable Y is not set"));
}

/// A provider is cloned into every stream it opens, and a key one stream
/// records has to be seen by the next.
#[test]
fn clones_share_one_record() {
    let seen = SeenNotices::default();
    let clone = seen.clone();

    assert!(seen.first("seven_day@0.75"));
    assert!(!clone.first("seven_day@0.75"));
}
