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
