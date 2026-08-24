use std::time::Duration;

use camino_tempfile::tempdir;
use chrono::{TimeZone as _, Utc};
use clap::Parser as _;
use jp_config::AppConfig;
use jp_conversation::{
    Conversation, ConversationEvent, ConversationId,
    event::{ChatRequest, ChatResponse, ToolCallRequest, ToolCallResponse, TurnStart},
};
use jp_printer::{OutputFormat, OutputWidth, Printer, SharedBuffer};
use jp_workspace::Workspace;
use serde_json::{Map, Value};
use strip_ansi_escapes::strip_str;

use super::*;
use crate::{
    Globals,
    cmd::{conversation_id::FlagIds, target::ConversationTarget},
};

/// A fixed timestamp for events whose time is irrelevant to the assertion.
fn ts() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2020, 1, 1, 0, 0, 0).unwrap()
}

fn make_id(secs: u64) -> ConversationId {
    ConversationId::try_from(chrono::DateTime::<Utc>::UNIX_EPOCH + Duration::from_secs(secs))
        .unwrap()
}

/// Output lines with ANSI styling removed.
fn lines(output: &str) -> Vec<String> {
    output.trim_end().lines().map(strip_str).collect()
}

/// Plain-text context: no ANSI, no heading, no width limit — the shape a pipe
/// sees.
fn setup(events: Vec<(ConversationId, Vec<ConversationEvent>)>) -> (Ctx, SharedBuffer) {
    setup_with(events, OutputFormat::Text, OutputWidth::Unknown)
}

/// Pretty context with a known terminal width — the shape a terminal sees.
fn setup_pretty(
    events: Vec<(ConversationId, Vec<ConversationEvent>)>,
    width: u16,
) -> (Ctx, SharedBuffer) {
    setup_with(
        events,
        OutputFormat::TextPretty,
        OutputWidth::Terminal(width),
    )
}

fn setup_json(events: Vec<(ConversationId, Vec<ConversationEvent>)>) -> (Ctx, SharedBuffer) {
    setup_with(events, OutputFormat::Json, OutputWidth::Unknown)
}

fn setup_with(
    events: Vec<(ConversationId, Vec<ConversationEvent>)>,
    format: OutputFormat,
    width: OutputWidth,
) -> (Ctx, SharedBuffer) {
    let entries = events
        .into_iter()
        .map(|(id, evts)| (id, Conversation::default(), evts))
        .collect();
    setup_conversations_with(entries, format, width)
}

fn setup_conversations(
    entries: Vec<(ConversationId, Conversation, Vec<ConversationEvent>)>,
) -> (Ctx, SharedBuffer) {
    setup_conversations_with(entries, OutputFormat::Text, OutputWidth::Unknown)
}

fn setup_conversations_with(
    entries: Vec<(ConversationId, Conversation, Vec<ConversationEvent>)>,
    format: OutputFormat,
    width: OutputWidth,
) -> (Ctx, SharedBuffer) {
    let tmp = tempdir().unwrap();
    let config = AppConfig::new_test();
    let workspace = Workspace::in_memory(tmp.path());
    let (printer, out, _err) = Printer::memory(format);
    let printer = printer.with_output_width(width);
    let mut ctx = Ctx::new(
        workspace,
        None,
        tokio::runtime::Runtime::new().unwrap(),
        Globals::default(),
        config,
        None,
        printer,
    );

    for (id, conversation, evts) in entries {
        ctx.workspace
            .create_conversation_with_id(id, conversation, ctx.config());
        let h = ctx.workspace.acquire_conversation(&id).unwrap();
        let lock = ctx.workspace.test_lock(h);
        lock.as_mut().update_events(|e| e.extend(evts));
    }

    (ctx, out)
}

/// Run a grep and return its ANSI-stripped output lines.
fn run(grep: Grep, ctx: &mut Ctx, out: &SharedBuffer) -> Vec<String> {
    grep.run(ctx, vec![]).unwrap();
    ctx.printer.flush();
    lines(&out.lock().clone())
}

fn grep(pattern: &str) -> Grep {
    Grep {
        pattern: vec![pattern.to_owned()],
        ..Default::default()
    }
}

/// A single-turn conversation: a `TurnStart` followed by `events`.
fn turn(events: Vec<ConversationEvent>) -> Vec<ConversationEvent> {
    let mut out = vec![ConversationEvent::new(TurnStart, ts())];
    out.extend(events);
    out
}

// --- coordinates ------------------------------------------------------------

#[test]
fn line_mode_emits_id_turn_scope_text() {
    let id = make_id(1000);
    let (mut ctx, out) = setup(vec![(
        id,
        turn(vec![ConversationEvent::new(
            ChatRequest::from("tell me about Rust generics"),
            ts(),
        )]),
    )]);

    assert_eq!(run(grep("generics"), &mut ctx, &out), [format!(
        "{id}:1:user:m:tell me about Rust generics"
    )]);
}

#[test]
fn turn_numbers_are_one_based_and_track_the_turn_the_line_came_from() {
    // The coordinate has to name the turn `jp c print --turn N` would show, so
    // the third turn reports 3 rather than its event's position in the stream.
    let id = make_id(1100);
    let mut events = vec![];
    for n in 1..=3 {
        events.extend(turn(vec![ConversationEvent::new(
            ChatRequest::from(format!("phi-mark in turn {n}").as_str()),
            ts(),
        )]));
    }
    let (mut ctx, out) = setup(vec![(id, events)]);

    assert_eq!(run(grep("phi-mark"), &mut ctx, &out), [
        format!("{id}:1:user:m:phi-mark in turn 1"),
        format!("{id}:2:user:m:phi-mark in turn 2"),
        format!("{id}:3:user:m:phi-mark in turn 3"),
    ]);
}

#[test]
fn title_hits_report_the_whole_conversation_range() {
    // A title isn't turn-scoped. `..` is what `--turn` accepts for "all turns",
    // so the coordinate stays usable without a special case.
    let id = make_id(1200);
    let conv = Conversation {
        title: Some("chi-mark in the title".into()),
        ..Default::default()
    };
    let (mut ctx, out) = setup_conversations(vec![(id, conv, vec![])]);

    assert_eq!(run(grep("chi-mark"), &mut ctx, &out), [format!(
        "{id}:..:title:m:chi-mark in the title"
    )]);
}

#[test]
fn context_lines_are_marked_by_the_kind_field() {
    // The marker is a field, not a delimiter: putting it in a separator would
    // mean a script had to know a line's kind before it could find the field
    // boundaries, and had to parse the fields to learn the kind.
    let id = make_id(1300);
    let (mut ctx, out) = setup(vec![(
        id,
        turn(vec![ConversationEvent::new(
            ChatRequest::from("before\npsi-mark\nafter"),
            ts(),
        )]),
    )]);

    let grep = Grep {
        context: 1,
        ..grep("psi-mark")
    };
    assert_eq!(run(grep, &mut ctx, &out), [
        format!("{id}:1:user:c:before"),
        format!("{id}:1:user:m:psi-mark"),
        format!("{id}:1:user:c:after"),
    ]);
}

// --- heading mode -----------------------------------------------------------

#[test]
fn heading_mode_groups_hits_under_the_conversation() {
    let id = make_id(2000);
    let conv = Conversation {
        title: Some("Investigating a flaky test".into()),
        ..Default::default()
    };
    let (mut ctx, out) = setup_conversations_with(
        vec![(
            id,
            conv,
            turn(vec![
                ConversationEvent::new(ChatRequest::from("omega-mark asked"), ts()),
                ConversationEvent::new(ChatResponse::message("omega-mark answered"), ts()),
            ]),
        )],
        OutputFormat::TextPretty,
        OutputWidth::Terminal(80),
    );

    let rendered = run(grep("omega-mark"), &mut ctx, &out);
    assert_eq!(rendered, [
        format!(
            "{id}  Investigating a flaky test{:25}2 matches · 1 turn",
            ""
        ),
        // The scope field is padded to the widest in the group, so the text
        // starts in the same column on both rows.
        "  1:     user:omega-mark asked".to_owned(),
        "  1:assistant:omega-mark answered".to_owned(),
    ]);
    // The stats sit flush against the right margin.
    assert_eq!(display_width(&rendered[0]), 80);
}

#[test]
fn heading_mode_context_rows_are_blank_where_the_coordinate_would_be() {
    // A context row's turn and scope are its match's — every hit in a block comes
    // from one event — so repeating them down the block is noise that buries the
    // match. Blanked and padded instead, a visible coordinate marks the match on
    // its own.
    let id = make_id(2060);
    let (mut ctx, out) = setup_pretty(
        vec![(
            id,
            turn(vec![ConversationEvent::new(
                ChatRequest::from("before\nupsilon-mark\nafter"),
                ts(),
            )]),
        )],
        80,
    );

    let grep = Grep {
        context: 1,
        ..grep("upsilon-mark")
    };
    let rendered = run(grep, &mut ctx, &out);

    // `  1:user:` is nine columns, so a context row carries nine spaces.
    assert_eq!(&rendered[1..], [
        "         before",
        "  1:user:upsilon-mark",
        "         after",
    ]);

    // Every row's text starts in the same column, which is the point of padding
    // rather than simply omitting the prefix.
    let text_column = |line: &str| line.find(|c: char| !c.is_whitespace()).unwrap_or(0);
    assert_eq!(
        text_column(&rendered[1]),
        rendered[2].find("upsilon").unwrap()
    );
    assert_eq!(
        text_column(&rendered[3]),
        rendered[2].find("upsilon").unwrap()
    );
}

#[test]
fn heading_mode_separators_belong_to_context_output() {
    // Two matches, no context: the `--` would sit between two adjacent lines and
    // delimit nothing. With context it marks the gap between the two blocks.
    let id = make_id(2050);
    let events = turn(vec![ConversationEvent::new(
        ChatRequest::from("tau-mark one\nskip\nskip\nskip\nskip\ntau-mark two"),
        ts(),
    )]);

    let (mut ctx, out) = setup_pretty(vec![(id, events.clone())], 80);
    let rendered = run(grep("tau-mark"), &mut ctx, &out);
    assert!(
        !rendered.iter().any(|line| line.trim() == "--"),
        "no context means no blocks to separate: {rendered:?}"
    );

    let (mut ctx, out) = setup_pretty(vec![(id, events)], 80);
    let grep = Grep {
        context: 1,
        ..grep("tau-mark")
    };
    let rendered = run(grep, &mut ctx, &out);
    assert!(
        rendered.iter().any(|line| line.trim() == "--"),
        "two non-contiguous context blocks need a separator: {rendered:?}"
    );
}

#[test]
fn heading_counts_matches_and_turns_separately() {
    // The two figures answer different questions ("how much did this give me"
    // and "how big is this if I open it") and are not commensurate, so they are
    // reported side by side rather than as a ratio.
    let id = make_id(2100);
    let mut events = turn(vec![ConversationEvent::new(
        ChatRequest::from("tau-mark once\ntau-mark twice"),
        ts(),
    )]);
    events.extend(turn(vec![ConversationEvent::new(
        ChatRequest::from("nothing here"),
        ts(),
    )]));
    let (mut ctx, out) = setup_pretty(vec![(id, events)], 80);

    let rendered = run(grep("tau-mark"), &mut ctx, &out);
    assert!(
        rendered[0].ends_with("2 matches · 2 turns"),
        "got: {:?}",
        rendered[0]
    );
}

#[test]
fn heading_omits_the_turn_count_for_a_title_only_search() {
    // `--scope title` never reads the event stream, so the turn count is
    // unknown. Reporting it as `0 turns` would be a lie.
    let id = make_id(2150);
    let conv = Conversation {
        title: Some("psi-mark in the title".into()),
        ..Default::default()
    };
    let (mut ctx, out) = setup_conversations_with(
        vec![(
            id,
            conv,
            turn(vec![ConversationEvent::new(
                ChatRequest::from("an event that is never read"),
                ts(),
            )]),
        )],
        OutputFormat::TextPretty,
        OutputWidth::Terminal(80),
    );

    let grep = Grep {
        scopes: vec![Scope::Title],
        ..grep("psi-mark")
    };
    let rendered = run(grep, &mut ctx, &out);
    assert!(
        rendered[0].ends_with("1 match"),
        "expected no turn figure: {:?}",
        rendered[0]
    );
}

#[test]
fn heading_singularizes_a_lone_match() {
    let id = make_id(2200);
    let (mut ctx, out) = setup_pretty(
        vec![(
            id,
            turn(vec![ConversationEvent::new(
                ChatRequest::from("rho-mark"),
                ts(),
            )]),
        )],
        80,
    );

    let rendered = run(grep("rho-mark"), &mut ctx, &out);
    assert!(
        rendered[0].ends_with("1 match · 1 turn"),
        "got: {:?}",
        rendered[0]
    );
}

#[test]
fn heading_falls_back_when_the_conversation_has_no_title() {
    let id = make_id(2300);
    let (mut ctx, out) = setup_pretty(
        vec![(
            id,
            turn(vec![ConversationEvent::new(
                ChatRequest::from("sigma-mark"),
                ts(),
            )]),
        )],
        80,
    );

    let rendered = run(grep("sigma-mark"), &mut ctx, &out);
    assert!(rendered[0].contains("(no title)"), "got: {:?}", rendered[0]);
}

#[test]
fn heading_mode_separates_conversations_with_a_blank_line() {
    let id_a = make_id(2400);
    let id_b = make_id(2500);
    let (mut ctx, out) = setup_pretty(
        vec![
            (
                id_a,
                turn(vec![ConversationEvent::new(
                    ChatRequest::from("iota-mark alpha"),
                    ts(),
                )]),
            ),
            (
                id_b,
                turn(vec![ConversationEvent::new(
                    ChatRequest::from("iota-mark beta"),
                    ts(),
                )]),
            ),
        ],
        80,
    );

    let rendered = run(grep("iota-mark"), &mut ctx, &out);
    assert_eq!(rendered[1], "  1:user:iota-mark alpha");
    assert_eq!(
        rendered[2], "",
        "conversations are separated by a blank line"
    );
    assert!(rendered[3].starts_with(&id_b.to_string()));
}

#[test]
fn heading_mode_right_aligns_the_turn_field() {
    // Turn 9 and turn 10 in one conversation: the coordinates have to line up
    // or the eye can't scan the column.
    let id = make_id(2600);
    let mut events = vec![];
    for n in 1..=10 {
        events.extend(turn(vec![ConversationEvent::new(
            ChatRequest::from(if n >= 9 { "kappa-mark" } else { "other" }),
            ts(),
        )]));
    }
    let (mut ctx, out) = setup_pretty(vec![(id, events)], 80);

    let rendered = run(grep("kappa-mark"), &mut ctx, &out);
    assert_eq!(rendered[1], "   9:user:kappa-mark");
    assert_eq!(rendered[2], "  10:user:kappa-mark");
}

#[test]
fn heading_is_off_when_piped_and_on_in_a_terminal() {
    let id = make_id(2700);
    let events = turn(vec![ConversationEvent::new(
        ChatRequest::from("upsilon-mark"),
        ts(),
    )]);

    let (mut ctx, out) = setup(vec![(id, events.clone())]);
    assert_eq!(run(grep("upsilon-mark"), &mut ctx, &out).len(), 1);

    let (mut ctx, out) = setup_pretty(vec![(id, events)], 80);
    assert_eq!(run(grep("upsilon-mark"), &mut ctx, &out).len(), 2);
}

#[test]
fn heading_can_be_forced_on_when_piped() {
    let id = make_id(2800);
    let (mut ctx, out) = setup(vec![(
        id,
        turn(vec![ConversationEvent::new(
            ChatRequest::from("eta-mark"),
            ts(),
        )]),
    )]);

    let grep = Grep {
        heading: true,
        ..grep("eta-mark")
    };
    assert_eq!(run(grep, &mut ctx, &out), [
        format!("{id}  (no title)  1 match · 1 turn"),
        "  1:user:eta-mark".to_owned(),
    ]);
}

#[test]
fn heading_can_be_forced_off_in_a_terminal() {
    let id = make_id(2900);
    let (mut ctx, out) = setup_pretty(
        vec![(
            id,
            turn(vec![ConversationEvent::new(
                ChatRequest::from("theta-mark"),
                ts(),
            )]),
        )],
        80,
    );

    let grep = Grep {
        no_heading: true,
        ..grep("theta-mark")
    };
    assert_eq!(run(grep, &mut ctx, &out), [format!(
        "{id}:1:user:m:theta-mark"
    )]);
}

// --- width ------------------------------------------------------------------

#[test]
fn text_budget_is_what_the_prefix_leaves_over() {
    assert_eq!(text_budget(Some(120), 16), Some(104));
    assert_eq!(text_budget(None, 16), None);
}

#[test]
fn text_budget_floors_at_a_usable_width() {
    // A prefix wider than the line still leaves room for text, so a hit never
    // renders as prefix-only.
    assert_eq!(text_budget(Some(20), 40), Some(MIN_TEXT_WIDTH));
}

#[test]
fn piped_output_is_never_truncated() {
    let id = make_id(3000);
    let text = "z".repeat(200);
    let (mut ctx, out) = setup(vec![(
        id,
        turn(vec![ConversationEvent::new(
            ChatRequest::from(text.as_str()),
            ts(),
        )]),
    )]);

    assert_eq!(run(grep("zzz"), &mut ctx, &out), [format!(
        "{id}:1:user:m:{text}"
    )]);
}

#[test]
fn an_explicit_width_caps_the_whole_line() {
    // The global `--width` is the only truncation lever: it reaches grep as the
    // known output width, whether it was detected or given.
    let id = make_id(3100);
    let (mut ctx, out) = setup_with(
        vec![(
            id,
            turn(vec![ConversationEvent::new(
                ChatRequest::from("z".repeat(200).as_str()),
                ts(),
            )]),
        )],
        OutputFormat::Text,
        // `Declared` rather than `Terminal`: this is the `--width` path, a known
        // width on output the caller says is laid out but that isn't a TTY.
        OutputWidth::Declared(40),
    );

    let rendered = run(grep("zzz"), &mut ctx, &out);
    assert_eq!(display_width(&rendered[0]), 40);
    assert!(rendered[0].ends_with('…'));
}

#[test]
fn piped_text_is_verbatim_even_when_the_match_is_trailing_whitespace() {
    // The piped record promises TEXT verbatim, and `--regex '\s+$'` is the case
    // where trimming would remove the match itself.
    let id = make_id(3250);
    let (mut ctx, out) = setup(vec![(
        id,
        turn(vec![ConversationEvent::new(
            ChatRequest::from("omega   "),
            ts(),
        )]),
    )]);

    let grep = Grep {
        regex: true,
        ..grep(r"\s+$")
    };
    grep.run(&mut ctx, vec![]).unwrap();
    ctx.printer.flush();

    let raw = out.lock().clone();
    let line = raw.strip_suffix('\n').unwrap_or(&raw);
    assert_eq!(line, format!("{id}:1:user:m:omega   "));
}

#[test]
fn output_text_is_verbatim_even_when_the_match_is_trailing_whitespace() {
    let id = make_id(3260);
    let (mut ctx, out) = setup(vec![(
        id,
        turn(vec![ConversationEvent::new(
            ChatRequest::from("omega   "),
            ts(),
        )]),
    )]);

    let grep = Grep {
        regex: true,
        output: OutputKind::Text,
        ..grep(r"\s+$")
    };
    grep.run(&mut ctx, vec![]).unwrap();
    ctx.printer.flush();

    let raw = out.lock().clone();
    let line = raw.strip_suffix('\n').unwrap_or(&raw);
    assert_eq!(line, "omega   ");
}

#[test]
fn a_terminal_fits_lines_to_its_width() {
    let id = make_id(3300);
    let (mut ctx, out) = setup_pretty(
        vec![(
            id,
            turn(vec![ConversationEvent::new(
                ChatRequest::from("z".repeat(200).as_str()),
                ts(),
            )]),
        )],
        80,
    );

    let rendered = run(grep("zzz"), &mut ctx, &out);
    assert_eq!(display_width(&rendered[1]), 80);
}

// --- match highlighting -----------------------------------------------------

#[test]
fn highlight_styles_only_the_matched_span() {
    let styled = highlight("a needle here", std::slice::from_ref(&(2..8)), None);
    assert_eq!(strip_str(&styled), "a needle here");
    assert!(
        styled.starts_with("a "),
        "text before the span is unstyled: {styled:?}"
    );
    assert!(
        styled.ends_with(" here"),
        "text after the span is unstyled: {styled:?}"
    );
    assert!(styled.contains('\x1b'), "the span is styled: {styled:?}");
}

#[test]
fn highlight_drops_spans_past_a_truncated_line() {
    // Spans are found against the full line; after truncation the ones beyond
    // the cut have nothing to point at.
    let styled = highlight("abc", &[0..1, 10..12], None);
    assert_eq!(styled, format!("{}bc", "a".red().bold()));
}

#[test]
fn highlight_clips_a_span_straddling_the_end() {
    let styled = highlight("abc", std::slice::from_ref(&(1..9)), None);
    assert_eq!(strip_str(&styled), "abc");
}

#[test]
fn highlight_never_styles_the_truncation_ellipsis() {
    // `spans` index the original line, but the string handed to `highlight` is a
    // prefix of it plus `…`. Clipping to the string's length would treat the
    // ellipsis bytes as match text; `kept` bounds it to the surviving prefix.
    //
    // "abc…" is 3 + 3 bytes, so the ellipsis occupies 3..6 and `kept` is 3.
    let ellipsis_start = 3;

    // A match straddling the cut keeps the visible part styled and leaves the
    // ellipsis bare.
    let styled = highlight("abc…", std::slice::from_ref(&(0..5)), Some(ellipsis_start));
    assert_eq!(styled, format!("{}…", "abc".red().bold()));

    // A match whose clipped end would land *inside* the ellipsis keeps its
    // highlight. Clipping to the string length put `end` at a non-boundary, so
    // the span was skipped whole and a partly-visible match lost its styling.
    let styled = highlight("abc…", std::slice::from_ref(&(2..4)), Some(ellipsis_start));
    assert_eq!(styled, format!("ab{}…", "c".red().bold()));

    // A match starting at the cut is entirely invisible, so nothing is styled.
    let styled = highlight("abc…", std::slice::from_ref(&(3..6)), Some(ellipsis_start));
    assert_eq!(styled, "abc…");
}

#[test]
fn highlight_leaves_multibyte_text_intact() {
    let styled = highlight("héllo wörld", std::slice::from_ref(&(0..6)), None);
    assert_eq!(strip_str(&styled), "héllo wörld");
}

#[test]
fn a_terminal_highlights_the_match_within_the_line() {
    let id = make_id(3400);
    let (mut ctx, out) = setup_pretty(
        vec![(
            id,
            turn(vec![ConversationEvent::new(
                ChatRequest::from("find the needle in here"),
                ts(),
            )]),
        )],
        80,
    );

    grep("needle").run(&mut ctx, vec![]).unwrap();
    ctx.printer.flush();
    let raw = out.lock().clone();

    assert!(
        raw.contains(&"needle".red().bold().to_string()),
        "expected the match to be styled: {raw:?}"
    );
}

// --- smart case -------------------------------------------------------------

#[test]
fn a_lowercase_pattern_matches_case_insensitively() {
    let id = make_id(4000);
    let (mut ctx, out) = setup(vec![(
        id,
        turn(vec![ConversationEvent::new(
            ChatRequest::from("Tell me about WASM"),
            ts(),
        )]),
    )]);

    assert_eq!(run(grep("wasm"), &mut ctx, &out), [format!(
        "{id}:1:user:m:Tell me about WASM"
    )]);
}

#[test]
fn an_uppercase_pattern_matches_case_sensitively() {
    let id = make_id(4100);
    let (mut ctx, _out) = setup(vec![(
        id,
        turn(vec![ConversationEvent::new(
            ChatRequest::from("tell me about wasm"),
            ts(),
        )]),
    )]);

    assert!(grep("WASM").run(&mut ctx, vec![]).is_err());
}

#[test]
fn case_sensitive_overrides_smart_case() {
    let id = make_id(4200);
    let (mut ctx, _out) = setup(vec![(
        id,
        turn(vec![ConversationEvent::new(
            ChatRequest::from("Tell me about WASM"),
            ts(),
        )]),
    )]);

    let grep = Grep {
        case_sensitive: true,
        ..grep("wasm")
    };

    let error = grep.run(&mut ctx, vec![]).unwrap_err();

    // Non-zero, so a script can branch on "found nothing", but marked expected:
    // `JP_DEBUG=1 jp c grep ... | fzf` must not get the trace log location
    // printed into fzf's terminal just because nothing matched.
    assert_eq!(error.code.get(), 1);
    assert!(error.expected);
}

#[test]
fn ignore_case_overrides_smart_case() {
    let id = make_id(4300);
    let (mut ctx, out) = setup(vec![(
        id,
        turn(vec![ConversationEvent::new(
            ChatRequest::from("tell me about wasm"),
            ts(),
        )]),
    )]);

    let grep = Grep {
        ignore_case: true,
        ..grep("WASM")
    };
    assert_eq!(run(grep, &mut ctx, &out), [format!(
        "{id}:1:user:m:tell me about wasm"
    )]);
}

// --- output kinds -----------------------------------------------------------

#[test]
fn output_ids_lists_one_conversation_per_line() {
    let id_a = make_id(5000);
    let id_b = make_id(5100);
    let (mut ctx, out) = setup(vec![
        (
            id_a,
            turn(vec![ConversationEvent::new(
                ChatRequest::from("zeta-mark alpha"),
                ts(),
            )]),
        ),
        (
            id_b,
            turn(vec![ConversationEvent::new(
                ChatRequest::from("zeta-mark beta"),
                ts(),
            )]),
        ),
    ]);

    let grep = Grep {
        output: OutputKind::Ids,
        ..grep("zeta-mark")
    };
    assert_eq!(run(grep, &mut ctx, &out), [
        id_a.to_string(),
        id_b.to_string()
    ]);
}

#[test]
fn output_ids_reports_each_conversation_once() {
    let id = make_id(5200);
    let (mut ctx, out) = setup(vec![(
        id,
        turn(vec![
            ConversationEvent::new(ChatRequest::from("delta-mark asked"), ts()),
            ConversationEvent::new(ChatResponse::message("delta-mark answered"), ts()),
        ]),
    )]);

    let grep = Grep {
        output: OutputKind::Ids,
        ..grep("delta-mark")
    };
    assert_eq!(run(grep, &mut ctx, &out), [id.to_string()]);
}

#[test]
fn output_count_reports_matching_lines_not_context_lines() {
    let id = make_id(5300);
    let (mut ctx, out) = setup(vec![(
        id,
        turn(vec![ConversationEvent::new(
            ChatRequest::from("before\ngamma-mark\nbetween\ngamma-mark\nafter"),
            ts(),
        )]),
    )]);

    let grep = Grep {
        output: OutputKind::Count,
        context: 1,
        ..grep("gamma-mark")
    };
    assert_eq!(run(grep, &mut ctx, &out), [format!("{id}:2")]);
}

#[test]
fn output_text_drops_coordinates_and_separators() {
    // `--output text` exists to pipe content onward, where a coordinate or a
    // `--` marker would be something the consumer has to strip back out.
    let id = make_id(5400);
    let (mut ctx, out) = setup(vec![(
        id,
        turn(vec![ConversationEvent::new(
            ChatRequest::from("first beta-mark\nskip\nskip\nsecond beta-mark"),
            ts(),
        )]),
    )]);

    let grep = Grep {
        output: OutputKind::Text,
        ..grep("beta-mark")
    };
    assert_eq!(run(grep, &mut ctx, &out), [
        "first beta-mark",
        "second beta-mark"
    ]);
}

#[test]
fn output_and_format_compose() {
    // `--output` picks the records and `-F` picks the encoding; every
    // combination has to work rather than one silently overriding the other.
    let id = make_id(5500);
    let (mut ctx, out) = setup_json(vec![(
        id,
        turn(vec![ConversationEvent::new(
            ChatRequest::from("alpha-mark"),
            ts(),
        )]),
    )]);

    let grep = Grep {
        output: OutputKind::Count,
        ..grep("alpha-mark")
    };
    grep.run(&mut ctx, vec![]).unwrap();
    ctx.printer.flush();

    let parsed: Value = serde_json::from_str(&out.lock().clone()).unwrap();
    assert_eq!(parsed[0]["id"], id.to_string());
    assert_eq!(parsed[0]["count"], 1);
}

// --- limits -----------------------------------------------------------------

#[test]
fn max_matches_caps_matches_per_conversation() {
    let id = make_id(6000);
    let (mut ctx, out) = setup(vec![(
        id,
        turn(vec![ConversationEvent::new(
            ChatRequest::from("mu-mark one\nmu-mark two\nmu-mark three"),
            ts(),
        )]),
    )]);

    let grep = Grep {
        max_matches: NonZeroUsize::new(2),
        ..grep("mu-mark")
    };
    assert_eq!(run(grep, &mut ctx, &out), [
        format!("{id}:1:user:m:mu-mark one"),
        format!("{id}:1:user:m:mu-mark two"),
    ]);
}

#[test]
fn max_matches_applies_across_the_whole_conversation() {
    // The budget spans scopes: two matches in the request already exhaust it, so
    // the response contributes none.
    let id = make_id(6100);
    let (mut ctx, out) = setup(vec![(
        id,
        turn(vec![
            ConversationEvent::new(ChatRequest::from("nu-mark one\nnu-mark two"), ts()),
            ConversationEvent::new(ChatResponse::message("nu-mark three"), ts()),
        ]),
    )]);

    let grep = Grep {
        max_matches: NonZeroUsize::new(2),
        ..grep("nu-mark")
    };
    let rendered = run(grep, &mut ctx, &out);
    assert_eq!(rendered.len(), 2, "got: {rendered:?}");
    assert!(!rendered.iter().any(|line| line.contains("three")));
}

#[test]
fn limit_caps_conversations_in_sort_order() {
    let id_a = make_id(6200);
    let id_b = make_id(6300);
    let id_c = make_id(6400);
    let (mut ctx, out) = setup(vec![
        (
            id_a,
            turn(vec![ConversationEvent::new(
                ChatRequest::from("xi-mark a"),
                ts(),
            )]),
        ),
        (
            id_b,
            turn(vec![ConversationEvent::new(
                ChatRequest::from("xi-mark b"),
                ts(),
            )]),
        ),
        (
            id_c,
            turn(vec![ConversationEvent::new(
                ChatRequest::from("xi-mark c"),
                ts(),
            )]),
        ),
    ]);

    let grep = Grep {
        limit: NonZeroUsize::new(2),
        output: OutputKind::Ids,
        ..grep("xi-mark")
    };
    assert_eq!(run(grep, &mut ctx, &out), [
        id_a.to_string(),
        id_b.to_string()
    ]);
}

// --- quiet ------------------------------------------------------------------

#[test]
fn quiet_reports_a_match_through_its_exit_status_alone() {
    let id = make_id(7000);
    let (mut ctx, out) = setup(vec![(
        id,
        turn(vec![ConversationEvent::new(
            ChatRequest::from("omicron-mark"),
            ts(),
        )]),
    )]);
    ctx.term.args.quiet = true;

    assert!(grep("omicron-mark").run(&mut ctx, vec![]).is_ok());
    ctx.printer.flush();
    assert_eq!(
        out.lock().clone(),
        "",
        "--quiet must not write to the output channel"
    );
}

#[test]
fn quiet_exits_zero_when_a_match_survives_a_failing_pattern() {
    // `grep -q`'s rule: if a line is selected the status is 0 even if an error
    // occurred.
    //
    // Both inputs live in one event, poisoning line first, so `matching_lines`
    // reaches them in a fixed order. Splitting them across two conversations
    // makes the test vacuous: `find_any` can return the match before the
    // pathological one is ever scanned, so no failure gets recorded and the
    // assertion holds for the wrong reason — which is the scheduling dependence
    // this guards against.
    let id = make_id(7200);
    let (mut ctx, _out) = setup(vec![(
        id,
        turn(vec![ConversationEvent::new(
            ChatRequest::from(format!("{}\naab", "a".repeat(64)).as_str()),
            ts(),
        )]),
    )]);
    ctx.term.args.quiet = true;

    let grep = Grep {
        regex: true,
        ..grep(r"(a+)+\1b")
    };
    assert!(
        grep.run(&mut ctx, vec![]).is_ok(),
        "a found match outranks a mid-search failure"
    );
}

#[test]
fn quiet_exits_two_when_a_failure_leaves_no_match() {
    // With nothing selected the failure is the only outcome, so it must not be
    // reported as a clean no-match.
    let id = make_id(7400);
    let (mut ctx, _out) = setup(vec![(
        id,
        turn(vec![ConversationEvent::new(
            ChatRequest::from("a".repeat(64).as_str()),
            ts(),
        )]),
    )]);
    ctx.term.args.quiet = true;

    let grep = Grep {
        regex: true,
        ..grep(r"(a+)+\1b")
    };
    let error = grep.run(&mut ctx, vec![]).unwrap_err();
    assert_eq!(error.code.get(), 2);
}

#[test]
fn quiet_fails_without_a_match() {
    let id = make_id(7100);
    let (mut ctx, _out) = setup(vec![(
        id,
        turn(vec![ConversationEvent::new(
            ChatRequest::from("nothing"),
            ts(),
        )]),
    )]);
    ctx.term.args.quiet = true;

    let error = grep("pi-mark").run(&mut ctx, vec![]).unwrap_err();
    assert_eq!(error.code.get(), 1);
    // This path bypasses `render_empty`, so it carries the expected marker
    // itself. Without it, `JP_DEBUG=1 jp c grep -q ... | fzf` announces the
    // trace log into the pipe on every empty search.
    assert!(error.expected);
}

// --- exit status ------------------------------------------------------------

#[test]
fn no_match_exits_one_and_stays_silent_when_piped() {
    let id = make_id(8000);
    let (mut ctx, out) = setup(vec![(
        id,
        turn(vec![ConversationEvent::new(
            ChatRequest::from("hello"),
            ts(),
        )]),
    )]);

    let error = grep("nonexistent").run(&mut ctx, vec![]).unwrap_err();
    assert_eq!(error.code.get(), 1);
    assert_eq!(error.message, None);
    // Non-zero so a script can branch on "found nothing", but not a failure:
    // failure-only diagnostics stay quiet for a piped run.
    assert!(error.expected);
    ctx.printer.flush();
    assert_eq!(out.lock().clone(), "");
}

#[test]
fn no_match_still_emits_a_well_formed_empty_json_result() {
    let id = make_id(8100);
    let (mut ctx, out) = setup_json(vec![(
        id,
        turn(vec![ConversationEvent::new(
            ChatRequest::from("hello"),
            ts(),
        )]),
    )]);

    assert!(grep("nonexistent").run(&mut ctx, vec![]).is_err());
    ctx.printer.flush();
    let parsed: Value = serde_json::from_str(&out.lock().clone()).unwrap();
    assert_eq!(parsed, json!([]));
}

#[test]
fn a_pattern_that_fails_mid_search_exits_two() {
    // An exceeded backtrack limit leaves the result unknown — some lines may
    // never have been decided — which is a different outcome from finding
    // nothing, so it must not be reported as a clean no-match.
    let id = make_id(8300);
    let (mut ctx, out) = setup(vec![(
        id,
        turn(vec![ConversationEvent::new(
            ChatRequest::from("a".repeat(64).as_str()),
            ts(),
        )]),
    )]);

    // A backreference forces `fancy-regex`'s backtracking engine, and nested
    // quantifiers over a long run of the same character blow its step limit.
    let grep = Grep {
        regex: true,
        ..grep(r"(a+)+\1b")
    };
    let error = grep.run(&mut ctx, vec![]).unwrap_err();

    assert_eq!(
        error.code.get(),
        2,
        "a mid-search failure is not a no-match"
    );
    ctx.printer.flush();
    assert_eq!(out.lock().clone(), "", "no hits are reported on failure");
}

#[test]
fn an_invalid_pattern_exits_two() {
    // A broken pattern is a different outcome from a pattern that found
    // nothing, and a script has to be able to tell them apart.
    let id = make_id(8200);
    let (mut ctx, _out) = setup(vec![(
        id,
        turn(vec![ConversationEvent::new(
            ChatRequest::from("anything"),
            ts(),
        )]),
    )]);

    let grep = Grep {
        regex: true,
        ..grep("(unclosed")
    };
    let error = grep.run(&mut ctx, vec![]).unwrap_err();
    assert_eq!(error.code.get(), 2);
}

// --- scopes -----------------------------------------------------------------

#[test]
fn the_scope_field_names_where_each_line_came_from() {
    // `--scope chat` is one flag covering four scopes, so the record itself has
    // to say which one matched.
    let id = make_id(9000);
    let (mut ctx, out) = setup(vec![(
        id,
        turn(vec![
            ConversationEvent::new(ChatRequest::from("lambda-mark"), ts()),
            ConversationEvent::new(ChatResponse::message("lambda-mark"), ts()),
            ConversationEvent::new(ChatResponse::reasoning("lambda-mark"), ts()),
        ]),
    )]);

    let grep = Grep {
        scopes: vec![Scope::Chat],
        ..grep("lambda-mark")
    };
    assert_eq!(run(grep, &mut ctx, &out), [
        format!("{id}:1:user:m:lambda-mark"),
        format!("{id}:1:assistant:m:lambda-mark"),
        format!("{id}:1:reasoning:m:lambda-mark"),
    ]);
}

#[test]
fn scope_user_excludes_assistant_lines() {
    let id = make_id(9100);
    let (mut ctx, out) = setup(vec![(
        id,
        turn(vec![
            ConversationEvent::new(ChatRequest::from("alpha-from-user"), ts()),
            ConversationEvent::new(ChatResponse::message("alpha-from-assistant"), ts()),
        ]),
    )]);

    let grep = Grep {
        scopes: vec![Scope::User],
        ..grep("alpha")
    };
    assert_eq!(run(grep, &mut ctx, &out), [format!(
        "{id}:1:user:m:alpha-from-user"
    )]);
}

#[test]
fn scope_title_does_not_match_events() {
    let id = make_id(9200);
    let (mut ctx, _out) = setup(vec![(
        id,
        turn(vec![ConversationEvent::new(
            ChatRequest::from("delta-in-request"),
            ts(),
        )]),
    )]);

    let grep = Grep {
        scopes: vec![Scope::Title],
        ..grep("delta")
    };
    assert!(grep.run(&mut ctx, vec![]).is_err());
}

#[test]
fn scope_tool_call_searches_arguments() {
    let id = make_id(9300);
    let mut args = Map::new();
    args.insert("path".into(), Value::String("/etc/epsilon-file".into()));

    let (mut ctx, out) = setup(vec![(
        id,
        turn(vec![ConversationEvent::new(
            ToolCallRequest::new("tc1".into(), "read_file".into(), args),
            ts(),
        )]),
    )]);

    let grep = Grep {
        scopes: vec![Scope::ToolCall],
        ..grep("epsilon-file")
    };
    // Tool arguments are pretty-printed, and the leading indentation survives
    // so the structure stays readable.
    assert_eq!(run(grep, &mut ctx, &out), [format!(
        "{id}:1:tool-call:m:  \"path\": \"/etc/epsilon-file\""
    )]);
}

#[test]
fn tool_results_are_searched() {
    let id = make_id(9400);
    let (mut ctx, out) = setup(vec![(
        id,
        turn(vec![ConversationEvent::new(
            ToolCallResponse {
                id: "tc1".into(),
                result: Ok("file content with secret-keyword here".into()),
            },
            ts(),
        )]),
    )]);

    assert_eq!(run(grep("secret-keyword"), &mut ctx, &out), [format!(
        "{id}:1:tool-result:m:file content with secret-keyword here"
    )]);
}

#[test]
fn scope_structured_searches_serialized_json() {
    let id = make_id(9500);
    let (mut ctx, out) = setup(vec![(
        id,
        turn(vec![ConversationEvent::new(
            ChatResponse::structured(json!({ "name": "Alice" })),
            ts(),
        )]),
    )]);

    let grep = Grep {
        scopes: vec![Scope::Structured],
        ..grep("Alice")
    };
    // The persisted value is a JSON object, so the searchable text is the
    // pretty-printed serialization rather than the value itself.
    assert_eq!(run(grep, &mut ctx, &out), [format!(
        "{id}:1:structured:m:  \"name\": \"Alice\""
    )]);
}

#[test]
fn expand_scopes_empty_is_all() {
    assert_eq!(expand_scopes(&[]).len(), ConcreteScope::ALL.len());
}

#[test]
fn expand_scopes_chat_meta() {
    let expanded = expand_scopes(&[Scope::Chat]);
    assert!(expanded.contains(&ConcreteScope::User));
    assert!(expanded.contains(&ConcreteScope::Assistant));
    assert!(expanded.contains(&ConcreteScope::Reasoning));
    assert!(expanded.contains(&ConcreteScope::Structured));
    assert!(!expanded.contains(&ConcreteScope::Title));
    assert!(!expanded.contains(&ConcreteScope::ToolCall));
}

#[test]
fn expand_scopes_tool_meta() {
    let expanded = expand_scopes(&[Scope::Tool]);
    assert!(expanded.contains(&ConcreteScope::ToolCall));
    assert!(expanded.contains(&ConcreteScope::ToolResult));
    assert!(!expanded.contains(&ConcreteScope::User));
}

#[test]
fn expand_scopes_mixed() {
    let expanded = expand_scopes(&[Scope::Title, Scope::User]);
    assert_eq!(expanded.len(), 2);
    assert!(expanded.contains(&ConcreteScope::Title));
    assert!(expanded.contains(&ConcreteScope::User));
}

#[test]
fn a_title_only_search_never_reads_the_event_stream() {
    assert!(!needs_events_for(&expand_scopes(&[Scope::Title])));
    assert!(needs_events_for(&expand_scopes(&[
        Scope::Title,
        Scope::User
    ])));
}

// --- context ----------------------------------------------------------------

#[test]
fn context_surrounds_each_match() {
    let id = make_id(10_000);
    let (mut ctx, out) = setup(vec![(
        id,
        turn(vec![ConversationEvent::new(
            ChatRequest::from("line-one\nline-two\nMATCH-here\nline-four\nline-five"),
            ts(),
        )]),
    )]);

    let grep = Grep {
        context: 1,
        ..grep("MATCH")
    };
    assert_eq!(run(grep, &mut ctx, &out), [
        format!("{id}:1:user:c:line-two"),
        format!("{id}:1:user:m:MATCH-here"),
        format!("{id}:1:user:c:line-four"),
    ]);
}

#[test]
fn context_merges_overlapping_ranges() {
    let id = make_id(10_100);
    let (mut ctx, out) = setup(vec![(
        id,
        turn(vec![ConversationEvent::new(
            ChatRequest::from("line-a\nMATCH-one\nline-c\nMATCH-two\nline-e"),
            ts(),
        )]),
    )]);

    let grep = Grep {
        context: 1,
        ..grep("MATCH")
    };
    let rendered = run(grep, &mut ctx, &out);
    assert_eq!(rendered.len(), 5, "one merged group: {rendered:?}");
    assert!(!rendered.iter().any(|line| line == "--"));
}

#[test]
fn context_clamps_at_the_edges() {
    let id = make_id(10_200);
    let (mut ctx, out) = setup(vec![(
        id,
        turn(vec![ConversationEvent::new(
            ChatRequest::from("MATCH-first\nline-b\nline-c"),
            ts(),
        )]),
    )]);

    let grep = Grep {
        context: 3,
        ..grep("MATCH")
    };
    assert_eq!(run(grep, &mut ctx, &out).len(), 3);
}

#[test]
fn context_ranges_no_overlap() {
    assert_eq!(context_ranges(&[0, 4], 1, 5), vec![(0, 1), (3, 4)]);
}

#[test]
fn context_ranges_merge() {
    assert_eq!(context_ranges(&[1, 3], 1, 5), vec![(0, 4)]);
}

#[test]
fn context_ranges_single() {
    assert_eq!(context_ranges(&[2], 0, 5), vec![(2, 2)]);
}

// --- separators -------------------------------------------------------------

#[test]
fn separator_between_non_contiguous_groups_in_one_event() {
    let id = make_id(11_000);
    let (mut ctx, out) = setup(vec![(
        id,
        turn(vec![ConversationEvent::new(
            ChatRequest::from("MATCH-first\nline-b\nline-c\nline-d\nMATCH-last"),
            ts(),
        )]),
    )]);

    let grep = Grep {
        context: 1,
        ..grep("MATCH")
    };
    assert_eq!(run(grep, &mut ctx, &out), [
        format!("{id}:1:user:m:MATCH-first"),
        format!("{id}:1:user:c:line-b"),
        "--".to_owned(),
        format!("{id}:1:user:c:line-d"),
        format!("{id}:1:user:m:MATCH-last"),
    ]);
}

#[test]
fn the_default_piped_stream_is_all_coordinate_records() {
    // Every line has to carry the full coordinate, or a field-splitting consumer
    // (the advertised `fzf --delimiter :` recipe) selects a row it can't act on.
    // `--` group separators belong to `--context` output, as in `grep`.
    let id_a = make_id(11_050);
    let id_b = make_id(11_060);
    let (mut ctx, out) = setup(vec![
        (
            id_a,
            turn(vec![ConversationEvent::new(
                ChatRequest::from("zeta-mark one\nskip\nskip\nzeta-mark two"),
                ts(),
            )]),
        ),
        (
            id_b,
            turn(vec![ConversationEvent::new(
                ChatRequest::from("zeta-mark three"),
                ts(),
            )]),
        ),
    ]);

    let rendered = run(grep("zeta-mark"), &mut ctx, &out);
    assert_eq!(rendered, [
        format!("{id_a}:1:user:m:zeta-mark one"),
        format!("{id_a}:1:user:m:zeta-mark two"),
        format!("{id_b}:1:user:m:zeta-mark three"),
    ]);
    for line in &rendered {
        let fields: Vec<&str> = line.splitn(5, ':').collect();
        assert_eq!(fields.len(), 5, "not a coordinate record: {line}");
        assert_eq!(fields[3], "m", "every default line is a match: {line}");
    }
}

#[test]
fn every_record_has_the_same_field_count_whatever_the_flags() {
    // The parse contract: four colon-free fields then the text verbatim, so a
    // script splitting on `:` never has to know how grep was invoked. Only the
    // kind field distinguishes a match from a context line.
    let id = make_id(11_070);
    let (mut ctx, out) = setup(vec![(
        id,
        turn(vec![ConversationEvent::new(
            ChatRequest::from("before\nomega-mark: with a colon\nafter"),
            ts(),
        )]),
    )]);

    let grep = Grep {
        context: 1,
        ..grep("omega-mark")
    };
    let rendered = run(grep, &mut ctx, &out);

    let kinds: Vec<String> = rendered
        .iter()
        .map(|line| {
            let fields: Vec<&str> = line.splitn(5, ':').collect();
            assert_eq!(fields.len(), 5, "not a coordinate record: {line}");
            // Text containing colons must not shift the coordinate fields.
            assert_eq!(fields[0], id.to_string());
            assert_eq!(fields[1], "1");
            assert_eq!(fields[2], "user");
            fields[3].to_owned()
        })
        .collect();

    assert_eq!(kinds, ["c", "m", "c"]);
    // The matched line's text is recovered whole, colon and all.
    assert!(rendered[1].ends_with(":omega-mark: with a colon"));
}

#[test]
fn line_mode_separators_stay_unstyled_under_a_pretty_format() {
    // `-F text-pretty | fzf --ansi` is the advertised pipeline, and the docs tell
    // the reader to drop separators with `grep -v '^--$'`. A dimmed `--` carries
    // ANSI bytes, so that filter would match nothing and the row would stay
    // selectable in `fzf`, where it has no ID or turn to preview.
    //
    // Asserted against the raw buffer: the `lines()` helper strips ANSI, so a
    // stripped assertion passes whether or not the marker is styled — which is
    // how this got past the existing separator tests.
    let id = make_id(11_080);
    let (mut ctx, out) = setup_pretty(
        vec![(
            id,
            turn(vec![ConversationEvent::new(
                ChatRequest::from("tau-mark one\nskip\nskip\nskip\nskip\ntau-mark two"),
                ts(),
            )]),
        )],
        200,
    );

    let grep = Grep {
        no_heading: true,
        context: 1,
        ..grep("tau-mark")
    };
    grep.run(&mut ctx, vec![]).unwrap();
    ctx.printer.flush();

    let raw = out.lock().clone();
    assert!(
        raw.lines().any(|line| line == "--"),
        "a separator row must be byte-exactly `--`: {raw:?}"
    );
    // The coordinate fields are still styled — only the separator is bare.
    assert!(
        raw.contains('\x1b'),
        "pretty line mode still styles records: {raw:?}"
    );
}

#[test]
fn separator_between_conversations() {
    let id_a = make_id(11_100);
    let id_b = make_id(11_200);
    let (mut ctx, out) = setup(vec![
        (
            id_a,
            turn(vec![ConversationEvent::new(
                ChatRequest::from("rho-mark alpha"),
                ts(),
            )]),
        ),
        (
            id_b,
            turn(vec![ConversationEvent::new(
                ChatRequest::from("rho-mark beta"),
                ts(),
            )]),
        ),
    ]);

    let grep = Grep {
        context: 1,
        ..grep("rho-mark")
    };
    assert_eq!(run(grep, &mut ctx, &out), [
        format!("{id_a}:1:user:m:rho-mark alpha"),
        "--".to_owned(),
        format!("{id_b}:1:user:m:rho-mark beta"),
    ]);
}

// --- sorting ----------------------------------------------------------------

#[test]
fn sort_by_created_ascending() {
    let id_a = make_id(12_300);
    let id_b = make_id(12_100);
    let id_c = make_id(12_200);
    let (mut ctx, out) = setup(vec![
        (
            id_a,
            turn(vec![ConversationEvent::new(
                ChatRequest::from("MATCH-a"),
                ts(),
            )]),
        ),
        (
            id_b,
            turn(vec![ConversationEvent::new(
                ChatRequest::from("MATCH-b"),
                ts(),
            )]),
        ),
        (
            id_c,
            turn(vec![ConversationEvent::new(
                ChatRequest::from("MATCH-c"),
                ts(),
            )]),
        ),
    ]);

    let grep = Grep {
        sort: Sort::Created,
        output: OutputKind::Ids,
        ..grep("MATCH")
    };
    assert_eq!(run(grep, &mut ctx, &out), [
        id_b.to_string(),
        id_c.to_string(),
        id_a.to_string(),
    ]);
}

#[test]
fn sort_by_created_descending() {
    let id_a = make_id(12_600);
    let id_b = make_id(12_400);
    let id_c = make_id(12_500);
    let (mut ctx, out) = setup(vec![
        (
            id_a,
            turn(vec![ConversationEvent::new(
                ChatRequest::from("MATCH-a"),
                ts(),
            )]),
        ),
        (
            id_b,
            turn(vec![ConversationEvent::new(
                ChatRequest::from("MATCH-b"),
                ts(),
            )]),
        ),
        (
            id_c,
            turn(vec![ConversationEvent::new(
                ChatRequest::from("MATCH-c"),
                ts(),
            )]),
        ),
    ]);

    let grep = Grep {
        sort: Sort::Created,
        descending: true,
        output: OutputKind::Ids,
        ..grep("MATCH")
    };
    assert_eq!(run(grep, &mut ctx, &out), [
        id_a.to_string(),
        id_c.to_string(),
        id_b.to_string(),
    ]);
}

#[test]
fn sort_by_activated() {
    let id_a = make_id(13_100);
    let id_b = make_id(13_200);
    let id_c = make_id(13_300);

    // Activation order: b (Jan) < c (Feb) < a (Mar).
    let conv = |month: u32| {
        Conversation::default()
            .with_last_activated_at(Utc.with_ymd_and_hms(2025, month, 1, 0, 0, 0).unwrap())
    };
    let event = || {
        turn(vec![ConversationEvent::new(
            ChatRequest::from("MATCH"),
            ts(),
        )])
    };

    let (mut ctx, out) = setup_conversations(vec![
        (id_a, conv(3), event()),
        (id_b, conv(1), event()),
        (id_c, conv(2), event()),
    ]);

    let grep = Grep {
        sort: Sort::Activated,
        output: OutputKind::Ids,
        ..grep("MATCH")
    };
    assert_eq!(run(grep, &mut ctx, &out), [
        id_b.to_string(),
        id_c.to_string(),
        id_a.to_string(),
    ]);
}

// --- targeting --------------------------------------------------------------

#[test]
fn explicit_targets_narrow_the_search() {
    let id_a = make_id(14_000);
    let id_b = make_id(14_100);
    let (mut ctx, out) = setup(vec![
        (
            id_a,
            turn(vec![ConversationEvent::new(
                ChatRequest::from("unique-marker-alpha"),
                ts(),
            )]),
        ),
        (
            id_b,
            turn(vec![ConversationEvent::new(
                ChatRequest::from("unique-marker-beta"),
                ts(),
            )]),
        ),
    ]);

    let grep = Grep {
        target: FlagIds::from_targets(vec![ConversationTarget::Id(id_a)]),
        ..grep("unique-marker")
    };
    let handle = ctx.workspace.acquire_conversation(&id_a).unwrap();

    grep.run(&mut ctx, vec![handle]).unwrap();
    ctx.printer.flush();
    assert_eq!(lines(&out.lock().clone()), [format!(
        "{id_a}:1:user:m:unique-marker-alpha"
    )]);
}

// --- regex ------------------------------------------------------------------

#[test]
fn a_regex_pattern_anchors() {
    // Only `triage-001` matches: `triage-0012` has a digit past `\z` and
    // `xtriage-001` fails the `\A` anchor.
    let id_match = make_id(15_000);
    let id_extra = make_id(15_100);
    let id_prefixed = make_id(15_200);

    let titled = |title: &str| Conversation {
        title: Some(title.into()),
        ..Default::default()
    };
    let (mut ctx, out) = setup_conversations(vec![
        (id_match, titled("triage-001"), vec![]),
        (id_extra, titled("triage-0012"), vec![]),
        (id_prefixed, titled("xtriage-001"), vec![]),
    ]);

    let grep = Grep {
        regex: true,
        output: OutputKind::Ids,
        scopes: vec![Scope::Title],
        ..grep(r"\Atriage-\d{3}\z")
    };
    assert_eq!(run(grep, &mut ctx, &out), [id_match.to_string()]);
}

#[test]
fn a_literal_pattern_does_not_interpret_metacharacters() {
    // Without `--regex`, `a.c` matches the three literal characters and not
    // `abc`.
    let id_literal = make_id(15_300);
    let id_wildcard = make_id(15_400);
    let (mut ctx, out) = setup(vec![
        (
            id_literal,
            turn(vec![ConversationEvent::new(
                ChatRequest::from("a.c literal"),
                ts(),
            )]),
        ),
        (
            id_wildcard,
            turn(vec![ConversationEvent::new(
                ChatRequest::from("abc wildcard"),
                ts(),
            )]),
        ),
    ]);

    let grep = Grep {
        output: OutputKind::Ids,
        ..grep("a.c")
    };
    assert_eq!(run(grep, &mut ctx, &out), [id_literal.to_string()]);
}

// --- json -------------------------------------------------------------------

#[test]
fn json_carries_the_coordinate_and_submatches() {
    let id = make_id(16_000);
    let conv = Conversation {
        title: Some("A titled conversation".into()),
        ..Default::default()
    };
    let (mut ctx, out) = setup_conversations_with(
        vec![(
            id,
            conv,
            turn(vec![ConversationEvent::new(
                ChatRequest::from("find the needle here"),
                ts(),
            )]),
        )],
        OutputFormat::Json,
        OutputWidth::Unknown,
    );

    grep("needle").run(&mut ctx, vec![]).unwrap();
    ctx.printer.flush();
    let parsed: Value = serde_json::from_str(&out.lock().clone()).unwrap();

    assert_eq!(parsed[0]["id"], id.to_string());
    assert_eq!(parsed[0]["turn"], 1);
    assert_eq!(parsed[0]["scope"], "user");
    assert_eq!(parsed[0]["title"], "A titled conversation");
    assert_eq!(parsed[0]["text"], "find the needle here");
    assert_eq!(parsed[0]["match"], true);
    assert_eq!(
        parsed[0]["submatches"],
        json!([{
            "match": "needle",
            "start": 9,
            "end": 15,
        }])
    );
}

#[test]
fn json_submatch_offsets_index_the_emitted_text() {
    // A pattern that matches trailing whitespace is the case where trimming the
    // emitted text would shift every offset past the end of it.
    let id = make_id(16_050);
    let (mut ctx, out) = setup_json(vec![(
        id,
        turn(vec![ConversationEvent::new(
            ChatRequest::from("omega   "),
            ts(),
        )]),
    )]);

    let grep = Grep {
        regex: true,
        ..grep(r"\s+$")
    };
    grep.run(&mut ctx, vec![]).unwrap();
    ctx.printer.flush();
    let parsed: Value = serde_json::from_str(&out.lock().clone()).unwrap();

    let text = parsed[0]["text"].as_str().unwrap();
    let start = usize::try_from(parsed[0]["submatches"][0]["start"].as_u64().unwrap()).unwrap();
    let end = usize::try_from(parsed[0]["submatches"][0]["end"].as_u64().unwrap()).unwrap();
    let matched = parsed[0]["submatches"][0]["match"].as_str().unwrap();

    assert_eq!(text, "omega   ");
    assert_eq!(
        &text[start..end],
        matched,
        "offsets must slice the emitted text"
    );
}

#[test]
fn json_reports_a_title_hit_with_a_null_turn() {
    let id = make_id(16_100);
    let conv = Conversation {
        title: Some("phi-mark title".into()),
        ..Default::default()
    };
    let (mut ctx, out) = setup_conversations_with(
        vec![(id, conv, vec![])],
        OutputFormat::Json,
        OutputWidth::Unknown,
    );

    grep("phi-mark").run(&mut ctx, vec![]).unwrap();
    ctx.printer.flush();
    let parsed: Value = serde_json::from_str(&out.lock().clone()).unwrap();

    assert_eq!(parsed[0]["turn"], Value::Null);
    assert_eq!(parsed[0]["scope"], "title");
}

#[test]
fn json_marks_context_lines_and_leaves_their_submatches_empty() {
    let id = make_id(16_200);
    let (mut ctx, out) = setup_json(vec![(
        id,
        turn(vec![ConversationEvent::new(
            ChatRequest::from("before\nsigma-mark\nafter"),
            ts(),
        )]),
    )]);

    let grep = Grep {
        context: 1,
        ..grep("sigma-mark")
    };
    grep.run(&mut ctx, vec![]).unwrap();
    ctx.printer.flush();
    let parsed: Value = serde_json::from_str(&out.lock().clone()).unwrap();

    assert_eq!(parsed[0]["match"], false);
    assert_eq!(parsed[0]["submatches"], json!([]));
    assert_eq!(parsed[1]["match"], true);
    assert_eq!(parsed[2]["match"], false);
}

// --- flags ------------------------------------------------------------------

fn parse(args: &[&str]) -> Result<Grep, clap::Error> {
    #[derive(clap::Parser)]
    struct TestCli {
        #[command(flatten)]
        grep: Grep,
    }

    let mut argv = vec!["grep"];
    argv.extend_from_slice(args);
    TestCli::try_parse_from(argv).map(|cli| cli.grep)
}

#[test]
fn output_is_a_single_choice_rather_than_competing_flags() {
    assert_eq!(parse(&["x"]).unwrap().output, OutputKind::Hits);
    assert_eq!(
        parse(&["x", "--output", "ids"]).unwrap().output,
        OutputKind::Ids
    );
    assert_eq!(
        parse(&["x", "--output", "count"]).unwrap().output,
        OutputKind::Count
    );
    assert_eq!(
        parse(&["x", "--output", "text"]).unwrap().output,
        OutputKind::Text
    );
    assert!(parse(&["x", "--output", "bogus"]).is_err());
}

#[test]
fn heading_and_no_heading_are_mutually_exclusive() {
    assert!(parse(&["x", "--heading"]).is_ok());
    assert!(parse(&["x", "--no-heading"]).is_ok());
    assert!(parse(&["x", "--heading", "--no-heading"]).is_err());
}

#[test]
fn case_flags_are_mutually_exclusive() {
    assert!(parse(&["x", "--ignore-case"]).is_ok());
    assert!(parse(&["x", "--case-sensitive"]).is_ok());
    assert!(parse(&["x", "--ignore-case", "--case-sensitive"]).is_err());
}

#[test]
fn scope_does_not_swallow_the_pattern() {
    // `--scope` precedes the positional pattern in every documented example, so
    // it must take exactly one value per occurrence. A multi-value `--scope`
    // consumes the pattern as a second scope and fails on it.
    let grep = parse(&["--scope", "chat", "error"]).unwrap();
    assert_eq!(grep.pattern, ["error"]);
    assert_eq!(grep.scopes, vec![Scope::Chat]);
}

#[test]
fn a_pattern_is_required() {
    // A `Vec` positional is optional by default in clap, so without
    // `required = true` a bare `jp c grep` parses, joins to `""`, compiles the
    // empty regex, and matches every line of every conversation — exit 0 and a
    // full dump where `grep`(1) prints usage and exits 2.
    assert!(parse(&[]).is_err());

    // An explicit empty pattern still matches everything, as in `grep`(1).
    assert_eq!(parse(&[""]).unwrap().pattern, [""]);
}

#[test]
fn an_unquoted_multi_word_pattern_is_rejoined() {
    // The shell splits on whitespace, so `jp c grep two words` arrives as two
    // arguments. Requiring quotes for a phrase is friction with no purpose.
    let grep = parse(&["Store", "temporary", "files", "in", "workspace"]).unwrap();
    assert_eq!(grep.pattern, [
        "Store",
        "temporary",
        "files",
        "in",
        "workspace"
    ]);
}

#[test]
fn a_multi_word_pattern_does_not_swallow_trailing_flags() {
    // The risk of a variadic positional: a known flag after the pattern must
    // still parse as a flag, or every documented invocation breaks.
    let grep = parse(&["two", "words", "--scope", "chat", "--regex"]).unwrap();
    assert_eq!(grep.pattern, ["two", "words"]);
    assert_eq!(grep.scopes, vec![Scope::Chat]);
    assert!(grep.regex);
}

#[test]
fn a_pattern_starting_with_a_dash_needs_the_double_dash_separator() {
    // Unchanged from a single positional, but worth pinning: clap reads a
    // leading `-` as a flag, so `--` is the escape.
    assert!(parse(&["--nope"]).is_err());
    assert_eq!(parse(&["--", "--nope"]).unwrap().pattern, ["--nope"]);
}

#[test]
fn scope_accepts_repetition_and_comma_separation() {
    // The two spellings the doc comment promises.
    assert_eq!(
        parse(&["--scope", "user", "--scope", "assistant", "x"])
            .unwrap()
            .scopes,
        vec![Scope::User, Scope::Assistant]
    );
    assert_eq!(
        parse(&["--scope", "user,assistant", "x"]).unwrap().scopes,
        vec![Scope::User, Scope::Assistant]
    );
}

#[test]
fn short_flags_match_their_grep_meanings() {
    let grep = parse(&["x", "--context", "3", "-m", "2", "-r", "-s", "user"]).unwrap();
    assert_eq!(grep.context, 3);
    assert_eq!(grep.max_matches, NonZeroUsize::new(2));
    assert!(grep.regex);
    assert_eq!(grep.scopes.len(), 1);
}

#[test]
fn max_matches_rejects_zero() {
    // `--max-matches 0` would exhaust the budget before the first event, so
    // every conversation comes back empty and the run reports "no matches"
    // while matches exist — and `--quiet`, which caps at 1 internally, would
    // disagree with it.
    assert!(parse(&["x", "--max-matches", "1"]).is_ok());
    assert!(parse(&["x", "--max-matches", "0"]).is_err());
}

#[test]
fn limit_rejects_zero() {
    // `--limit 0` would drop real matches and report the run as finding
    // nothing, contradicting both the flag's purpose and exit status 0.
    assert!(parse(&["x", "--limit", "1"]).is_ok());
    assert!(parse(&["x", "--limit", "0"]).is_err());
}

#[test]
fn grep_declares_no_short_flag_that_a_global_already_owns() {
    // A local arg reusing a global's short parses without complaint and sets
    // *both*, so the user can't tell which flag they invoked and the two can
    // never diverge. `-q` belongs to the global `--quiet`, and `-C` is reserved
    // for the global `--no-cfg` (RFD 070).
    let cli = crate::Cli::try_parse_from(["jp", "conversation", "grep", "-q", "foo"]).unwrap();
    assert!(cli.globals.quiet, "-q must reach the global flag");

    assert!(
        crate::Cli::try_parse_from(["jp", "conversation", "grep", "-C", "1", "foo"]).is_err(),
        "-C must stay free for the global --no-cfg"
    );
}

#[test]
fn capital_shorts_stay_free_for_inverse_flags() {
    // Capital shorts are reserved for `--no-` variants across the CLI, and `-C`
    // specifically for the global `--no-cfg` (RFD 070). Neither `--context` nor
    // `--case-sensitive` may claim one.
    assert!(parse(&["x", "-C", "2"]).is_err());
    assert!(parse(&["x", "-S"]).is_err());
}

#[test]
fn lowercase_shorts_owned_by_globals_are_not_reclaimed() {
    // A local arg reusing a global's short parses without complaint and sets
    // *both*, so the user can't tell which flag they invoked. `-i` belongs to
    // the conversation-target flag on every `jp c` subcommand.
    assert!(parse(&["x", "--ignore-case"]).is_ok());
    assert!(!parse(&["x", "-i", "recent"]).unwrap().ignore_case);
}
