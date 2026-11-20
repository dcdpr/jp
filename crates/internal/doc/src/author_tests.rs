use serde_json::json;

use super::*;

/// A configured name that carries its email is written as given, so the result
/// doesn't depend on the git identity of the machine running the test.
#[test]
fn a_configured_name_with_an_email_is_used_as_written() {
    let config = json!({ "user": { "name": "  Jane Doe <jane@example.com>  " } });

    assert_eq!(
        resolve(Utf8Path::new("/"), &config).unwrap(),
        "Jane Doe <jane@example.com>"
    );
}
