use pretty_assertions::assert_eq;
use serde_json::json;

use super::*;

/// A result bundle's test tree, in the shape `xcresulttool get test-results
/// tests` documents.
///
/// The schema is published by `xcresulttool get test-results tests --help`:
/// `testNodes` holds a `TestNode` tree, `nodeType` is a closed enum including
/// `Test Plan`, `UI test bundle`, `Test Suite` and `Test Case`, and a test
/// case's `nodeIdentifier` is its suite path and function without the bundle.
///
/// Nested suites appear as nested `Test Suite` nodes, which is how a
/// swift-testing suite inside another one arrives.
fn document() -> Value {
    json!({
        "devices": [{ "deviceName": "My Mac" }],
        "testPlanConfigurations": [{ "configurationId": "1" }],
        "testNodes": [{
            "name": "JP",
            "nodeType": "Test Plan",
            "children": [{
                "name": "JPUITests",
                "nodeType": "UI test bundle",
                "children": [{
                    "name": "UISuite",
                    "nodeType": "Test Suite",
                    "children": [{
                        "name": "ConversationListTests",
                        "nodeType": "Test Suite",
                        "children": [
                            {
                                "name": "clickSelects()",
                                "nodeIdentifier": "UISuite/ConversationListTests/clickSelects()",
                                "nodeType": "Test Case",
                                "result": "Passed"
                            },
                            {
                                "name": "labelsRows()",
                                "nodeIdentifier": "UISuite/ConversationListTests/labelsRows()",
                                "nodeType": "Test Case",
                                "result": "Passed"
                            }
                        ]
                    }]
                }]
            }]
        }]
    })
}

/// The identifiers are what the caller names a test with, so they can be
/// compared against the requested selectors without reshaping either side.
#[test]
fn reads_the_tests_that_ran() {
    assert_eq!(executed_tests(&document()), vec![
        "UISuite/ConversationListTests/clickSelects()".to_owned(),
        "UISuite/ConversationListTests/labelsRows()".to_owned(),
    ]);
}

/// Only `Test Case` nodes name a test that ran.
/// A suite carries a name too, and counting it would let a suite that matched
/// nothing look as though it had.
#[test]
fn ignores_every_node_that_is_not_a_test_case() {
    let document = json!({
        "testNodes": [{
            "name": "UISuite",
            "nodeIdentifier": "UISuite",
            "nodeType": "Test Suite",
            "children": [{
                "name": "Failure Message",
                "nodeIdentifier": "not-a-test",
                "nodeType": "Failure Message"
            }]
        }]
    });

    assert!(executed_tests(&document).is_empty());
}

/// `nodeIdentifier` is optional in the schema.
/// A test case without one cannot be matched against a request, and the caller
/// reads an empty list as "cannot tell" rather than "nothing ran".
#[test]
fn skips_a_test_case_with_no_identifier() {
    let document = json!({
        "testNodes": [{
            "name": "clickSelects()",
            "nodeType": "Test Case",
            "result": "Passed"
        }]
    });

    assert!(executed_tests(&document).is_empty());
}

#[test]
fn reads_nothing_from_a_document_with_no_tests() {
    assert!(executed_tests(&json!({ "testNodes": [] })).is_empty());
}
