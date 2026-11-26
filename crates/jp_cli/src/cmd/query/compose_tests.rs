use assert_matches::assert_matches;
use camino_tempfile::Utf8TempDir;
use jp_config::{PartialConfig as _, types::command::CommandConfigOrString};
use jp_conversation::event::ChatResponse;
use serde_json::Value;

use super::*;
use crate::{
    Cli,
    cli::flag::{ArgPrompts, Parsed},
    cmd::Commands,
};

#[test]
fn echo_request_unless_inline() {
    // Editor-composed query: the editor took over the screen, so echo.
    assert!(Query::default().should_echo_request(QuerySource::Editor));

    // Plain inline query, no editor: the user already sees their input.
    assert!(!Query::default().should_echo_request(QuerySource::Inline));

    // Synthesized query (`--no-edit` without a query): the user never typed
    // or saw the resulting message, so it must be echoed.
    assert!(Query::default().should_echo_request(QuerySource::Synthesized));

    // Replay without an editor: the message comes from history and isn't
    // otherwise visible on the terminal, so it must be echoed.
    let replay: Query = Query {
        replay: true,
        ..Default::default()
    };
    assert!(replay.should_echo_request(QuerySource::Inline));
}

#[test]
fn edit_message_synthesizes_when_no_edit_without_query() {
    let config = AppConfig::new_test();
    let root = Utf8Path::new("/tmp");
    let query = Query {
        no_edit: true,
        ..Default::default()
    };

    // Empty request and empty stream: a default "continue" message is
    // synthesized, so the caller must echo it.
    let mut request = ChatRequest::default();
    let stream = ConversationStream::new_test();
    let mut pending_trim = PendingStreamTrim::default();
    let (source, partial) = query
        .edit_message(
            &mut request,
            &stream,
            &mut pending_trim,
            false,
            true,
            &config,
            root,
            &Printer::sink(),
        )
        .unwrap();
    assert_eq!(source, QuerySource::Synthesized);
    assert_eq!(request.content, "continue");
    assert!(partial.is_empty());

    // Empty request with the stream's trailing event being a chat request:
    // that request is peeked and re-sent verbatim, also synthesized.
    let mut request = ChatRequest::default();
    let mut stream = ConversationStream::new_test();
    stream.start_turn("earlier text");
    let mut pending_trim = PendingStreamTrim::default();
    let (source, _) = query
        .edit_message(
            &mut request,
            &stream,
            &mut pending_trim,
            false,
            true,
            &config,
            root,
            &Printer::sink(),
        )
        .unwrap();
    assert_eq!(source, QuerySource::Synthesized);
    assert_eq!(request.content, "earlier text");
    // The trailing request is not popped here; its removal is deferred to the
    // turn-start commit point via `pending_trim`, so the stream is untouched.
    assert!(pending_trim.pop_request);
    assert!(stream.pop_if(ConversationEvent::is_chat_request).is_some());
}

#[test]
fn edit_message_quote_without_editor_is_synthesized() {
    // `--quote --no-edit`: `build_conversation` seeds the request with the
    // quoted assistant message before `edit_message` runs, so the request is
    // non-empty here even though the user never typed or saw the final text.
    // It must be classified as synthesized (and therefore echoed), not
    // inline.
    let config = AppConfig::new_test();
    let query = Query {
        quote: Some(true),
        no_edit: true,
        ..Default::default()
    };

    let mut request = ChatRequest::from(" >  quoted reply");
    let stream = ConversationStream::new_test();
    let mut pending_trim = PendingStreamTrim::default();
    let (source, partial) = query
        .edit_message(
            &mut request,
            &stream,
            &mut pending_trim,
            false,
            true,
            &config,
            Utf8Path::new("/tmp"),
            &Printer::sink(),
        )
        .unwrap();
    assert_eq!(source, QuerySource::Synthesized);
    // The seeded content is sent as-is.
    assert_eq!(request.content, " >  quoted reply");
    assert!(partial.is_empty());
}

#[test]
fn edit_message_skips_editor_when_no_edit_with_piped_stdin() {
    // Regression: `--no-edit` must skip the editor even when piped stdin
    // content makes the request non-empty. Previously the explicit override
    // was only checked when the request was still empty, so `--no-edit` was
    // silently ignored whenever stdin was piped without a query argument.
    let config = AppConfig::new_test();
    let root = Utf8Path::new("/tmp");
    let query = Query {
        no_edit: true,
        ..Default::default()
    };

    let mut request = ChatRequest::from("hi");
    let stream = ConversationStream::new_test();
    let mut pending_trim = PendingStreamTrim::default();
    let (source, partial) = query
        .edit_message(
            &mut request,
            &stream,
            &mut pending_trim,
            true,
            true,
            &config,
            root,
            &Printer::sink(),
        )
        .unwrap();

    assert_eq!(source, QuerySource::Inline);
    assert_eq!(request.content, "hi");
    assert!(partial.is_empty());
}

#[test]
fn edit_message_skips_editor_when_non_interactive() {
    // A query arriving on stdin with nobody at the terminal: the editor would
    // block on a buffer no one can save, so the piped text is sent as-is.
    //
    // The configured editor names a program that cannot be spawned, so
    // reaching the editor branch fails loudly instead of passing by accident.
    let dir = Utf8TempDir::new().unwrap();
    let mut config = AppConfig::new_test();
    config.editor.cmd = Some(CommandConfigOrString::String(
        "jp-editor-that-does-not-exist".to_owned(),
    ));

    let mut request = ChatRequest::from("piped payload");
    let stream = ConversationStream::new_test();
    let mut pending_trim = PendingStreamTrim::default();
    let (source, partial) = Query::default()
        .edit_message(
            &mut request,
            &stream,
            &mut pending_trim,
            true,
            false,
            &config,
            dir.path(),
            &Printer::sink(),
        )
        .unwrap();

    assert_eq!(source, QuerySource::Inline);
    assert_eq!(request.content, "piped payload");
    assert!(partial.is_empty());
    // The draft file is written before the editor is spawned, so its absence
    // proves the editor branch was never entered.
    assert!(!dir.path().join(editor::QUERY_FILENAME).exists());

    // The same call with a user present does reach the editor, which proves
    // the assertions above are pinned on interactivity and not on some other
    // reason to skip.
    let mut request = ChatRequest::from("piped payload");
    let mut pending_trim = PendingStreamTrim::default();
    let error = Query::default()
        .edit_message(
            &mut request,
            &stream,
            &mut pending_trim,
            true,
            true,
            &config,
            dir.path(),
            &Printer::sink(),
        )
        .unwrap_err();

    assert_matches!(error, Error::Editor(_));
}

#[test]
fn edit_message_rejects_a_forced_editor_when_non_interactive() {
    // `--quote` seeds the request with the assistant's own last message, for
    // the user to reply between the quoted lines. With nobody to compose that
    // reply, sending the seed would submit the assistant's words as the user's
    // turn and persist it, so the run fails instead.
    //
    // `--edit` shares the condition, so it needs no test of its own.
    let mut config = AppConfig::new_test();
    config.editor.cmd = Some(CommandConfigOrString::String(
        "jp-editor-that-does-not-exist".to_owned(),
    ));

    let query = Query {
        quote: Some(true),
        ..Default::default()
    };

    let mut request = ChatRequest::from(" >  quoted reply");
    let stream = ConversationStream::new_test();
    let mut pending_trim = PendingStreamTrim::default();
    let error = query
        .edit_message(
            &mut request,
            &stream,
            &mut pending_trim,
            false,
            false,
            &config,
            Utf8Path::new("/tmp"),
            &Printer::sink(),
        )
        .unwrap_err();

    let Error::NonInteractiveEditor { suggestion } = error else {
        panic!("expected a non-interactive editor error, got: {error:?}");
    };
    assert_eq!(
        suggestion,
        "Run this from a terminal, or pass --no-edit instead to send the text as-is."
    );
    assert_eq!(
        request.content, " >  quoted reply",
        "the seed stays put rather than becoming the turn"
    );
}

#[test]
fn edit_message_forced_editor_without_text_suggests_a_placeholder() {
    // A bare `jp -e`, or `--quote` on a conversation with no assistant message
    // to quote: the editor is forced and the request is empty, so `--no-edit`
    // would synthesize rather than send anything the caller wrote. Advice to
    // send "the text as-is" would name text that does not exist.
    //
    // Passing the query as an argument is not the escape here: `--edit` and
    // `--quote` force the editor regardless of a positional query, so the run
    // would fail the same way again.
    let mut config = AppConfig::new_test();
    config.editor.cmd = Some(CommandConfigOrString::String(
        "jp-editor-that-does-not-exist".to_owned(),
    ));

    let query = Query {
        edit: true,
        ..Default::default()
    };

    let mut request = ChatRequest::default();
    let stream = ConversationStream::new_test();
    let mut pending_trim = PendingStreamTrim::default();
    let error = query
        .edit_message(
            &mut request,
            &stream,
            &mut pending_trim,
            false,
            false,
            &config,
            Utf8Path::new("/tmp"),
            &Printer::sink(),
        )
        .unwrap_err();

    let Error::NonInteractiveEditor { suggestion } = error else {
        panic!("expected a non-interactive editor error, got: {error:?}");
    };
    assert_eq!(
        suggestion,
        "Run this from a terminal, or use --no-edit to send a placeholder message."
    );
}

#[test]
fn edit_message_without_a_query_is_an_error_when_non_interactive() {
    // Nothing to send and no way to compose it: erroring out is the only
    // honest outcome. The run used to hang in the editor instead.
    let dir = Utf8TempDir::new().unwrap();
    let mut config = AppConfig::new_test();
    config.editor.cmd = Some(CommandConfigOrString::String(
        "jp-editor-that-does-not-exist".to_owned(),
    ));

    let mut request = ChatRequest::default();
    let stream = ConversationStream::new_test();
    let mut pending_trim = PendingStreamTrim::default();
    let error = Query::default()
        .edit_message(
            &mut request,
            &stream,
            &mut pending_trim,
            false,
            false,
            &config,
            dir.path(),
            &Printer::sink(),
        )
        .unwrap_err();

    assert_matches!(error, Error::NonInteractiveEditor { .. });
    assert!(!dir.path().join(editor::QUERY_FILENAME).exists());
}

/// A stream whose last assistant message is `message`.
fn stream_with_message(message: &str) -> ConversationStream {
    let mut stream = ConversationStream::new_test();
    stream.start_turn("question");
    stream
        .current_turn_mut()
        .add_chat_response(ChatResponse::message(message))
        .build()
        .unwrap();
    stream
}

/// A stream whose last assistant message is a two-line reply.
fn stream_with_assistant_reply() -> ConversationStream {
    stream_with_message("line one\nline two")
}

/// Count the `TurnStart` events in a stream.
fn turn_start_count(stream: &ConversationStream) -> usize {
    stream.iter().filter(|e| e.is_turn_start()).count()
}

/// Assert that no `TurnStart` is immediately followed by another `TurnStart`
/// (an empty middle turn).
fn assert_no_adjacent_turn_starts(stream: &ConversationStream) {
    let mut previous_was_turn_start = false;
    for e in stream.iter() {
        assert!(
            !(previous_was_turn_start && e.is_turn_start()),
            "stream contains adjacent TurnStart events (empty middle turn)"
        );
        previous_was_turn_start = e.is_turn_start();
    }
}

#[test]
fn pending_trim_replay_removes_stale_turn_start() {
    // Multi-turn conversation: the stale `TurnStart` sits *after* the first
    // `ChatRequest`, where `sanitize`'s `normalize_turn_starts` would not
    // collapse it.
    let mut stream = ConversationStream::new_test();
    stream.start_turn("first question");
    stream
        .current_turn_mut()
        .add_chat_response(ChatResponse::message("first answer"))
        .build()
        .unwrap();
    stream.start_turn("second question");

    let trim = PendingStreamTrim {
        replay_turn: true,
        pop_request: false,
    };
    trim.apply(&mut stream);

    // The replayed request re-enters the stream as a fresh turn.
    stream.start_turn("second question, revised");

    assert_eq!(
        turn_start_count(&stream),
        2,
        "replay must replace the trimmed turn, not open an extra one"
    );
    assert_no_adjacent_turn_starts(&stream);
}

#[test]
fn pending_trim_pop_request_removes_stale_turn_start() {
    // Bare `--no-edit` replay: the last turn holds only its request.
    let mut stream = ConversationStream::new_test();
    stream.start_turn("first question");
    stream
        .current_turn_mut()
        .add_chat_response(ChatResponse::message("first answer"))
        .build()
        .unwrap();
    stream.start_turn("replayed question");

    let trim = PendingStreamTrim {
        replay_turn: false,
        pop_request: true,
    };
    trim.apply(&mut stream);

    stream.start_turn("replayed question");

    assert_eq!(
        turn_start_count(&stream),
        2,
        "pop_request must replace the trimmed turn, not open an extra one"
    );
    assert_no_adjacent_turn_starts(&stream);
}

/// `--replay` on a turn holding a chat response this build cannot read (one a
/// newer jp wrote) removes it with the rest of the turn, instead of leaving it
/// behind to be read as part of the turn before.
#[test]
fn pending_trim_replay_removes_an_unreadable_chat_response_with_its_turn() {
    let mut stream = ConversationStream::new_test();
    stream.start_turn("first question");
    stream.push_event(ConversationEvent::now(ChatResponse::message(
        "first answer",
    )));
    stream.start_turn("second question");
    stream.push_event(ConversationEvent::now(ChatResponse::message(
        "second answer",
    )));

    let (base_config, mut events) = stream.to_parts().unwrap();
    let answer = events
        .iter()
        .position(|e| e.get("message").and_then(Value::as_str) == Some("second answer"))
        .unwrap();
    events.insert(
        answer,
        serde_json::json!({
            "type": "chat_response",
            "timestamp": "2025-01-01 00:00:00.0",
            "commentary": "Checking the second question."
        }),
    );
    let mut stream =
        ConversationStream::from_parts(base_config, events, &PartialAppConfig::empty()).unwrap();

    PendingStreamTrim {
        replay_turn: true,
        pop_request: false,
    }
    .apply(&mut stream);
    stream.start_turn("second question, revised");

    let (_, events) = stream.to_parts().unwrap();
    assert!(
        !events.iter().any(|e| e.get("commentary").is_some()),
        "the replayed turn's unreadable chat response survived: {events:#?}"
    );
    assert_eq!(turn_start_count(&stream), 2);
}

fn turn_summaries(stream: &ConversationStream) -> Vec<Option<String>> {
    stream
        .iter_turns()
        .map(|turn| turn.summary().map(str::to_owned))
        .collect()
}

fn summarize_turns(stream: &mut ConversationStream, summaries: &[&str]) {
    let turn_ids: Vec<_> = stream
        .iter_turns()
        .filter_map(|turn| turn.id().cloned())
        .collect();

    for (turn_id, summary) in turn_ids.iter().zip(summaries) {
        assert!(stream.set_turn_summary(turn_id, *summary));
    }
}

/// `--replay` replays the whole last turn, including a reply sent while it was
/// interrupted, so the turn goes with its summary and the turn before keeps its
/// own.
#[test]
fn pending_trim_replay_removes_a_turn_holding_an_interrupt_reply() {
    let mut stream = ConversationStream::new_test();
    stream.start_turn("Set up the project");
    stream
        .current_turn_mut()
        .add_chat_response(ChatResponse::message("Done."))
        .build()
        .unwrap();
    stream.start_turn("Add retries to the stream handler");
    stream
        .current_turn_mut()
        .add_chat_response(ChatResponse::message("I'll retry every second"))
        .add_chat_request(ChatRequest::from("Use exponential backoff instead."))
        .add_chat_response(ChatResponse::message("Done. Retries now back off."))
        .build()
        .unwrap();
    summarize_turns(&mut stream, &[
        "Set up the project",
        "Switch stream retries to exponential backoff",
    ]);

    PendingStreamTrim {
        replay_turn: true,
        pop_request: false,
    }
    .apply(&mut stream);

    assert_eq!(turn_summaries(&stream), [Some(
        "Set up the project".to_owned()
    )]);
}

#[test]
fn pending_trim_replay_of_a_whole_turn_keeps_the_summary_before_it() {
    let mut stream = ConversationStream::new_test();
    stream.start_turn("Set up the project");
    stream
        .current_turn_mut()
        .add_chat_response(ChatResponse::message("Done."))
        .build()
        .unwrap();
    stream.start_turn("Add retries to the stream handler");
    stream
        .current_turn_mut()
        .add_chat_response(ChatResponse::message("Added."))
        .build()
        .unwrap();
    summarize_turns(&mut stream, &["Set up the project", "Add stream retries"]);

    PendingStreamTrim {
        replay_turn: true,
        pop_request: false,
    }
    .apply(&mut stream);

    assert_eq!(turn_summaries(&stream), [Some(
        "Set up the project".to_owned()
    )]);
}

/// Bare `--no-edit` popping a turn's only request empties the turn, which goes
/// with its summary, and leaves the summary of the turn before it alone.
#[test]
fn pending_trim_pop_request_of_a_turns_only_request_keeps_the_summary_before_it() {
    let mut stream = ConversationStream::new_test();
    stream.start_turn("Set up the project");
    stream
        .current_turn_mut()
        .add_chat_response(ChatResponse::message("Done."))
        .build()
        .unwrap();
    stream.start_turn("Add retries to the stream handler");
    summarize_turns(&mut stream, &["Set up the project", "Add stream retries"]);

    PendingStreamTrim {
        replay_turn: false,
        pop_request: true,
    }
    .apply(&mut stream);

    assert_eq!(turn_summaries(&stream), [Some(
        "Set up the project".to_owned()
    )]);
}

#[test]
fn pending_trim_pop_request_clears_the_summary_of_a_turn_it_shortens() {
    let mut stream = ConversationStream::new_test();
    stream.start_turn("Add retries to the stream handler");
    stream
        .current_turn_mut()
        .add_chat_response(ChatResponse::message("I'll retry every second"))
        .add_chat_request(ChatRequest::from("Use exponential backoff instead."))
        .build()
        .unwrap();
    summarize_turns(&mut stream, &["Add stream retries"]);

    PendingStreamTrim {
        replay_turn: false,
        pop_request: true,
    }
    .apply(&mut stream);

    assert_eq!(turn_summaries(&stream), [None]);
}

#[test]
fn pending_trim_default_is_noop() {
    let mut stream = ConversationStream::new_test();
    stream.start_turn("question");
    let before = stream.len();

    PendingStreamTrim::default().apply(&mut stream);

    assert_eq!(
        stream.len(),
        before,
        "a default PendingStreamTrim must not mutate the stream"
    );
}

/// Read `jp query --no-edit <args>` through the real command line and settle it
/// with no tools configured and nobody to ask.
fn parse_query(args: &[&str]) -> std::result::Result<Query, clap::Error> {
    let argv = ["jp", "query", "--no-edit"]
        .into_iter()
        .chain(args.iter().copied());
    let Commands::Query(query) = Cli::try_parse_from(argv)?.command else {
        panic!("`jp query` parses as a query");
    };
    let query: Query<Parsed> = query;

    Ok(query
        .settle(&PartialAppConfig::default(), &ArgPrompts::default())
        .expect("settles without asking"))
}

/// Build the request for `args`, with no piped stdin and an empty stream.
///
/// Runs the same two steps `run` does: resolve the query, then compose it.
fn built_request(args: &[&str]) -> String {
    built_request_against(args, &ConversationStream::new_test())
}

/// Build the request for `args` against `stream`, with no piped stdin.
fn built_request_against(args: &[&str], stream: &ConversationStream) -> String {
    let query = parse_query(args).unwrap();
    let resolved = query.resolve_query().unwrap();

    query
        .build_conversation(
            "",
            resolved.as_deref(),
            stream,
            true,
            &AppConfig::new_test(),
            Utf8Path::new("/tmp"),
            &Printer::sink(),
        )
        .unwrap()
        .chat_request
        .expect("non-empty request")
        .content
}

#[test]
fn build_conversation_seeds_a_verbatim_quote_above_the_query() {
    let built = built_request_against(
        &["--quote=false", "and", "what", "about", "X?"],
        &stream_with_assistant_reply(),
    );

    assert_eq!(built, "line one line two\n\nand what about X?");
}

#[test]
fn build_conversation_seeds_a_blockquoted_quote_above_the_query() {
    let built = built_request_against(
        &["--quote", "and", "what", "about", "X?"],
        &stream_with_assistant_reply(),
    );

    assert_eq!(built, "> line one line two\n\nand what about X?");
}

#[test]
fn build_conversation_reads_single_at_path_query() {
    let dir = camino_tempfile::tempdir().unwrap();
    let path = dir.path().join("notes.md");
    std::fs::write(&path, "# Notes\n\nbody\n").unwrap();

    assert_eq!(
        built_request(&[format!("@{path}").as_str()]),
        "# Notes\n\nbody\n"
    );
}

// A bare `@` is the whole query: there is no path after the sigil, so it is
// text. Reading it as the empty path is what aborted the query.
#[test]
fn build_conversation_keeps_lone_bare_sigil() {
    assert_eq!(built_request(&["@"]), "@");
}

// The reported failure: a bare `@` mid-sentence was read as the empty path and
// aborted the query before it was ever sent.
#[test]
fn build_conversation_keeps_bare_sigil_in_prose() {
    assert_eq!(
        built_request(&["we", "should", "drop", "the", "@", "entirely"]),
        "we should drop the @ entirely"
    );
}

// An `@path` that names a real file is still ordinary text when it is one word
// of a longer query: only a single-value query reads from disk.
#[test]
fn build_conversation_keeps_at_path_word_in_multi_word_query() {
    let dir = camino_tempfile::tempdir().unwrap();
    let path = dir.path().join("notes.md");
    std::fs::write(&path, "file contents").unwrap();

    assert_eq!(
        built_request(&["see", format!("@{path}").as_str()]),
        format!("see @{path}")
    );
    // Leading position too: it is the value count that decides, not where the
    // sigil sits.
    assert_eq!(
        built_request(&[format!("@{path}").as_str(), "is", "stale"]),
        format!("@{path} is stale")
    );
}

#[test]
fn build_conversation_prepends_query_to_piped_stdin() {
    let built = parse_query(&["look", "at", "this"])
        .unwrap()
        .build_conversation(
            "piped payload",
            Some("look at this"),
            &ConversationStream::new_test(),
            true,
            &AppConfig::new_test(),
            Utf8Path::new("/tmp"),
            &Printer::sink(),
        )
        .unwrap();

    assert_eq!(
        built.chat_request.unwrap().content,
        "look at this\n\npiped payload"
    );
}
