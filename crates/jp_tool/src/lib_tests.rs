use super::*;

#[test]
fn question_id_rejects_dot() {
    assert_eq!("has.dot".parse::<QuestionId>(), Err(InvalidQuestionId));
    assert_eq!(
        QuestionId::try_from("has.dot".to_owned()),
        Err(InvalidQuestionId)
    );
    assert_eq!(QuestionId::try_from("has.dot"), Err(InvalidQuestionId));
}

#[test]
fn question_id_rejects_empty() {
    assert_eq!("".parse::<QuestionId>(), Err(InvalidQuestionId));
    assert_eq!(QuestionId::try_from(String::new()), Err(InvalidQuestionId));
    assert_eq!(QuestionId::try_from(""), Err(InvalidQuestionId));

    // Rejected at the deserialization boundary too.
    assert!(serde_json::from_str::<QuestionId>("\"\"").is_err());

    // And through every constructor.
    assert!(Question::text("", "?").is_err());
    assert!(Question::boolean("", "?").is_err());
    assert!(Question::select("", "?").is_err());
    assert!(Question::secret("", "?").is_err());
}

#[test]
fn question_id_accepts_plain() {
    let id: QuestionId = "overwrite_file".parse().unwrap();
    assert_eq!(id, "overwrite_file");
    assert_eq!(id.as_str(), "overwrite_file");
    assert_eq!(id.to_string(), "overwrite_file");
}

#[test]
fn question_id_deserialize_validates() {
    let id: QuestionId = serde_json::from_str("\"confirm\"").unwrap();
    assert_eq!(id, "confirm");

    // A dotted id is rejected at the deserialization boundary.
    assert!(serde_json::from_str::<QuestionId>("\"a.b\"").is_err());
}

#[test]
fn question_id_serializes_transparently() {
    let id: QuestionId = "confirm".parse().unwrap();
    assert_eq!(serde_json::to_string(&id).unwrap(), "\"confirm\"");
}

#[test]
fn question_constructors_reject_dotted_id() {
    assert!(Question::boolean("a.b", "?").is_err());
    assert!(Question::text("a.b", "?").is_err());
    assert!(Question::select("a.b", "?").is_err());

    let q = Question::boolean("confirm", "Proceed?").unwrap();
    assert_eq!(q.id, "confirm");
}

#[test]
fn answer_type_serializes_with_internal_type_tag() {
    use serde_json::json;

    assert_eq!(
        serde_json::to_value(AnswerType::Boolean).unwrap(),
        json!({ "type": "boolean" })
    );
    assert_eq!(
        serde_json::to_value(AnswerType::Text).unwrap(),
        json!({ "type": "text" })
    );
    assert_eq!(
        serde_json::to_value(AnswerType::Secret).unwrap(),
        json!({ "type": "secret" })
    );
    assert_eq!(
        serde_json::to_value(AnswerType::Select {
            options: vec!["a".to_owned()]
        })
        .unwrap(),
        json!({ "type": "select", "options": ["a"] })
    );

    let secret: AnswerType = serde_json::from_value(json!({ "type": "secret" })).unwrap();
    assert_eq!(secret, AnswerType::Secret);
}

#[test]
fn question_secret_constructor() {
    let q = Question::secret("passphrase", "Enter passphrase").unwrap();
    assert_eq!(q.id, "passphrase");
    assert_eq!(q.answer_type, AnswerType::Secret);
    assert!(Question::secret("a.b", "?").is_err());
}
