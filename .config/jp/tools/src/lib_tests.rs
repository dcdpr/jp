use super::*;

#[test]
fn test_to_xml_with_root() {
    #[derive(serde::Serialize)]
    struct Data {
        foo: String,
        baz: Vec<u64>,
    }

    let value = Data {
        foo: "bar".to_owned(),
        baz: vec![1, 2, 3],
    };

    assert_eq!(to_xml(value).unwrap(), indoc::indoc! {"
            ```xml
            <Data>
              <foo>bar</foo>
              <baz>1</baz>
              <baz>2</baz>
              <baz>3</baz>
            </Data>
            ```"});
}

#[test]
fn opt_treats_null_as_absent() {
    let tool = Tool {
        name: "git_log".into(),
        arguments: serde_json::from_value(serde_json::json!({
            "query": null,
            "count": 5,
        }))
        .unwrap(),
        answers: Map::new(),
        options: Map::new(),
    };

    // `null` for an optional string argument should be treated as absent,
    // not as a type error. Some LLMs emit explicit nulls for unset params.
    let query: Option<String> = tool.opt("query").unwrap();
    assert_eq!(query, None);

    let count: Option<usize> = tool.opt("count").unwrap();
    assert_eq!(count, Some(5));

    let missing: Option<String> = tool.opt("missing").unwrap();
    assert_eq!(missing, None);
}

#[test]
fn test_to_xml_without_root() {
    let value = serde_json::json!({
        "foo": "bar",
        "baz": [1, 2, 3],
    });

    assert_eq!(to_xml(value).unwrap(), indoc::indoc! {"
            ```xml
            <result>
              <foo>bar</foo>
              <baz>1</baz>
              <baz>2</baz>
              <baz>3</baz>
            </result>
            ```"});
}
