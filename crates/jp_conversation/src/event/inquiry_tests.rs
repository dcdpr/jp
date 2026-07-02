use serde_json::json;
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
fn test_inquiry_response_answered_serialization() {
    let response = InquiryResponse::boolean("test-id", true);

    let json = serde_json::to_value(&response).unwrap();
    assert_eq!(
        json,
        json!({ "outcome": "answered", "id": "test-id", "answer": true })
    );

    let deserialized: InquiryResponse = serde_json::from_value(json).unwrap();
    assert_eq!(deserialized, response);
}

#[test]
fn test_inquiry_response_legacy_answered_deserialization() {
    // Pre-082 events carry no `outcome` field.
    let json = json!({ "id": "call_1.answer", "answer": true });
    let response: InquiryResponse = serde_json::from_value(json).unwrap();
    assert_eq!(response, InquiryResponse::boolean("call_1.answer", true));
}

#[test]
fn test_inquiry_response_null_answer_round_trips() {
    // A literal `null` answer must stay distinguishable from an absent
    // `answer` field: whatever the serializer produces, the deserializer
    // accepts.
    let response = InquiryResponse::Answered {
        id: InquiryId::new("call_1.confirm.1"),
        answer: Value::Null,
    };

    let json = serde_json::to_value(&response).unwrap();
    assert_eq!(
        json,
        json!({ "outcome": "answered", "id": "call_1.confirm.1", "answer": null })
    );

    let deserialized: InquiryResponse = serde_json::from_value(json).unwrap();
    assert_eq!(deserialized, response);

    // The legacy flat form accepts a null answer the same way.
    let legacy = json!({ "id": "call_1.confirm", "answer": null });
    let deserialized: InquiryResponse = serde_json::from_value(legacy).unwrap();
    assert_eq!(deserialized, InquiryResponse::Answered {
        id: InquiryId::new("call_1.confirm"),
        answer: Value::Null,
    });
}

#[test]
fn test_inquiry_response_cancelled_serialization() {
    let user = InquiryResponse::Cancelled {
        id: InquiryId::new("call_1.confirm.1"),
        reason: CancellationReason::User,
    };
    assert_eq!(
        serde_json::to_value(&user).unwrap(),
        json!({ "outcome": "cancelled", "id": "call_1.confirm.1", "reason": "user" })
    );

    let backend = InquiryResponse::Cancelled {
        id: InquiryId::new("call_1.confirm.1"),
        reason: CancellationReason::BackendError,
    };
    assert_eq!(
        serde_json::to_value(&backend).unwrap(),
        json!({ "outcome": "cancelled", "id": "call_1.confirm.1", "reason": "backend_error" })
    );
}

#[test]
fn test_inquiry_response_cancelled_missing_reason_is_unknown() {
    // A `cancelled` event without a usable `reason` carries no audit claim;
    // it must not be fabricated into a specific reason like `User`.
    let json = json!({ "outcome": "cancelled", "id": "call_1.confirm.1" });
    let response: InquiryResponse = serde_json::from_value(json).unwrap();
    assert_eq!(response, InquiryResponse::Cancelled {
        id: InquiryId::new("call_1.confirm.1"),
        reason: CancellationReason::Unknown("unspecified".to_owned()),
    });

    // A non-string reason is equally unusable and lands on the same sentinel,
    // consistent with unrecognized string tags mapping to `Unknown`.
    let json = json!({ "outcome": "cancelled", "id": "call_1.confirm.1", "reason": 42 });
    let response: InquiryResponse = serde_json::from_value(json).unwrap();
    assert_eq!(response, InquiryResponse::Cancelled {
        id: InquiryId::new("call_1.confirm.1"),
        reason: CancellationReason::Unknown("unspecified".to_owned()),
    });
}

#[test]
fn test_inquiry_response_unknown_reason_round_trips() {
    let json = json!({
        "outcome": "cancelled",
        "id": "call_1.confirm.1",
        "reason": "some_future_variant",
    });
    let response: InquiryResponse = serde_json::from_value(json.clone()).unwrap();
    assert_eq!(response, InquiryResponse::Cancelled {
        id: InquiryId::new("call_1.confirm.1"),
        reason: CancellationReason::Unknown("some_future_variant".to_owned()),
    });

    // The unknown tag survives a re-serialize verbatim.
    assert_eq!(serde_json::to_value(&response).unwrap(), json);
}

#[test]
fn test_inquiry_response_redacted_serialization() {
    let response = InquiryResponse::Redacted {
        id: InquiryId::new("call_1.passphrase.1"),
    };

    let json = serde_json::to_value(&response).unwrap();
    assert_eq!(
        json,
        json!({ "outcome": "redacted", "id": "call_1.passphrase.1" })
    );
    assert!(json.get("answer").is_none());

    let deserialized: InquiryResponse = serde_json::from_value(json).unwrap();
    assert_eq!(deserialized, response);
    assert_eq!(deserialized.answer(), None);
}

#[test]
fn test_inquiry_response_invalid_shape_is_error() {
    // No `outcome`, no `answer` — not interpretable as any variant.
    let json = json!({ "id": "call_1.confirm" });
    let result: Result<InquiryResponse, _> = serde_json::from_value(json);
    assert!(result.is_err());
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
fn test_inquiry_response_answer_accessor() {
    let answered = InquiryResponse::select("id", 42);
    assert_eq!(answered.answer(), Some(&json!(42)));

    let cancelled = InquiryResponse::Cancelled {
        id: InquiryId::new("id"),
        reason: CancellationReason::User,
    };
    assert_eq!(cancelled.answer(), None);

    let redacted = InquiryResponse::Redacted {
        id: InquiryId::new("id"),
    };
    assert_eq!(redacted.answer(), None);
}
