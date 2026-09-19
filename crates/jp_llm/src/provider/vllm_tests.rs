use jp_config::{
    assistant::{sections::SectionConfig, tool_choice::ToolChoice},
    model::parameters::PartialReasoningConfig,
};
use jp_conversation::{
    ConversationEvent, ConversationStream,
    event::{ChatRequest, ChatResponse, ToolCallRequest, ToolCallResponse},
    thread::Thread,
};
use serde_json::{Map, json};

use super::*;
use crate::tool::{ToolDefinition, ToolDocs};

fn qwen_model() -> VllmModel {
    serde_json::from_value(json!({
        "id": "Qwen/Qwen3-8B",
        "object": "model",
        "owned_by": "vllm",
        "max_model_len": 40_960,
    }))
    .unwrap()
}

fn qwen_details() -> ModelDetails {
    ModelDetails::empty((PROVIDER, "Qwen/Qwen3-8B").try_into().unwrap())
}

fn query(events: ConversationStream, tools: Vec<ToolDefinition>) -> ChatQuery {
    ChatQuery {
        thread: Thread {
            system_prompt: None,
            sections: vec![],
            attachments: vec![],
            events,
        },
        tools,
        tool_choice: ToolChoice::Auto,
    }
}

/// vLLM reports the served context window as `max_model_len`, and the model id
/// keeps its vendor prefix because vLLM accepts only the full id.
#[test]
fn map_model_keeps_full_id_and_reads_max_model_len() {
    let details = map_model(&qwen_model()).unwrap();

    assert_eq!(details.id.name.as_ref(), "Qwen/Qwen3-8B");
    assert_eq!(details.context_window, Some(40_960));
    assert_eq!(details.reasoning, None);
}

/// A plain message becomes one user message, with streaming on and thinking
/// off, because the test config has no reasoning setting.
#[test]
fn create_request_plain_message() {
    let events = ConversationStream::new_test().with_turn("Hello");

    let (body, is_structured) = create_request(&qwen_details(), query(events, vec![])).unwrap();

    assert!(!is_structured);
    assert_eq!(
        body,
        json!({
            "model": "Qwen/Qwen3-8B",
            "messages": [{ "role": "user", "content": "Hello" }],
            "stream": true,
            "chat_template_kwargs": { "enable_thinking": false },
        })
    );
}

/// Regression: vLLM renders the request through the served model's own chat
/// template, and several of those templates reject a system message that isn't
/// the first message.
/// The prompt, its sections, and the attachment XML must therefore arrive as a
/// single system message.
#[test]
fn create_request_joins_system_parts_into_one_message() {
    let query = ChatQuery {
        thread: Thread {
            system_prompt: Some("You are JP.".to_owned()),
            sections: vec![
                SectionConfig::default().with_content("Rule 1."),
                SectionConfig::default().with_content("Rule 2."),
            ],
            attachments: vec![],
            events: ConversationStream::new_test().with_turn("test"),
        },
        tools: vec![],
        tool_choice: ToolChoice::Auto,
    };

    let (body, _) = create_request(&qwen_details(), query).unwrap();

    assert_eq!(
        body["messages"],
        json!([
            { "role": "system", "content": "You are JP.\n\nRule 1.\n\nRule 2." },
            { "role": "user", "content": "test" },
        ])
    );
}

/// Whether the model thinks at all is the chat template's decision, driven by
/// `enable_thinking`.
#[test]
fn create_request_asks_the_template_to_think_when_reasoning_is_on() {
    let mut events = ConversationStream::new_test().with_turn("Hello");
    let mut delta = jp_config::PartialAppConfig::empty();
    delta.assistant.model.parameters.reasoning = Some(PartialReasoningConfig::Auto);
    events.add_config_delta(delta);

    let (body, _) = create_request(&qwen_details(), query(events, vec![])).unwrap();

    assert_eq!(
        body["chat_template_kwargs"],
        json!({ "enable_thinking": true })
    );
    assert!(body.get("reasoning_format").is_none());
}

/// A tool call and its result become one assistant message with `tool_calls`
/// and one `tool` message, and the tool list uses the strict function shape.
#[test]
fn create_request_tool_call_round_trip() {
    let mut events = ConversationStream::new_test().with_turn("Read the file");
    events.extend([
        ConversationEvent::now(ToolCallRequest {
            id: "call_1".into(),
            name: "read_file".into(),
            arguments: serde_json::from_value(json!({ "path": "a.txt" })).unwrap(),
        }),
        ConversationEvent::now(ToolCallResponse {
            id: "call_1".into(),
            result: Ok("contents".into()),
        }),
    ]);

    let tool = ToolDefinition {
        name: "read_file".into(),
        docs: ToolDocs {
            summary: Some("Read a file.".into()),
            ..ToolDocs::default()
        },
        parameters: json!({
            "type": "object",
            "properties": { "path": { "type": "string" } },
            "required": ["path"],
        }),
    };

    let (body, _) = create_request(&qwen_details(), query(events, vec![tool])).unwrap();

    assert_eq!(
        body["messages"],
        json!([
            { "role": "user", "content": "Read the file" },
            {
                "role": "assistant",
                "tool_calls": [{
                    "id": "call_1",
                    "type": "function",
                    "function": { "name": "read_file", "arguments": "{\"path\":\"a.txt\"}" },
                }],
            },
            { "role": "tool", "tool_call_id": "call_1", "content": "contents" },
        ])
    );
    assert_eq!(body["tool_choice"], json!("auto"));
    assert_eq!(
        body["tools"],
        json!([{
            "type": "function",
            "function": {
                "name": "read_file",
                "description": "Read a file.",
                "parameters": {
                    "type": "object",
                    "properties": { "path": { "type": "string" } },
                    "required": ["path"],
                    "additionalProperties": false,
                },
                "strict": true,
            },
        }])
    );
}

/// A schema on the request becomes a strict `json_schema` response format.
#[test]
fn create_request_structured_schema() {
    let schema: Map<String, serde_json::Value> = serde_json::from_value(json!({
        "type": "object",
        "properties": { "answer": { "type": "string" } },
    }))
    .unwrap();
    let events = ConversationStream::new_test().with_turn(ChatRequest {
        content: "Answer".into(),
        schema: Some(schema),
        author: None,
    });

    let (body, is_structured) = create_request(&qwen_details(), query(events, vec![])).unwrap();

    assert!(is_structured);
    assert_eq!(
        body["response_format"],
        json!({
            "type": "json_schema",
            "json_schema": {
                "name": "structured_output",
                "schema": {
                    "type": "object",
                    "properties": { "answer": { "type": "string" } },
                },
                "strict": true,
            },
        })
    );
}

/// The prior assistant reply stays a plain message in the history.
#[test]
fn create_request_keeps_assistant_history() {
    let mut events = ConversationStream::new_test().with_turn("Hi");
    events.extend([ConversationEvent::now(ChatResponse::message("Hello!"))]);
    let events = events.with_turn("Again");

    let (body, _) = create_request(&qwen_details(), query(events, vec![])).unwrap();

    assert_eq!(
        body["messages"],
        json!([
            { "role": "user", "content": "Hi" },
            { "role": "assistant", "content": "Hello!" },
            { "role": "user", "content": "Again" },
        ])
    );
}
