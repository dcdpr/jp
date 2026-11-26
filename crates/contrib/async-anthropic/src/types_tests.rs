use serde_json::json;

use super::*;

#[test_log::test(tokio::test)]
async fn test_deserialize_response() {
    let response = json!({
      "id": "msg_01KkaCASJuaAgTWD2wqdbwC8",
      "type": "message",
      "role": "assistant",
      "model": "claude-3-5-sonnet-20241022",
      "content": [
        {
          "type": "text",
          "text": "Hi! How can I help you today?"
        }
      ],
      "stop_reason": "end_turn",
      "stop_sequence": null,
      "usage": {
        "input_tokens": 10,
        "cache_creation_input_tokens": 0,
        "cache_read_input_tokens": 0,
        "output_tokens": 12
      }
    })
    .to_string();

    let response = serde_json::from_str::<CreateMessagesResponse>(&response).unwrap();

    let usage = response.usage.as_ref().unwrap();

    assert_eq!(usage.input_tokens, Some(10));
    assert_eq!(usage.output_tokens, Some(12));
    assert_eq!(
        response.id,
        Some("msg_01KkaCASJuaAgTWD2wqdbwC8".to_string())
    );
    assert_eq!(
        response.model,
        Some("claude-3-5-sonnet-20241022".to_string())
    );
    assert_eq!(response.stop_reason, Some("end_turn".to_string()));
    assert_eq!(response.stop_sequence, None);
    assert_eq!(
        response
            .messages()
            .first()
            .unwrap()
            .content
            .first()
            .unwrap()
            .as_text(),
        Some(&Text {
            text: "Hi! How can I help you today?".to_string(),
            cache_control: None,
        })
    );
}

#[test_log::test(tokio::test)]
async fn test_from_str() {
    let message: Message = "Hello world!".into();

    assert_eq!(message, Message {
        role: MessageRole::User,
        content: MessageContentList(vec![MessageContent::Text(Text {
            text: "Hello world!".to_string(),
            cache_control: None,
        })]),
    });

    assert_eq!(message.text(), Some("Hello world!".to_string()));
}
