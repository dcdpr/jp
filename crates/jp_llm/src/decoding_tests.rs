use futures::{StreamExt as _, stream};
use serde_json::{Map, Value, json};

use super::*;
use crate::{StreamError, StreamErrorKind, event::FinishReason, event_builder::EventBuilder};

#[tokio::test]
async fn plans_follow_tool_names_across_interleaved_calls() {
    let mut decoders = ArgumentDecoders::default();
    decoders.insert("search", ArgumentDecoding {
        omit_null: vec!["kinds".to_owned()],
        ..ArgumentDecoding::default()
    });
    let mut events = decoders.attach(
        stream::iter(vec![
            Ok(Event::tool_call_start(0, "call_1", "search")),
            Ok(Event::tool_call_start(1, "call_2", "other")),
            Ok(Event::tool_call_args(0, r#"{"kinds":"#)),
            Ok(Event::tool_call_args(1, r#"{"kinds":null}"#)),
            Ok(Event::tool_call_start(0, "ignored", "other")),
            Ok(Event::tool_call_args(0, "null}")),
            Ok(Event::flush(1)),
            Ok(Event::flush(0)),
        ])
        .boxed(),
    );
    let mut builder = EventBuilder::new();
    let mut calls = vec![];
    while let Some(event) = events.next().await {
        match event.unwrap() {
            Event::Part {
                index,
                part,
                metadata,
            } => builder.handle_part(index, part, metadata),
            Event::Flush { index, metadata } => {
                let event = builder.handle_flush(index, metadata).unwrap();
                assert_eq!(event.metadata, Map::new());
                let call = event.into_tool_call_request().unwrap();
                calls.push(json!({"id": call.id, "name": call.name, "arguments": call.arguments}));
            }
            _ => panic!("unexpected event"),
        }
    }
    assert_eq!(calls, vec![
        json!({"id": "call_2", "name": "other", "arguments": {"kinds": null}}),
        json!({"id": "call_1", "name": "search", "arguments": {}}),
    ]);
}

#[tokio::test]
async fn decoding_does_not_hide_chunks_errors_or_incomplete_calls() {
    let mut decoders = ArgumentDecoders::default();
    decoders.insert("search", ArgumentDecoding {
        omit_null: vec!["kinds".to_owned()],
        ..ArgumentDecoding::default()
    });
    let mut events = decoders.attach(
        stream::iter(vec![
            Ok(Event::tool_call_start(0, "call_1", "search")),
            Ok(Event::tool_call_args(0, r#"{"kinds":n"#)),
            Err(StreamError::timeout("fixture timeout")),
            Ok(Event::Finished(FinishReason::MaxTokens)),
        ])
        .boxed(),
    );
    let mut builder = EventBuilder::new();
    let Event::Part {
        index,
        part,
        metadata,
    } = events.next().await.unwrap().unwrap()
    else {
        panic!("expected start");
    };
    builder.handle_part(index, part, metadata);
    let chunk = events.next().await.unwrap().unwrap();
    assert_eq!(chunk, Event::tool_call_args(0, r#"{"kinds":n"#));
    let Event::Part {
        index,
        part,
        metadata,
    } = chunk
    else {
        panic!("expected chunk");
    };
    builder.handle_part(index, part, metadata);
    let error = events.next().await.unwrap().unwrap_err();
    assert_eq!(error.kind, StreamErrorKind::Timeout);
    assert_eq!(error.message(), "fixture timeout");
    assert_eq!(
        events.next().await.unwrap().unwrap(),
        Event::Finished(FinishReason::MaxTokens)
    );
    assert!(events.next().await.is_none());
    assert_eq!(builder.incomplete_tool_calls(), vec!["search"]);
    assert_eq!(builder.drain(), vec![]);
}

#[test]
fn explicit_values_are_preserved() {
    let plan = ArgumentDecoding {
        omit_null: vec!["kinds".to_owned()],
        ..ArgumentDecoding::default()
    };
    let mut arguments = json!({"kinds": ["Function"], "nullable": null})
        .as_object()
        .unwrap()
        .clone();
    plan.apply(&mut arguments);
    assert_eq!(
        Value::Object(arguments),
        json!({"kinds": ["Function"], "nullable": null})
    );
}
