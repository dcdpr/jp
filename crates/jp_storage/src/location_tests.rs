use camino::Utf8Path;

use super::*;

// A file is known by its name in messages, and by its full path everywhere
// else.
#[test]
fn a_path_is_labelled_by_its_file_name() {
    let location = StorageLocation::of_path(Utf8Path::new("/ws/.jp/conversations/1/metadata.json"));

    assert_eq!(location.label(), "metadata.json");
    assert_eq!(location.as_str(), "/ws/.jp/conversations/1/metadata.json");
    assert_eq!(
        location.to_string(),
        "/ws/.jp/conversations/1/metadata.json"
    );
}

#[test]
fn an_unlabelled_location_is_labelled_by_its_name() {
    assert_eq!(StorageLocation::new("row 7").label(), "row 7");
}

// The label is how a message refers to the location, not part of which
// location it is.
#[test]
fn locations_with_the_same_name_are_equal() {
    assert_eq!(
        StorageLocation::labelled("/ws/metadata.json", "metadata.json"),
        StorageLocation::new("/ws/metadata.json")
    );
}

#[test]
fn json_holds_the_name_only() {
    let location = StorageLocation::labelled("/ws/metadata.json", "metadata.json");

    assert_eq!(
        serde_json::to_string(&location).unwrap(),
        r#""/ws/metadata.json""#
    );
}
