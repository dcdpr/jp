use test_log::test;

use super::*;

#[test]
fn test_inquiry_id_serialization() {
    let id = InquiryId::new("test-id");
    let json = serde_json::to_value(&id).unwrap();
    assert_eq!(json, "test-id"); // transparent serialization

    let deserialized: InquiryId = serde_json::from_value(json).unwrap();
    assert_eq!(deserialized, id);
}

#[test]
fn test_inquiry_request_serialization() {
    let request = InquiryRequest::new(
        "test-id",
        InquirySource::tool("file_editor"),
        InquiryQuestion::boolean("Do you want to proceed?".to_string())
            .with_default(Value::Bool(false)),
    );

    let json = serde_json::to_value(&request).unwrap();
    assert_eq!(json["id"], "test-id");
    assert_eq!(json["source"]["source"], "tool");
    assert_eq!(json["source"]["name"], "file_editor");
    assert_eq!(json["question"]["text"], "Do you want to proceed?");
    assert_eq!(json["question"]["answer_type"]["type"], "boolean");
    assert_eq!(json["question"]["default"], false);

    let deserialized: InquiryRequest = serde_json::from_value(json).unwrap();
    assert_eq!(deserialized, request);
}

#[test]
fn test_inquiry_response_serialization() {
    let response = InquiryResponse::boolean("test-id", true);

    let json = serde_json::to_value(&response).unwrap();
    assert_eq!(json["id"], "test-id");
    assert_eq!(json["answer"], true);

    let deserialized: InquiryResponse = serde_json::from_value(json).unwrap();
    assert_eq!(deserialized, response);
}

#[test]
fn test_inquiry_question_types() {
    let q = InquiryQuestion::boolean("Confirm?".to_string());
    assert!(matches!(q.answer_type, InquiryAnswerType::Boolean));

    let q = InquiryQuestion::select("Choose one:".to_string(), vec![
        SelectOption::new("y", "yes"),
        SelectOption::new("n", "no"),
    ]);
    if let InquiryAnswerType::Select { options } = &q.answer_type {
        assert_eq!(options.len(), 2);
        assert_eq!(options[0].value, "y");
        assert_eq!(options[1].value, "n");
        assert_eq!(options[0].description.as_deref(), Some("yes"));
        assert_eq!(options[1].description.as_deref(), Some("no"));
    } else {
        panic!("Expected Select variant");
    }

    let q = InquiryQuestion::select_values("Pick:".to_string(), vec![
        Value::Number(1.into()),
        Value::Number(2.into()),
    ]);
    if let InquiryAnswerType::Select { options } = &q.answer_type {
        assert_eq!(options.len(), 2);
        assert_eq!(options[0].value, 1);
        assert_eq!(options[1].value, 2);
        assert!(options[0].description.is_none());
        assert!(options[1].description.is_none());
    } else {
        panic!("Expected Select variant");
    }

    let q = InquiryQuestion::text("Enter name:".to_string());
    assert!(matches!(q.answer_type, InquiryAnswerType::Text));
}

#[test]
fn test_select_option_serialization() {
    let opt = SelectOption::new("y", "Run tool");
    let json = serde_json::to_value(&opt).unwrap();
    assert_eq!(json["value"], "y");
    assert_eq!(json["description"], "Run tool");

    let opt_no_desc = SelectOption::from("n");
    let json = serde_json::to_value(&opt_no_desc).unwrap();
    assert_eq!(json["value"], "n");
    assert!(json.get("description").is_none());

    let deserialized: SelectOption = serde_json::from_value(json).unwrap();
    assert_eq!(deserialized, opt_no_desc);
}

#[test]
fn test_inquiry_response_helpers() {
    let response = InquiryResponse::boolean("id", true);
    assert_eq!(response.as_bool(), Some(true));
    assert_eq!(response.as_str(), None);

    let response = InquiryResponse::text("id", "hello".to_string());
    assert_eq!(response.as_str(), Some("hello"));

    let response = InquiryResponse::select("id", "option1");
    assert_eq!(response.as_str(), Some("option1"));

    let response = InquiryResponse::select("id", 42);
    assert_eq!(response.answer, 42);
}
