use axum::http::{HeaderMap, HeaderName};
use pretty_assertions::assert_eq;
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

/// A caller that says nothing about what it holds gets the tail.
#[test]
fn resend_from_gives_the_tail_to_a_caller_that_holds_nothing() {
    assert_eq!(resend_from(500, None, None, 500, false), 300);
}

/// A short conversation fits in one window, so the tail is the whole thing.
#[test]
fn resend_from_gives_the_whole_transcript_when_it_fits() {
    assert_eq!(resend_from(12, None, None, 12, false), 0);
}

/// Nothing to send when the caller is up to date and nothing is in flight.
///
/// `from == total` is what the handler reads as "nothing to say".
#[test]
fn resend_from_sends_nothing_to_an_up_to_date_caller() {
    assert_eq!(resend_from(13, Some(13), Some(13), 13, false), 13);
}

/// A count past the end means the transcript was rewritten under the caller, so
/// the tail is the only safe answer.
#[test]
fn resend_from_ignores_a_count_past_the_end() {
    assert_eq!(resend_from(400, Some(900), None, 400, false), 200);
}

/// The tool call the caller is waiting on is sent again, so its result reaches
/// the page.
#[test]
fn resend_from_reaches_back_to_the_call_the_caller_is_waiting_on() {
    // Thirteen events, the caller holds all of them, and the call at 11 has no
    // result yet.
    assert_eq!(resend_from(13, Some(13), Some(11), 11, false), 11);
}

/// A tool call that gained its result between two polls is sent again.
///
/// By then the server's own boundary has moved past it — the next call in the
/// batch is the one waiting — so the caller's floor is what reaches back for
/// the entry that changed.
#[test]
fn resend_from_reaches_back_to_a_call_that_has_since_resolved() {
    // The caller was last told that everything from 10 was provisional. Since
    // then the call at 10 resolved and the one at 11 is the first still waiting.
    assert_eq!(resend_from(13, Some(13), Some(10), 11, false), 10);
}

/// A floor above what the caller holds cannot reach past the end of its copy.
#[test]
fn resend_from_never_sends_past_what_the_caller_holds() {
    assert_eq!(resend_from(20, Some(11), Some(30), 20, false), 11);
}

/// A boundary that moved backwards is honoured over a stale floor.
///
/// Compaction rewrites the transcript, which can leave an entry the caller
/// believes final provisional again.
#[test]
fn resend_from_honours_a_boundary_below_the_callers_floor() {
    assert_eq!(resend_from(30, Some(30), Some(20), 8, false), 8);
}

/// The regression the tail rule guards: an entry that changes in place while
/// nothing after it is provisional.
///
/// Consecutive assistant text renders as one block that grows, and it is the
/// last entry — so the settled boundary sits at the end and neither it nor the
/// caller's floor reaches back for it.
/// The count stays where it was too, so a caller that trusts either holds the
/// first version forever.
#[test]
fn an_unsettled_tail_is_resent_to_a_caller_that_already_has_it() {
    assert_eq!(resend_from(4, Some(4), Some(4), 4, true), 3);
}

#[test]
fn a_settled_tail_leaves_an_up_to_date_caller_alone() {
    assert_eq!(resend_from(4, Some(4), Some(4), 4, false), 4);
}

/// An unsettled entry wins over the newest one: a tool call still waiting on
/// its result is where the answer has to start, however much came after it.
#[test]
fn an_unsettled_entry_is_resent_from_where_it_starts() {
    assert_eq!(resend_from(9, Some(9), Some(9), 4, true), 4);
}

#[test]
fn an_empty_transcript_has_nothing_to_resend() {
    assert_eq!(resend_from(0, Some(0), Some(0), 0, true), 0);
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

    let asked = render::render_events(&asked);
    let held = asked.len();
    assert_eq!(held, 2);

    // What the caller was told on the poll that delivered the unanswered call.
    let floor = render::settled_upto(&asked);
    assert_eq!(floor, 1);

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

    let from = resend_from(
        rendered.len(),
        Some(held),
        Some(floor),
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

/// Both shapes of choice reach the host as `--cfg` arguments, assignments last.
///
/// A value typed into the form is the more specific of the two, so it is
/// applied over the same key set by a configuration named beside it.
#[test]
fn a_turn_form_orders_assignments_after_named_configurations() {
    let form = TurnForm::parse(
        "content=go&cfg=personas%2Fdev&cfg_key=assistant.model.id&cfg_value=opus&cfg=skill%2Frfd",
    );

    assert_eq!(form.content, "go");
    assert_eq!(form.cfg.args(), [
        "personas/dev",
        "skill/rfd",
        "assistant.model.id=opus"
    ]);
}

/// The form always carries a spare row, which is not an assignment.
#[test]
fn a_turn_form_drops_a_row_with_no_key() {
    let form = TurnForm::parse("content=go&cfg_key=&cfg_value=&cfg_key=+&cfg_value=stray");

    assert!(form.cfg.args().is_empty());
}

/// Space around an assignment is the keyboard's, not the reader's.
#[test]
fn a_turn_form_trims_an_assignment() {
    let form = TurnForm::parse("content=go&cfg_key=+assistant.name+&cfg_value=+JP+");

    assert_eq!(form.cfg.args(), ["assistant.name=JP"]);
}

/// The new-conversation form reads the same fields as the composer.
#[test]
fn the_new_conversation_form_reads_the_same_configuration_fields() {
    let form = NewConversationForm::parse(
        "title=Spike&content=go&cfg=personas%2Fdev&cfg_key=assistant.name&cfg_value=JP",
    );

    assert_eq!(form.title, "Spike");
    assert_eq!(form.cfg.names, ["personas/dev"]);
    assert_eq!(form.cfg.pairs(), [(
        "assistant.name".to_owned(),
        "JP".to_owned()
    )]);
}
