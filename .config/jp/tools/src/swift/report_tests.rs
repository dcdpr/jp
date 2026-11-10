use pretty_assertions::assert_eq;
use serde_json::json;

use super::*;

/// The whole point of the check: `xcodebuild` unions the selectors and ignores
/// one that matches nothing, so without this a typo alongside a real name is a
/// passing run that quietly did half the work.
#[test]
fn names_the_requested_tests_that_never_ran() {
    let executed = ["UISuite/ConversationListTests/clickSelects()".to_owned()];
    let requested = [
        "UISuite/ConversationListTests/clickSelects()".to_owned(),
        "UISuite/ConversationListTests/clickSelcts()".to_owned(),
    ];

    assert_eq!(unmatched(&requested, &executed), vec![
        "UISuite/ConversationListTests/clickSelcts()".to_owned()
    ]);
}

/// A suite stands for everything under it, so naming one is answered by any
/// test beneath it rather than by an identifier equal to it.
#[test]
fn a_suite_is_matched_by_the_tests_inside_it() {
    let executed = [
        "UISuite/ConversationListTests/clickSelects()".to_owned(),
        "UISuite/ConversationListTests/labelsRows()".to_owned(),
    ];

    assert!(unmatched(&["UISuite/ConversationListTests".to_owned()], &executed).is_empty());
    assert!(unmatched(&["UISuite".to_owned()], &executed).is_empty());
}

/// A prefix that stops mid-name is not a suite of that name.
/// Matching on the bare string would let `ConversationList` answer for
/// `ConversationListTests`.
#[test]
fn a_partial_name_is_not_a_match() {
    let executed = ["UISuite/ConversationListTests/clickSelects()".to_owned()];

    assert_eq!(
        unmatched(&["UISuite/ConversationList".to_owned()], &executed),
        vec!["UISuite/ConversationList".to_owned()]
    );
}

/// An unreadable bundle cannot answer the question, and answering "none of them
/// ran" would fail a suite that passed.
#[test]
fn nothing_is_unmatched_when_nothing_is_known() {
    let requested = ["UISuite/ConversationListTests/clickSelects()".to_owned()];

    assert!(unmatched(&requested, &[]).is_empty());
}

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
