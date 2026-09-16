use serde_json::json;

use super::*;

fn question(id: &str, answer_type: AnswerType) -> Question {
    Question {
        id: id.parse().unwrap(),
        text: "Which branch?".to_owned(),
        pre_amble: Some("A preamble the request does not carry.".to_owned()),
        answer_type,
        default: Some(json!("main")),
    }
}

#[test]
fn a_successful_outcome_becomes_one_text_block() {
    let result = ToolResult::from(Outcome::Success {
        content: "done".to_owned(),
    });

    assert_eq!(result, ToolResult {
        content: vec![ContentBlock::text("done")],
        status: ToolStatus::Success,
        structured_content: None,
        metadata: None,
    });
}

#[test]
fn a_failed_outcome_keeps_its_trace_and_transience() {
    let result = ToolResult::from(Outcome::Error {
        message: "File not found: foo.rs".to_owned(),
        trace: vec!["io error: No such file or directory".to_owned()],
        transient: true,
    });

    assert_eq!(result, ToolResult {
        content: vec![ContentBlock::text(
            "File not found: foo.rs\n\nTrace:\nio error: No such file or directory"
        )],
        structured_content: None,
        metadata: None,
        status: ToolStatus::Error(ErrorDetails {
            transient: true,
            trace: vec!["io error: No such file or directory".to_owned()],
        }),
    });
}

/// A tool that stops to ask something has not failed: the caller answers and
/// runs it again.
#[test]
fn a_needs_input_outcome_is_not_an_error() {
    let result = ToolResult::from(Outcome::NeedsInput {
        question: question("target", AnswerType::Text),
    });

    assert!(!result.is_error());
    assert_eq!(result.error_details(), None);
    assert_eq!(
        result.input_request().map(|r| r.id.as_str()),
        Some("target")
    );
}

#[test]
fn outcome_conversion_preserves_question_context() {
    let result = ToolResult::from(Outcome::NeedsInput {
        question: question("target", AnswerType::Text),
    });
    assert_eq!(result.to_text(), "A preamble the request does not carry.");
    assert_eq!(result.content.len(), 2);
}

#[test]
fn a_question_keeps_its_answer_type_and_derives_an_enum_schema() {
    let answer_type = AnswerType::Select {
        options: vec!["main".to_owned(), "develop".to_owned()],
    };
    let request = InputRequest::from(question("branch", answer_type.clone()));

    assert_eq!(request, InputRequest {
        id: "branch".parse().unwrap(),
        label: "Which branch?".to_owned(),
        answer_type,
        default: Some(json!("main")),
    });
    assert_eq!(
        request.schema(),
        json!({ "type": "string", "enum": ["main", "develop"] })
            .as_object()
            .cloned()
            .unwrap()
    );
}

#[test]
fn a_boolean_question_derives_a_boolean_schema() {
    let request = InputRequest::from(question("proceed", AnswerType::Boolean));

    assert_eq!(request.answer_type, AnswerType::Boolean);
    assert_eq!(
        request.schema(),
        json!({ "type": "boolean" }).as_object().cloned().unwrap()
    );
}

/// Secrecy rides on the answer type, not on a schema keyword: a consumer that
/// rewrites the schema for a provider cannot drop the rule that the answer
/// stays off disk.
#[test]
fn a_secret_question_derives_a_plain_string_schema_and_stays_secret() {
    let request = InputRequest::from(question("token", AnswerType::Secret));

    assert!(request.is_secret());
    assert_eq!(
        request.schema(),
        json!({ "type": "string" }).as_object().cloned().unwrap()
    );
}

/// A text question derives the same schema as a secret one, which is exactly
/// why the schema cannot be what tells them apart.
#[test]
fn an_ordinary_text_question_is_not_secret() {
    let request = InputRequest::from(question("name", AnswerType::Text));

    assert!(!request.is_secret());
    assert_eq!(
        request.schema(),
        InputRequest::from(question("token", AnswerType::Secret)).schema()
    );
}

/// Every answer type comes back as itself after a request crosses the service
/// boundary, including the two that share a schema and the one whose options a
/// schema-only representation would have to re-read.
#[test]
fn every_answer_type_survives_the_request_round_trip() {
    let types = [
        AnswerType::Text,
        AnswerType::Secret,
        AnswerType::Boolean,
        AnswerType::Select {
            options: vec!["main".to_owned(), "develop".to_owned()],
        },
    ];

    for answer_type in types {
        let original = question("q", answer_type.clone());
        let request = InputRequest::from(original.clone());

        let mut restored = Question::new(request.id, request.label, request.answer_type);
        restored.default = request.default;
        restored.pre_amble = original.pre_amble.clone();

        assert_eq!(restored, original, "round trip lost {answer_type:?}");
    }
}

#[test]
fn flattening_joins_blocks_in_order_with_a_blank_line() {
    let result = ToolResult {
        content: vec![
            ContentBlock::text("first"),
            ContentBlock::Resource(Resource::text("file:///a.rs", "second")),
            ContentBlock::text("third"),
        ],
        status: ToolStatus::Success,
        structured_content: None,
        metadata: None,
    };

    assert_eq!(result.to_text(), "first\n\nsecond\n\nthird");
}

/// A blob has no text to contribute, so its URI stands in for it rather than
/// its bytes reaching the model as mojibake.
#[test]
fn flattening_names_a_binary_resource_by_its_uri() {
    let result = ToolResult {
        content: vec![ContentBlock::Resource(Resource {
            content: ResourceContent::Blob(vec![0x89, 0x50, 0x4e, 0x47]),
            ..Resource::text("file:///shot.png", "")
        })],
        status: ToolStatus::Success,
        structured_content: None,
        metadata: None,
    };

    assert_eq!(result.to_text(), "file:///shot.png");
}

/// The question is answered, not read: flattening a result that carries one
/// must not put the prompt in front of the model as output.
#[test]
fn flattening_omits_a_question_but_keeps_its_context() {
    let result = ToolResult {
        content: vec![
            ContentBlock::text("Two hunks remain."),
            ContentBlock::Question(InputRequest::from(question("stage", AnswerType::Boolean))),
        ],
        status: ToolStatus::Success,
        structured_content: None,
        metadata: None,
    };

    assert_eq!(result.to_text(), "Two hunks remain.");
}

#[test]
fn outcome_error_trace_is_rendered_once() {
    let result = ToolResult::from(Outcome::Error {
        message: "failed".to_owned(),
        trace: vec!["inner".to_owned(), "innermost".to_owned()],
        transient: false,
    });

    assert_eq!(result.to_text(), "failed\n\nTrace:\ninner\ninnermost");
}

#[test]
fn flattening_an_error_without_a_trace_is_just_the_message() {
    assert_eq!(ToolResult::error("failed").to_text(), "failed");
}
