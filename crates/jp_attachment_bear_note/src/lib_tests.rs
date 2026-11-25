use test_log::test;

use super::*;

#[test]
fn test_note_try_to_xml() {
    let note = Note {
        id: 1.to_string(),
        title: "Test Title".to_string(),
        content: "Testing content in XML".to_string(),
        tags: vec!["tag #1".to_string(), "tag #2".to_string()],
    };

    let xml = note.try_to_xml().unwrap();
    assert_eq!(xml, indoc::indoc! {"
            <Note>
              <id>1</id>
              <title>Test Title</title>
              <content>Testing content in XML</content>
              <tags>tag #1</tags>
              <tags>tag #2</tags>
            </Note>"});
}

#[test]
fn test_uri_to_query() {
    let cases = [
        (
            "bear://x-callback-url/open-note?id=123-456",
            Ok(Query::Get("123-456".to_string())),
        ),
        ("bear://get/1", Ok(Query::Get("1".to_string()))),
        (
            "bear:F70FD86D-752F-403D-A517-BF020591F530",
            Ok(Query::Get(
                "F70FD86D-752F-403D-A517-BF020591F530".to_string(),
            )),
        ),
        ("bear:", Err("Invalid bear note query".to_string())),
        // `bear:/NOTE_ID` parses as a hostless URL with a path-absolute,
        // i.e. NOT a `cannot_be_a_base` URL. Reject it: the documented
        // shorthand is the truly opaque `bear:NOTE_ID`.
        (
            "bear:/F70FD86D-752F-403D-A517-BF020591F530",
            Err("Invalid bear note query".to_string()),
        ),
        (
            "bear://get/tag%20%231",
            Ok(Query::Get("tag #1".to_string())),
        ),
        (
            "bear://search/tag%20%231",
            Ok(Query::Search {
                query: "tag #1".to_string(),
                tags: vec![],
                exclude_archived: false,
            }),
        ),
        (
            "bear://search/tag%20%231?tag=tag%20%232",
            Ok(Query::Search {
                query: "tag #1".to_string(),
                tags: vec!["tag #2".to_string()],
                exclude_archived: false,
            }),
        ),
        (
            "bear://search/tag%20%231?tag=tag%20%232&tag=tag%20%233",
            Ok(Query::Search {
                query: "tag #1".to_string(),
                tags: vec!["tag #2".to_string(), "tag #3".to_string()],
                exclude_archived: false,
            }),
        ),
        (
            "bear://search/?tag=foo&exclude_archived=true",
            Ok(Query::Search {
                query: String::new(),
                tags: vec!["foo".to_string()],
                exclude_archived: true,
            }),
        ),
        (
            "bear://search/?tag=foo&exclude_archived=false",
            Ok(Query::Search {
                query: String::new(),
                tags: vec!["foo".to_string()],
                exclude_archived: false,
            }),
        ),
        (
            "bear://invalid/foo",
            Err("Invalid bear note query".to_string()),
        ),
    ];

    for (uri, expected) in cases {
        let uri = Url::parse(uri).unwrap();
        let query = uri_to_query(&uri).map_err(|e| e.to_string());
        assert_eq!(query, expected);
    }
}

#[test]
fn test_exclude_archived_round_trips() {
    let handler = BearNotes::default();
    let query = Query::Search {
        query: String::new(),
        tags: vec!["rfd/D46/review".to_string()],
        exclude_archived: true,
    };

    let uri = handler.query_to_uri(&query).unwrap();
    assert_eq!(uri_to_query(&uri).unwrap(), query);
}

fn canonical(uri: &str) -> Result<String, String> {
    BearNotes::default()
        .canonical(&Url::parse(uri).unwrap())
        .map(|url| url.to_string())
        .map_err(|error| error.to_string())
}

/// The shorthand and Bear's own "Copy Link" URL name the same note.
#[test]
fn test_canonical_collapses_every_spelling_of_one_note() {
    for spelling in [
        "bear:F70FD86D",
        "bear://get/F70FD86D",
        "bear://x-callback-url/open-note?id=F70FD86D",
    ] {
        assert_eq!(
            canonical(spelling).as_deref(),
            Ok("bear://get/F70FD86D"),
            "{spelling}"
        );
    }
}

/// A note's tags are a set; the order they are filtered by changes nothing.
#[test]
fn test_canonical_lists_each_tag_once_in_order() {
    assert_eq!(
        canonical("bear://search/rfd?tag=b&tag=a&tag=b&exclude_archived=1").as_deref(),
        Ok("bear://search/rfd?tag=a&tag=b&exclude_archived=true")
    );
}

#[test]
fn test_canonical_rejects_a_url_add_rejects() {
    assert_eq!(
        canonical("bear:"),
        Err("Invalid bear note query".to_owned())
    );
}
