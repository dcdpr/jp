use jp_conversation::event::ChatResponse;

use super::*;

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

#[test]
fn quote_false_seeds_the_message_verbatim() {
    let config = AppConfig::new_test();
    let mut request = ChatRequest::default();
    assert!(seed_quoted_reply(
        &mut request,
        &stream_with_assistant_reply(),
        false,
        &config
    ));

    // The trailing blank line separates the seed from the reply the user is
    // about to type below it. The two source lines are joined by a soft
    // break, so reformatting flows them into a single wrapped line.
    assert_eq!(request.content, "line one line two\n\n");
}

#[test]
fn quote_true_seeds_the_message_as_a_blockquote() {
    let config = AppConfig::new_test();
    let mut request = ChatRequest::default();
    assert!(seed_quoted_reply(
        &mut request,
        &stream_with_assistant_reply(),
        true,
        &config
    ));

    assert_eq!(request.content, "> line one line two\n\n");
}

#[test]
fn quote_seeds_above_an_already_composed_request() {
    let config = AppConfig::new_test();
    let mut request = ChatRequest::from("and what about X?");
    assert!(seed_quoted_reply(
        &mut request,
        &stream_with_assistant_reply(),
        false,
        &config
    ));

    assert_eq!(request.content, "line one line two\n\nand what about X?");
}

#[test]
fn quote_leaves_the_request_untouched_without_an_assistant_message() {
    let config = AppConfig::new_test();
    let mut request = ChatRequest::from("only my words");
    assert!(!seed_quoted_reply(
        &mut request,
        &ConversationStream::new_test(),
        true,
        &config
    ));

    assert_eq!(request.content, "only my words");
}

#[test]
fn blockquote_prefixes_each_line() {
    assert_eq!(blockquote("hello"), "> hello");
    assert_eq!(blockquote("a\nb"), "> a\n> b");
    assert_eq!(blockquote("a\nb\nc"), "> a\n> b\n> c");
}

#[test]
fn blockquote_keeps_paragraph_breaks_with_bare_marker() {
    // Markdown continues a blockquote across a `>` line; an unprefixed
    // blank line would terminate it. The bare `>` (no trailing space)
    // also avoids editor trailing-whitespace warnings.
    assert_eq!(blockquote("a\n\nb"), "> a\n>\n> b");
}

#[test]
fn blockquote_trailing_newline_is_dropped_by_lines() {
    // `str::lines` drops the trailing terminator, so a string with and
    // without a trailing newline produce identical quotes.
    assert_eq!(blockquote("a\nb\n"), "> a\n> b");
}

#[test]
fn quote_wraps_a_long_paragraph_to_the_configured_width() {
    // The raw assistant message has no line breaks; the seed must be
    // rewrapped, not quoted verbatim, or a long reply would produce one
    // unreadable blockquote line.
    let mut config = AppConfig::new_test();
    config.style.markdown.wrap_width = 20;

    let stream = stream_with_message("alpha bravo charlie delta echo foxtrot");

    let mut request = ChatRequest::default();
    assert!(seed_quoted_reply(&mut request, &stream, true, &config));

    assert_eq!(
        request.content,
        "> alpha bravo charlie\n> delta echo foxtrot\n\n"
    );
}

#[test]
fn quote_aligns_table_columns() {
    // The source table's pipes don't line up ("1" vs "two"); the seed must
    // reformat it into a properly padded table rather than copying the
    // ragged source through.
    let config = AppConfig::new_test();

    let stream = stream_with_message("| A | B |\n| --- | --- |\n| 1 | two |\n");

    let mut request = ChatRequest::default();
    assert!(seed_quoted_reply(&mut request, &stream, false, &config));

    assert_eq!(
        request.content,
        "| A   | B   |\n|-----|-----|\n| 1   | two |\n\n"
    );
}

#[test]
fn quote_keeps_a_table_header_wider_than_the_display_cap() {
    // On screen a header past `table_max_column_width` is cut short with an
    // ellipsis. The seed is the text of the user's next request, so the cut
    // would send the model a column name that never existed.
    let config = AppConfig::new_test();
    assert_eq!(config.style.markdown.table_max_column_width, 40);

    let stream = stream_with_message(
        "| Execution time before optimization (milliseconds) | Result |\n| --- | --- |\n| 12 | ok \
         |\n",
    );

    let mut request = ChatRequest::default();
    assert!(seed_quoted_reply(&mut request, &stream, false, &config));

    assert_eq!(
        request.content,
        "| Execution time before optimization (milliseconds) | Result |\n|---------------------------------------------------|--------|\n| 12                                                | ok     |\n\n"
    );
}

#[test]
fn quote_keeps_a_wide_body_cell_on_one_row() {
    // A body cell past the cap is wrapped onto continuation lines for display.
    // Re-parsed as markdown those lines are extra rows, so the table the model
    // receives has a different shape than the one it wrote.
    let config = AppConfig::new_test();

    let stream = stream_with_message(
        "| Step | Detail |\n| --- | --- |\n| one | the quick brown fox jumps over the lazy dog \
         twice |\n",
    );

    let mut request = ChatRequest::default();
    assert!(seed_quoted_reply(&mut request, &stream, false, &config));

    assert_eq!(
        request.content,
        "| Step | Detail                                            \
         |\n|------|---------------------------------------------------|\n| one  | the quick \
         brown fox jumps over the lazy dog twice |\n\n"
    );
}

#[test]
fn quote_keeps_consecutive_spaces_in_inline_code() {
    // Spaces inside a code span are content: quoting `grep 'a  b'` back with
    // one space asks the model about a different command.
    let config = AppConfig::new_test();

    let stream = stream_with_message("Run `grep 'a  b' file` next.");

    let mut request = ChatRequest::default();
    assert!(seed_quoted_reply(&mut request, &stream, true, &config));

    assert_eq!(request.content, "> Run `grep 'a  b' file` next.\n\n");
}

#[test]
fn last_assistant_message_returns_most_recent_message() {
    let mut stream = ConversationStream::new_test();
    stream.start_turn("first question");
    stream
        .current_turn_mut()
        .add_chat_response(ChatResponse::message("first answer"))
        .build()
        .unwrap();
    stream.start_turn("second question");
    stream
        .current_turn_mut()
        .add_chat_response(ChatResponse::message("second answer"))
        .build()
        .unwrap();

    assert_eq!(last_assistant_message(&stream), Some("second answer"));
}

#[test]
fn last_assistant_message_skips_reasoning_after_message() {
    let mut stream = ConversationStream::new_test();
    stream.start_turn("question");
    stream
        .current_turn_mut()
        .add_chat_response(ChatResponse::message("the answer"))
        .add_chat_response(ChatResponse::reasoning("thinking after"))
        .build()
        .unwrap();

    // Reasoning is the most recent ChatResponse, but --quote wants the
    // assistant's spoken text, so the message wins.
    assert_eq!(last_assistant_message(&stream), Some("the answer"));
}

#[test]
fn last_assistant_message_returns_none_for_empty_stream() {
    let stream = ConversationStream::new_test();
    assert_eq!(last_assistant_message(&stream), None);
}

#[test]
fn last_assistant_message_returns_none_when_only_reasoning_present() {
    let mut stream = ConversationStream::new_test();
    stream.start_turn("question");
    stream
        .current_turn_mut()
        .add_chat_response(ChatResponse::reasoning("only thinking, no message yet"))
        .build()
        .unwrap();

    assert_eq!(last_assistant_message(&stream), None);
}
