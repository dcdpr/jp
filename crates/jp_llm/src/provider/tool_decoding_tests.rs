use futures::{StreamExt as _, stream};
use jp_config::{model::id::ProviderId, providers::llm::LlmProviderConfig};
use jp_conversation::{ConversationStream, thread::ThreadBuilder};
use jp_test::mock::{MockServer, POST};
use serde_json::{Value, json};

use super::{get_provider, openai::parameters_with_decoding};
use crate::{
    EventStream,
    event::Event,
    event_builder::EventBuilder,
    model::ModelDetails,
    query::ChatQuery,
    tool::{ToolDefinition, ToolDocs, decoding::ArgumentDecoders},
};

async fn assemble(schema: &Value, strict: bool, arguments: &str) -> Value {
    let (_parameters, decoding) = parameters_with_decoding(schema, strict);
    let mut decoders = ArgumentDecoders::default();
    decoders.insert("search", decoding);
    let events: EventStream = stream::iter(vec![
        Ok(Event::tool_call_start(0, "call_1", "search")),
        Ok(Event::tool_call_args(0, arguments)),
        Ok(Event::flush(0)),
    ])
    .boxed();
    let mut events = decoders.attach(events);
    let mut builder = EventBuilder::new();
    let mut requests = vec![];
    while let Some(event) = events.next().await {
        match event.unwrap() {
            Event::Part {
                index,
                part,
                metadata,
            } => builder.handle_part(index, part, metadata),
            Event::Flush { index, metadata } => {
                let event = builder.handle_flush(index, metadata).unwrap();
                let request = event.into_tool_call_request().unwrap();
                assert_eq!(request.id, "call_1");
                assert_eq!(request.name, "search");
                requests.push(Value::Object(request.arguments));
            }
            _ => panic!("unexpected event"),
        }
    }
    assert_eq!(requests.len(), 1);
    requests.remove(0)
}

#[tokio::test]
async fn strict_tool_omission_round_trip() {
    let schema = json!({
        "type": "object",
        "required": ["query"],
        "properties": {
            "query": {"type": "string"},
            "kinds": {"type": "array", "items": {"type": "string"}, "default": ["Function"]},
            "version": {"type": "string", "default": "latest"}
        }
    });
    assert_eq!(
        assemble(
            &schema,
            true,
            r#"{"query":"parse_document","kinds":null,"version":null}"#
        )
        .await,
        json!({"query": "parse_document"})
    );
}

#[tokio::test]
async fn strict_tool_preserves_source_nulls_and_explicit_values() {
    let schema = json!({
        "type": "object",
        "required": ["required"],
        "properties": {
            "required": {"type": "string"},
            "nullable": {"type": ["string", "null"], "default": "fallback"},
            "union": {"anyOf": [{"type": "string"}, {"type": "null"}]},
            "kinds": {"type": "array", "items": {"type": "string"}}
        }
    });
    assert_eq!(
        assemble(
            &schema,
            true,
            r#"{"required":null,"nullable":null,"union":null,"kinds":[]}"#
        )
        .await,
        json!({"required": null, "nullable": null, "union": null, "kinds": []})
    );
}

#[tokio::test]
async fn strict_tool_decodes_nested_objects_and_array_items() {
    let schema = json!({
        "type": "object",
        "properties": {
            "options": {"type": "object", "properties": {"limit": {"type": "integer"}}},
            "rows": {"type": "array", "items": {
                "type": "object", "properties": {"label": {"type": "string"}}
            }}
        }
    });
    assert_eq!(
        assemble(
            &schema,
            true,
            r#"{"options":{"limit":null},"rows":[{"label":null},{"label":"kept"},null]}"#
        )
        .await,
        json!({"options": {}, "rows": [{}, {"label": "kept"}, null]})
    );
}

const OPENAI_CALL: &str = r#"event: response.output_item.added
data: {"type":"response.output_item.added","output_index":0,"sequence_number":0,"item":{"type":"function_call","id":"fc_1","call_id":"call_1","name":"search","arguments":""}}

event: response.function_call_arguments.delta
data: {"type":"response.function_call_arguments.delta","item_id":"fc_1","output_index":0,"sequence_number":1,"delta":"{\"query\":\"parse_document\",\"kinds\":null}"}

event: response.output_item.done
data: {"type":"response.output_item.done","output_index":0,"sequence_number":2,"item":{"type":"function_call","id":"fc_1","call_id":"call_1","name":"search","arguments":"{\"query\":\"parse_document\",\"kinds\":null}","status":"completed"}}

"#;

const CHAT_CALL: &str = r#"data: {"id":"chat_1","object":"chat.completion.chunk","created":0,"model":"test","provider":"test","choices":[{"index":0,"delta":{"role":"assistant","tool_calls":[{"index":0,"id":"call_1","type":"function","function":{"name":"search","arguments":"{\"query\":\"parse_document\",\"kinds\":null}"}}]},"finish_reason":null}]}

data: {"id":"chat_1","object":"chat.completion.chunk","created":0,"model":"test","provider":"test","choices":[{"index":0,"delta":{},"finish_reason":"tool_calls"}]}

data: [DONE]

"#;

async fn provider_round_trip(provider_id: ProviderId, body: &'static str, streaming: bool) {
    let server = MockServer::start_async().await;
    let endpoint = server
        .mock_async(|when, then| {
            when.method(POST);
            then.status(200)
                .header(
                    "content-type",
                    if streaming {
                        "text/event-stream"
                    } else {
                        "application/json"
                    },
                )
                .body(body);
        })
        .await;
    let mut config = LlmProviderConfig::default();
    let key_env = if cfg!(windows) { "USERNAME" } else { "USER" };
    config.openai.api_key_env = key_env.to_owned();
    config.openai.base_url_env = String::new();
    config.openai.base_url = server.base_url();
    config.openrouter.api_key_env = key_env.to_owned();
    config.openrouter.base_url = server.base_url();
    config.llamacpp.base_url = server.base_url();
    let provider = get_provider(provider_id, &config).unwrap();
    let mut model = ModelDetails::empty((provider_id, "test").try_into().unwrap());
    if !streaming {
        model.features.push("streaming_unsupported");
    }
    let thread = ThreadBuilder::new()
        .with_events(ConversationStream::new_test().with_turn("search"))
        .build()
        .unwrap();
    let mut query = ChatQuery::from(thread);
    query.tools.push(ToolDefinition {
        name: "search".to_owned(),
        docs: ToolDocs::default(),
        parameters: json!({
            "type": "object", "required": ["query"],
            "properties": {
                "query": {"type": "string"},
                "kinds": {"type": "array", "items": {"$ref": "#/$defs/Kind"}}
            },
            "$defs": {"Kind": {"type": "string", "enum": ["Function"]}}
        }),
    });
    let mut events = provider
        .chat_completion_stream(&model, query)
        .await
        .unwrap();
    let mut builder = EventBuilder::new();
    let mut requests = vec![];
    while let Some(event) = events.next().await {
        match event.unwrap() {
            Event::Part {
                index,
                part,
                metadata,
            } => builder.handle_part(index, part, metadata),
            Event::Flush { index, metadata } => {
                if let Some(event) = builder.handle_flush(index, metadata) {
                    assert!(event.metadata.is_empty());
                    requests.push(event.into_tool_call_request().unwrap().arguments);
                }
            }
            Event::Finished(_) | Event::KeepAlive => {}
            Event::Patch(_) => panic!("unexpected patch"),
        }
    }
    assert_eq!(endpoint.calls_async().await, 1);
    assert_eq!(requests, vec![
        json!({"query": "parse_document"})
            .as_object()
            .unwrap()
            .clone()
    ]);
}

#[tokio::test]
async fn openai_decodes_before_tool_dispatch() {
    provider_round_trip(ProviderId::Openai, OPENAI_CALL, true).await;
}

#[tokio::test]
async fn openrouter_decodes_before_tool_dispatch() {
    provider_round_trip(ProviderId::Openrouter, CHAT_CALL, true).await;
}

#[tokio::test]
async fn llamacpp_decodes_before_tool_dispatch() {
    provider_round_trip(ProviderId::Llamacpp, CHAT_CALL, true).await;
}

#[tokio::test]
async fn openai_non_streaming_decodes_before_tool_dispatch() {
    provider_round_trip(
        ProviderId::Openai,
        r#"{
        "id": "resp_1", "object": "response", "created_at": 0,
        "status": "completed", "model": "test", "error": null,
        "incomplete_details": null, "instructions": null, "metadata": {},
        "output": [{"type": "function_call", "id": "fc_1", "call_id": "call_1",
            "name": "search", "arguments": "{\"query\":\"parse_document\",\"kinds\":null}",
            "status": "completed"}],
        "parallel_tool_calls": true, "temperature": 1, "top_p": 1,
        "tool_choice": "auto", "tools": [], "usage": null,
        "reasoning": {}, "text": {"format": {"type": "text"}},
        "truncation": "disabled", "store": false
    }"#,
        false,
    )
    .await;
}

#[tokio::test]
async fn reference_and_composition_paths_remain_undecoded() {
    let schema = json!({
        "type": "object",
        "properties": {
            "inline": {"type": "string"},
            "referenced": {"$ref": "#/$defs/Options"},
            "composed": {"anyOf": [
                {"type": "object", "properties": {"value": {"type": "string"}}},
                {"type": "object", "required": ["value"], "properties": {"value": {"type": "null"}}}
            ]}
        },
        "$defs": {"Options": {"type": "object", "properties": {"value": {"type": "string"}}}}
    });
    assert_eq!(
        assemble(
            &schema,
            true,
            r#"{"inline":null,"referenced":{"value":null},"composed":{"value":null}}"#
        )
        .await,
        json!({"referenced": {"value": null}, "composed": {"value": null}})
    );
}

#[tokio::test]
async fn nullable_object_preserves_null_but_decodes_its_properties() {
    let schema = json!({
        "type": "object", "properties": {
            "absent": {"anyOf": [
                {"type": "object", "properties": {"value": {"type": "string"}}}, {"type": "null"}
            ]},
            "present": {"anyOf": [
                {"type": "object", "properties": {"value": {"type": "string"}}}, {"type": "null"}
            ]}
        }
    });
    assert_eq!(
        assemble(&schema, true, r#"{"absent":null,"present":{"value":null}}"#).await,
        json!({"absent": null, "present": {}})
    );
}

#[tokio::test]
async fn non_strict_tool_keeps_nulls() {
    let schema = json!({"type": "object", "properties": {"query": {"type": "string"}}});
    assert_eq!(
        assemble(&schema, false, r#"{"query":null}"#).await,
        json!({"query": null})
    );
}
