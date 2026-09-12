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
        is_error: false,
        error: None,
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
        content: vec![ContentBlock::text("File not found: foo.rs")],
        is_error: true,
        error: Some(ErrorDetails {
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

    assert!(!result.is_error);
    assert_eq!(result.error, None);
    assert_eq!(
        result.input_request().map(|r| r.id.as_str()),
        Some("target")
    );
}

#[test]
fn a_select_question_becomes_an_enum_schema() {
    let request = InputRequest::from(question("branch", AnswerType::Select {
        options: vec!["main".to_owned(), "develop".to_owned()],
    }));

    assert_eq!(request, InputRequest {
        id: "branch".parse().unwrap(),
        label: "Which branch?".to_owned(),
        schema: json!({ "type": "string", "enum": ["main", "develop"] })
            .as_object()
            .cloned()
            .unwrap(),
        default: Some(json!("main")),
        secret: false,
    });
}

#[test]
fn a_boolean_question_becomes_a_boolean_schema() {
    let request = InputRequest::from(question("proceed", AnswerType::Boolean));

    assert_eq!(
        request.schema,
        json!({ "type": "boolean" }).as_object().cloned().unwrap()
    );
}

/// Secrecy is a typed field, not a schema keyword: a consumer that rewrites the
/// schema for a provider cannot drop the rule that the answer stays off disk.
#[test]
fn a_secret_question_is_a_plain_string_schema_and_a_set_flag() {
    let request = InputRequest::from(question("token", AnswerType::Secret));

    assert!(request.secret);
    assert_eq!(
        request.schema,
        json!({ "type": "string" }).as_object().cloned().unwrap()
    );
}

#[test]
fn an_ordinary_text_question_is_not_secret() {
    assert!(!InputRequest::from(question("name", AnswerType::Text)).secret);
}

#[test]
fn flattening_joins_blocks_in_order_with_a_blank_line() {
    let result = ToolResult {
        content: vec![
            ContentBlock::text("first"),
            ContentBlock::Resource(Resource::text("file:///a.rs", "second")),
            ContentBlock::text("third"),
        ],
        is_error: false,
        error: None,
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
        is_error: false,
        error: None,
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
        is_error: false,
        error: None,
    };

    assert_eq!(result.to_text(), "Two hunks remain.");
}

#[test]
fn flattening_an_error_appends_its_trace() {
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
