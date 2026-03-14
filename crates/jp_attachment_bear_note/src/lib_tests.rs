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
            "bear://get/tag%20%231",
            Ok(Query::Get("tag #1".to_string())),
        ),
        (
            "bear://search/tag%20%231",
            Ok(Query::Search {
                query: "tag #1".to_string(),
                tags: vec![],
            }),
        ),
        (
            "bear://search/tag%20%231?tag=tag%20%232",
            Ok(Query::Search {
                query: "tag #1".to_string(),
                tags: vec!["tag #2".to_string()],
            }),
        ),
        (
            "bear://search/tag%20%231?tag=tag%20%232&tag=tag%20%233",
            Ok(Query::Search {
                query: "tag #1".to_string(),
                tags: vec!["tag #2".to_string(), "tag #3".to_string()],
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
