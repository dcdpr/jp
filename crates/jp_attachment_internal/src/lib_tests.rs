use std::sync::Arc;

use camino::Utf8PathBuf;
use jp_storage::backend::{FsStorageBackend, PersistBackend as _};
use jp_workspace::Workspace;
use test_log::test;

use super::*;

fn workspace_with_backend(
    root: impl Into<Utf8PathBuf>,
    id: jp_workspace::Id,
    backend: FsStorageBackend,
) -> Workspace {
    let mut workspace = Workspace::new_with_id(root, id).with_backend(Arc::new(backend));
    workspace.load_conversation_index();
    workspace
}

#[test]
fn uri_to_entry_minimal() {
    let uri = Url::parse("jp://jp-c123456").unwrap();
    let entry = uri_to_entry(&uri).unwrap();
    assert_eq!(entry.id, "jp-c123456");
    assert_eq!(entry.select, "");
    assert_eq!(entry.raw, None);
}

#[test]
fn uri_to_entry_with_selector() {
    let uri = Url::parse("jp://jp-c123456?select=u,a:-3..").unwrap();
    let entry = uri_to_entry(&uri).unwrap();
    assert_eq!(entry.id, "jp-c123456");
    assert_eq!(entry.select, "u,a:-3..");
    assert_eq!(entry.raw, None);
}

#[test]
fn uri_to_entry_with_raw_flag() {
    let uri = Url::parse("jp://jp-c123456?raw").unwrap();
    let entry = uri_to_entry(&uri).unwrap();
    assert_eq!(entry.raw, Some(RawMode::Events));
    assert_eq!(entry.select, "");
}

#[test]
fn uri_to_entry_with_raw_all() {
    let uri = Url::parse("jp://jp-c123456?raw=all").unwrap();
    let entry = uri_to_entry(&uri).unwrap();
    assert_eq!(entry.raw, Some(RawMode::All));
}

#[test]
fn uri_to_entry_combines_select_and_raw() {
    let uri = Url::parse("jp://jp-c123456?select=a:-3..&raw=all").unwrap();
    let entry = uri_to_entry(&uri).unwrap();
    assert_eq!(entry.select, "a:-3..");
    assert_eq!(entry.raw, Some(RawMode::All));
}

#[test]
fn uri_to_entry_ignores_path_and_fragment() {
    let uri = Url::parse("jp://jp-c123456/ignored/path?select=a:-1#fragment").unwrap();
    let entry = uri_to_entry(&uri).unwrap();
    assert_eq!(entry.id, "jp-c123456");
    assert_eq!(entry.select, "a:-1");
}

#[test]
fn uri_to_entry_duplicate_params_are_last_one_wins() {
    let uri = Url::parse("jp://jp-c123456?select=a:-1&select=*:-3..&raw&raw=all").unwrap();
    let entry = uri_to_entry(&uri).unwrap();
    assert_eq!(entry.select, "*:-3..");
    assert_eq!(entry.raw, Some(RawMode::All));
}

#[test]
fn uri_to_entry_rejects_unknown_param() {
    let uri = Url::parse("jp://jp-c123456?bogus=1").unwrap();
    assert!(uri_to_entry(&uri).is_err());
}

#[test]
fn uri_to_entry_rejects_invalid_raw_value() {
    let uri = Url::parse("jp://jp-c123456?raw=nope").unwrap();
    assert!(uri_to_entry(&uri).is_err());
}

#[test]
fn uri_to_entry_rejects_missing_id() {
    // `jp:` with no authority at all is an opaque URL and fails host parsing.
    let uri = Url::parse("jp:no-host").unwrap();
    assert!(uri_to_entry(&uri).is_err());
}

#[test]
fn entry_to_url_minimal_has_no_query() {
    let entry = Entry {
        id: "jp-c123456".to_owned(),
        select: String::new(),
        raw: None,
    };
    assert_eq!(entry.to_url().unwrap().as_str(), "jp://jp-c123456");
}

#[test]
fn entry_to_url_select_only() {
    let entry = Entry {
        id: "jp-c123456".to_owned(),
        select: "u,a:-1".to_owned(),
        raw: None,
    };
    assert_eq!(
        entry.to_url().unwrap().as_str(),
        "jp://jp-c123456?select=u%2Ca%3A-1"
    );
}

#[test]
fn entry_to_url_raw_only() {
    let entry = Entry {
        id: "jp-c123456".to_owned(),
        select: String::new(),
        raw: Some(RawMode::Events),
    };
    assert_eq!(entry.to_url().unwrap().as_str(), "jp://jp-c123456?raw");

    let entry = Entry {
        id: "jp-c123456".to_owned(),
        select: String::new(),
        raw: Some(RawMode::All),
    };
    assert_eq!(entry.to_url().unwrap().as_str(), "jp://jp-c123456?raw=all");
}

#[test]
fn entry_to_url_select_and_raw() {
    let entry = Entry {
        id: "jp-c123456".to_owned(),
        select: "a:-3..".to_owned(),
        raw: Some(RawMode::All),
    };
    assert_eq!(
        entry.to_url().unwrap().as_str(),
        "jp://jp-c123456?select=a%3A-3..&raw=all"
    );
}

#[test]
fn entry_url_round_trips_through_uri_to_entry() {
    let cases = [
        Entry {
            id: "jp-c123456".to_owned(),
            select: String::new(),
            raw: None,
        },
        Entry {
            id: "jp-c123456".to_owned(),
            select: "u,a:-1".to_owned(),
            raw: None,
        },
        Entry {
            id: "jp-c123456".to_owned(),
            select: String::new(),
            raw: Some(RawMode::Events),
        },
        Entry {
            id: "jp-c123456".to_owned(),
            select: "*:..".to_owned(),
            raw: Some(RawMode::All),
        },
    ];
    for entry in cases {
        let url = entry.to_url().unwrap();
        let parsed = uri_to_entry(&url).unwrap();
        assert_eq!(parsed, entry, "round-trip mismatch for {url}");
    }
}

#[test]
fn validate_rejects_invalid_selector() {
    let uri = Url::parse("jp://jp-c123456?select=zzz").unwrap();
    assert!(validate(&uri).is_err());
}

#[test]
fn validate_rejects_invalid_id() {
    let uri = Url::parse("jp://not-a-real-id").unwrap();
    assert!(validate(&uri).is_err());
}

#[test]
fn validate_rejects_unsupported_variant() {
    let uri = Url::parse("jp://jp-w123456").unwrap();
    assert!(validate(&uri).is_err());
}

#[test]
fn validate_accepts_valid_uri() {
    let id = ConversationId::try_from_deciseconds(17_013_123_456).unwrap();
    let uri = Url::parse(&format!("jp://{id}?select=a:-1")).unwrap();
    validate(&uri).unwrap();
}

#[test]
fn resolve_errors_when_conversation_is_not_loaded() {
    let tmp = camino_tempfile::tempdir().unwrap();
    let workspace = Workspace::new(tmp.path().to_path_buf());
    let id = ConversationId::try_from_deciseconds(17_013_123_456).unwrap();
    let uri = Url::parse(&format!("jp://{id}")).unwrap();

    let result = resolve(&workspace, &uri);
    assert!(result.is_err(), "expected error, got {result:?}");
    assert!(
        result.unwrap_err().to_string().contains("not found"),
        "error message should explain the missing conversation"
    );
}

#[test]
fn render_stream_empty_returns_empty_string() {
    let stream = ConversationStream::new_test();
    let rendered = render_stream(&stream, Selector::default());
    assert_eq!(rendered, "");
}

#[test]
fn render_stream_last_assistant_only() {
    use jp_conversation::event::ChatRequest;

    let mut stream = ConversationStream::new_test();
    stream.start_turn(ChatRequest::from("hello"));
    stream
        .current_turn_mut()
        .add_chat_response(ChatResponse::message("world"))
        .build()
        .unwrap();

    stream.start_turn(ChatRequest::from("how are you?"));
    stream
        .current_turn_mut()
        .add_chat_response(ChatResponse::reasoning("thinking..."))
        .add_chat_response(ChatResponse::message("doing well"))
        .build()
        .unwrap();

    let rendered = render_stream(&stream, Selector::default());
    assert_eq!(rendered, indoc::indoc! {"
            ## Turn 2

            ### Assistant

            doing well

        "});
}

#[test]
fn render_stream_all_content_all_turns() {
    use jp_conversation::event::ChatRequest;

    let mut stream = ConversationStream::new_test();
    stream.start_turn(ChatRequest::from("hello"));
    stream
        .current_turn_mut()
        .add_chat_response(ChatResponse::reasoning("ok let's see"))
        .add_chat_response(ChatResponse::message("hi!"))
        .build()
        .unwrap();

    let selector: Selector = "*:..".parse().unwrap();
    let rendered = render_stream(&stream, selector);
    assert_eq!(rendered, indoc::indoc! {"
            ## Turn 1

            ### User

            hello

            ### Reasoning

            ok let's see

            ### Assistant

            hi!

        "});
}

#[test]
fn render_stream_skips_turns_without_matching_content() {
    use jp_conversation::event::ChatRequest;

    let mut stream = ConversationStream::new_test();
    stream.start_turn(ChatRequest::from("first"));

    stream.start_turn(ChatRequest::from("second"));
    stream
        .current_turn_mut()
        .add_chat_response(ChatResponse::message("hi"))
        .build()
        .unwrap();

    let selector: Selector = "a:..".parse().unwrap();
    let rendered = render_stream(&stream, selector);
    assert!(rendered.contains("## Turn 2"));
    assert!(!rendered.contains("## Turn 1"));
}

#[test]
fn resolve_loads_real_persisted_conversation() {
    use jp_conversation::{Conversation, event::ChatRequest};

    let tmp = camino_tempfile::tempdir().unwrap();
    let workspace_root = tmp.path().to_path_buf();
    let storage = workspace_root.join(".jp");
    std::fs::create_dir_all(&storage).unwrap();

    let workspace_id = jp_workspace::Id::new();
    workspace_id.store(&storage).unwrap();

    let mut stream = ConversationStream::new_test();
    stream.start_turn(ChatRequest::from("hello"));
    stream
        .current_turn_mut()
        .add_chat_response(ChatResponse::message("hi there"))
        .build()
        .unwrap();
    stream.start_turn(ChatRequest::from("follow up"));
    stream
        .current_turn_mut()
        .add_chat_response(ChatResponse::reasoning("thinking"))
        .add_chat_response(ChatResponse::message("final answer"))
        .build()
        .unwrap();

    let id = ConversationId::try_from_deciseconds(17_013_123_456).unwrap();
    let metadata = Conversation::default();

    let backend = FsStorageBackend::new(&storage).unwrap();
    backend.write(&id, &metadata, &stream).unwrap();

    let workspace = workspace_with_backend(workspace_root, workspace_id, backend);
    let uri = Url::parse(&format!("jp://{id}")).unwrap();
    let attachments = resolve(&workspace, &uri).unwrap();
    assert_eq!(attachments.len(), 1, "one attachment expected");

    let text = attachments[0].as_text().expect("text attachment");
    // Default selector = last assistant response only.
    assert!(text.contains("final answer"), "content not found: {text}");
    assert!(
        !text.contains("hi there"),
        "earlier turn leaked into selection: {text}"
    );
    assert!(
        !text.contains("thinking"),
        "reasoning leaked into default selection: {text}"
    );
}

#[test]
fn resolve_raw_events_returns_selected_events_json() {
    use jp_conversation::{Conversation, event::ChatRequest};

    let tmp = camino_tempfile::tempdir().unwrap();
    let workspace_root = tmp.path().to_path_buf();
    let storage = workspace_root.join(".jp");
    std::fs::create_dir_all(&storage).unwrap();
    let workspace_id = jp_workspace::Id::new();
    workspace_id.store(&storage).unwrap();

    let mut stream = ConversationStream::new_test();
    stream.start_turn(ChatRequest::from("hello"));
    stream
        .current_turn_mut()
        .add_chat_response(ChatResponse::message("world"))
        .build()
        .unwrap();

    let id = ConversationId::try_from_deciseconds(17_013_123_456).unwrap();
    let backend = FsStorageBackend::new(&storage).unwrap();
    backend
        .write(&id, &Conversation::default(), &stream)
        .unwrap();

    let workspace = workspace_with_backend(workspace_root, workspace_id, backend);
    let uri = Url::parse(&format!("jp://{id}?raw")).unwrap();
    let attachments = resolve(&workspace, &uri).unwrap();
    assert_eq!(attachments.len(), 1);
    let body = attachments[0].as_text().unwrap();

    let json: Value = serde_json::from_str(body).unwrap();
    let events = json["events"].as_array().expect("events array");

    // Default selector matches regular rendered mode: last assistant only,
    // plus the selected turn marker.
    assert_eq!(events.len(), 2, "events body: {body}");
    assert!(body.contains("\"world\""), "events body: {body}");
    assert!(!body.contains("\"hello\""), "events body: {body}");
    assert!(json.get("base_config").is_none());
    assert!(json.get("metadata").is_none());
}

#[test]
fn resolve_raw_all_returns_metadata_and_base_config() {
    use jp_conversation::{Conversation, event::ChatRequest};

    let tmp = camino_tempfile::tempdir().unwrap();
    let workspace_root = tmp.path().to_path_buf();
    let storage = workspace_root.join(".jp");
    std::fs::create_dir_all(&storage).unwrap();
    let workspace_id = jp_workspace::Id::new();
    workspace_id.store(&storage).unwrap();

    let mut stream = ConversationStream::new_test();
    stream.start_turn(ChatRequest::from("hi"));
    stream
        .current_turn_mut()
        .add_chat_response(ChatResponse::message("hello"))
        .build()
        .unwrap();

    let id = ConversationId::try_from_deciseconds(17_013_123_456).unwrap();
    let backend = FsStorageBackend::new(&storage).unwrap();
    backend
        .write(&id, &Conversation::default(), &stream)
        .unwrap();

    let workspace = workspace_with_backend(workspace_root, workspace_id, backend);
    let uri = Url::parse(&format!("jp://{id}?raw=all")).unwrap();
    let attachments = resolve(&workspace, &uri).unwrap();
    assert_eq!(attachments.len(), 1, "raw=all is one JSON attachment");

    let body = attachments[0].as_text().unwrap();
    let json: Value = serde_json::from_str(body).unwrap();
    assert!(json["events"].is_array(), "raw body: {body}");
    assert!(json["base_config"].is_object(), "raw body: {body}");
    assert!(json["metadata"].is_object(), "raw body: {body}");
}

#[test]
fn resolve_raw_with_selector_filters_events() {
    use jp_conversation::{Conversation, event::ChatRequest};

    let tmp = camino_tempfile::tempdir().unwrap();
    let workspace_root = tmp.path().to_path_buf();
    let storage = workspace_root.join(".jp");
    std::fs::create_dir_all(&storage).unwrap();
    let workspace_id = jp_workspace::Id::new();
    workspace_id.store(&storage).unwrap();

    let mut stream = ConversationStream::new_test();
    stream.start_turn(ChatRequest::from("first user"));
    stream
        .current_turn_mut()
        .add_chat_response(ChatResponse::message("first assistant"))
        .build()
        .unwrap();
    stream.start_turn(ChatRequest::from("second user"));
    stream
        .current_turn_mut()
        .add_chat_response(ChatResponse::message("second assistant"))
        .build()
        .unwrap();

    let id = ConversationId::try_from_deciseconds(17_013_123_456).unwrap();
    let backend = FsStorageBackend::new(&storage).unwrap();
    backend
        .write(&id, &Conversation::default(), &stream)
        .unwrap();

    let workspace = workspace_with_backend(workspace_root, workspace_id, backend);
    // Last turn only, all content kinds, raw output.
    let uri = Url::parse(&format!("jp://{id}?select=*:-1&raw")).unwrap();
    let attachments = resolve(&workspace, &uri).unwrap();
    let body = attachments[0].as_text().unwrap();

    let json: Value = serde_json::from_str(body).unwrap();
    let events = json["events"].as_array().expect("events array");

    assert_eq!(events.len(), 3, "turn marker + user + assistant: {body}");
    assert!(body.contains("second user"));
    assert!(body.contains("second assistant"));
    assert!(!body.contains("first user"), "earlier turn leaked: {body}");
    assert!(!body.contains("first assistant"));
}

#[test]
fn markdown_fence_expands_for_nested_backticks() {
    assert_eq!(markdown_fence("plain output"), "```");
    assert_eq!(markdown_fence("```rust\nfn main() {}\n```"), "````");
}
