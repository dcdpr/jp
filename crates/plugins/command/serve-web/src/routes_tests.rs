use axum::http::{HeaderMap, HeaderName};
use serde_json::json;

use super::*;

/// A request carrying exactly these headers and nothing else.
fn headers(pairs: &[(&str, &str)]) -> HeaderMap {
    let mut headers = HeaderMap::new();

    for (name, value) in pairs {
        headers.insert(
            HeaderName::from_bytes(name.as_bytes()).expect("a valid header name"),
            value.parse().expect("a valid header value"),
        );
    }

    headers
}

#[test]
fn a_page_posting_to_its_own_server_is_allowed() {
    assert!(same_origin(&headers(&[
        ("sec-fetch-site", "same-origin"),
        ("origin", "http://127.0.0.1:3000"),
        ("host", "127.0.0.1:3000"),
    ])));
}

/// The case the check exists for.
///
/// A form post needs no preflight, so a page on any site can submit one to this
/// server.
/// Refusing it is the only thing that stops a site the reader happens to visit
/// from starting a turn on their machine.
#[test]
fn a_form_post_from_another_site_is_refused() {
    assert!(!same_origin(&headers(&[
        ("sec-fetch-site", "cross-site"),
        ("origin", "https://example.test"),
        ("host", "127.0.0.1:3000"),
    ])));
}

/// Typing the address, or following a bookmark.
/// There is no page behind it to be acting on anyone's behalf.
#[test]
fn a_navigation_with_no_origin_behind_it_is_allowed() {
    assert!(same_origin(&headers(&[("sec-fetch-site", "none")])));
}

/// A browser too old to send `Sec-Fetch-Site` still sends `Origin`, and the
/// authority it names is either this server or somebody else.
#[test]
fn an_origin_that_is_not_this_server_is_refused() {
    assert!(!same_origin(&headers(&[
        ("origin", "https://example.test"),
        ("host", "127.0.0.1:3000"),
    ])));
}

#[test]
fn an_origin_matching_the_host_is_allowed() {
    assert!(same_origin(&headers(&[
        ("origin", "http://localhost:3000"),
        ("host", "localhost:3000"),
    ])));
}

/// `curl`, a script, another tool.
/// No browser can be made to omit both headers, so refusing here would lock out
/// every non-browser caller and stop nothing.
#[test]
fn a_caller_that_sends_neither_header_is_allowed() {
    assert!(same_origin(&headers(&[("host", "127.0.0.1:3000")])));
}

#[test]
fn a_caller_holding_the_whole_transcript_is_sent_nothing() {
    // `from == total` is what the handler reads as "nothing to say".
    assert_eq!(answer_from(Some(4), 4, 4, false), 4);
}

/// The regression this guards: an entry that changes in place.
///
/// A tool call is rendered when it is requested and gains its result later, and
/// consecutive assistant text renders as one block that grows.
/// Either way the count stays where it was, so a caller that trusts the count
/// holds the first version forever — no later poll corrects it, because by
/// then the count has moved past the entry that changed.
#[test]
fn an_unsettled_tail_is_resent_to_a_caller_that_already_has_it() {
    assert_eq!(answer_from(Some(4), 4, 4, true), 3);
}

#[test]
fn a_settled_tail_leaves_an_up_to_date_caller_alone() {
    assert_eq!(answer_from(Some(4), 4, 4, false), 4);
}

/// Waiting for the first token is the longest stretch of a turn, and the
/// transcript ends in the request for all of it.
/// Re-sending a request that cannot change would rebuild that entry once a
/// second for nothing.
#[test]
fn a_request_at_the_end_of_the_transcript_is_settled() {
    let waiting = [json!({"type": "chat_request", "content": "go on then"})];

    assert!(!render::tail_can_change(&render::render_events(&waiting)));
}

#[test]
fn a_tool_call_and_a_block_of_text_are_both_unsettled() {
    let called = [
        json!({"type": "chat_request", "content": "run it"}),
        json!({"type": "tool_call_request", "id": "t1", "name": "ls", "arguments": {}}),
    ];

    assert!(render::tail_can_change(&render::render_events(&called)));

    let answering = [
        json!({"type": "chat_request", "content": "run it"}),
        json!({"type": "chat_response", "message": "here is"}),
    ];

    assert!(render::tail_can_change(&render::render_events(&answering)));
}

/// An unsettled entry wins over the newest one: a tool call still waiting on
/// its result is where the answer has to start, however much came after it.
#[test]
fn an_unsettled_entry_is_resent_from_where_it_starts() {
    assert_eq!(answer_from(Some(9), 9, 4, true), 4);
}

/// A count past the end means the transcript was rewritten underneath the
/// caller — compacted, or edited on disk — so the only safe answer is the
/// tail.
#[test]
fn a_count_beyond_the_end_falls_back_to_the_tail() {
    assert_eq!(answer_from(Some(500), 300, 300, false), 100);
}

#[test]
fn a_caller_that_says_nothing_is_sent_the_tail() {
    assert_eq!(answer_from(None, 300, 300, false), 100);
}

#[test]
fn an_empty_transcript_has_nothing_to_resend() {
    assert_eq!(answer_from(Some(0), 0, 0, true), 0);
}

/// The same thing end to end, against what the renderer actually produces.
///
/// The count is identical before and after the result arrives, because
/// `tool_call_response` renders into the call it answers rather than beside it.
/// A caller that counted the unanswered call is therefore up to date by the
/// only measure it has, and still holding a tool call with no result.
#[test]
fn a_tool_result_reaches_a_caller_that_already_counted_the_call() {
    let asked = [
        json!({"type": "chat_request", "content": "run it"}),
        json!({"type": "tool_call_request", "id": "t1", "name": "ls", "arguments": {}}),
    ];

    let held = render::render_events(&asked).len();
    assert_eq!(held, 2);

    let answered = [
        json!({"type": "chat_request", "content": "run it"}),
        json!({"type": "tool_call_request", "id": "t1", "name": "ls", "arguments": {}}),
        json!({"type": "tool_call_response", "id": "t1", "content": "a.txt"}),
    ];

    let rendered = render::render_events(&answered);
    assert_eq!(
        rendered.len(),
        held,
        "the result renders into the call, so the count cannot report it"
    );

    let from = answer_from(
        Some(held),
        rendered.len(),
        render::settled_upto(&rendered),
        render::tail_can_change(&rendered),
    );

    assert_eq!(
        from, 1,
        "the answer starts at the tool call, so its result reaches the caller"
    );

    let html = views::detail::messages(&rendered[from..]).into_string();
    assert!(
        html.contains("a.txt"),
        "what is sent carries the result: {html}"
    );
}
