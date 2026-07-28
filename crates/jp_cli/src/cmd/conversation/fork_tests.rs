use std::{
    panic::{AssertUnwindSafe, catch_unwind, resume_unwind},
    sync::Arc,
    time::Duration,
};

use camino_tempfile::tempdir;
use chrono::{DateTime, TimeZone as _, Utc};
use jp_config::{AppConfig, PartialAppConfig};
use jp_conversation::{
    Conversation, ConversationEvent, ConversationId, ConversationStream,
    event::{ChatRequest, ChatResponse, TurnStart},
};
use jp_printer::{OutputFormat, Printer};
use jp_storage::backend::{FsStorageBackend, Projection};
use jp_workspace::Workspace;
use tokio::runtime::Runtime;

use super::*;
use crate::{
    Globals,
    cmd::{compact_flag::CompactFlag, conversation_id::PositionalIds},
};

/// Parse a [`TurnSelection`] from the flags a user would pass to `jp c fork`.
///
/// Going through clap keeps these cases pinned to the real flag surface rather
/// than to the struct's private fields.
fn selection(args: &[&str]) -> TurnSelection {
    #[derive(clap::Parser)]
    struct TestCli {
        #[command(flatten)]
        selection: TurnSelection,
    }

    let mut argv = vec!["fork"];
    argv.extend_from_slice(args);
    clap::Parser::try_parse_from(argv)
        .map(|cli: TestCli| cli.selection)
        .unwrap()
}

/// A stream of `count` turns, each holding one request, two minutes apart
/// starting at 2020-01-01 00:00:00 UTC.
///
/// Turn N starts at `00:(2N):00` and its request lands ten seconds later, so a
/// bound at an odd minute falls strictly inside a turn.
fn turns(count: u32) -> Vec<ConversationEvent> {
    (0..count)
        .flat_map(|turn| {
            [
                ConversationEvent::new(
                    TurnStart,
                    Utc.with_ymd_and_hms(2020, 1, 1, 0, turn * 2, 0).unwrap(),
                ),
                ConversationEvent::new(
                    ChatRequest::from(format!("Q{}", turn + 1)),
                    Utc.with_ymd_and_hms(2020, 1, 1, 0, turn * 2, 10).unwrap(),
                ),
            ]
        })
        .collect()
}

/// The request contents of a conversation, in stream order.
fn requests(stream: &ConversationStream) -> Vec<String> {
    stream
        .iter()
        .filter_map(|e| e.event.as_chat_request())
        .map(|r| r.content.clone())
        .collect()
}

#[test]
#[expect(clippy::too_many_lines)]
fn test_conversation_fork() {
    struct TestCase {
        args: Fork,
        setup: fn(&mut Ctx) -> ConversationId,
        assert: fn(Vec<(ConversationId, Conversation, ConversationStream)>, ConversationId),
    }

    let cases = vec![
        ("no conversation", TestCase {
            args: Fork {
                target: PositionalIds::default(),
                activate: false,
                range: TurnSelection::default(),
                title: None,
                compact: CompactFlag::default(),
                no_turns: false,
            },
            setup: |ctx| {
                let id = ConversationId::try_from(ctx.now()).unwrap();
                ctx.workspace.create_conversation_with_id(
                    id,
                    Conversation::default().with_last_activated_at(ctx.now()),
                    ctx.config(),
                );

                let h = ctx.workspace.acquire_conversation(&id).unwrap();
                let _lock = ctx.workspace.test_lock(h);
                id
            },

            assert: |mut convs, source_id| {
                assert_eq!(convs.len(), 2);
                convs.sort_by_key(|v| v.0);

                // source_id is the original conversation
                assert_eq!(source_id, convs[0].0);

                assert!(convs[0].1.last_activated_at < convs[1].1.last_activated_at);
                assert!(convs[0].2.created_at < convs[1].2.created_at);

                for (_, conv, stream) in &mut convs {
                    conv.last_activated_at = DateTime::<Utc>::UNIX_EPOCH;
                    stream.created_at = DateTime::<Utc>::UNIX_EPOCH;
                }

                assert!(convs[0].0.timestamp() < convs[1].0.timestamp());
                assert_eq!(convs[0].1, convs[1].1);
                assert_eq!(convs[0].2, convs[1].2);
            },
        }),
        ("conversation with events", TestCase {
            args: Fork {
                target: PositionalIds::default(),
                activate: false,
                range: TurnSelection::default(),
                title: None,
                compact: CompactFlag::default(),
                no_turns: false,
            },
            setup: |ctx| {
                let id = ConversationId::try_from(ctx.now()).unwrap();
                ctx.workspace.create_conversation_with_id(
                    id,
                    Conversation::default().with_last_activated_at(ctx.now()),
                    ctx.config(),
                );

                let h = ctx.workspace.acquire_conversation(&id).unwrap();
                let lock = ctx.workspace.test_lock(h);
                lock.as_mut().update_events(|e| {
                    e.extend(vec![
                        ConversationEvent::new(
                            ChatRequest::from("foo"),
                            Utc.with_ymd_and_hms(2020, 1, 1, 0, 0, 0).unwrap(),
                        ),
                        ConversationEvent::new(
                            ChatResponse::message("bar"),
                            Utc.with_ymd_and_hms(2020, 1, 1, 0, 1, 0).unwrap(),
                        ),
                    ]);
                });
                id
            },
            assert: |mut convs, source_id| {
                assert_eq!(convs.len(), 2);
                convs.sort_by_key(|v| v.0);
                assert_eq!(source_id, convs[0].0);

                assert!(convs[0].1.last_activated_at < convs[1].1.last_activated_at);
                assert!(convs[0].2.created_at < convs[1].2.created_at);

                for (_, conv, stream) in &mut convs {
                    conv.last_activated_at = DateTime::<Utc>::UNIX_EPOCH;
                    stream.created_at = DateTime::<Utc>::UNIX_EPOCH;
                }

                assert!(convs[0].0.timestamp() < convs[1].0.timestamp());
                assert_eq!(convs[0].1, convs[1].1);
                convs[0].2.sanitize();
                assert_eq!(convs[0].2, convs[1].2);
            },
        }),
        ("no turns keeps config but drops every turn", TestCase {
            args: Fork {
                target: PositionalIds::default(),
                activate: false,
                range: TurnSelection::default(),
                title: None,
                compact: CompactFlag::default(),
                no_turns: true,
            },
            setup: |ctx| {
                let id = ConversationId::try_from(ctx.now()).unwrap();
                ctx.workspace.create_conversation_with_id(
                    id,
                    Conversation::default().with_last_activated_at(ctx.now()),
                    ctx.config(),
                );

                let h = ctx.workspace.acquire_conversation(&id).unwrap();
                let lock = ctx.workspace.test_lock(h);
                lock.as_mut().update_events(|e| {
                    // A mid-conversation config change the fork must inherit,
                    // folded into its base config.
                    let mut partial = PartialAppConfig::empty();
                    partial.style.code.color = Some(false);
                    e.add_config_delta(partial);
                    e.extend(vec![
                        ConversationEvent::new(
                            ChatRequest::from("foo"),
                            Utc.with_ymd_and_hms(2020, 1, 1, 0, 0, 0).unwrap(),
                        ),
                        ConversationEvent::new(
                            ChatResponse::message("bar"),
                            Utc.with_ymd_and_hms(2020, 1, 1, 0, 1, 0).unwrap(),
                        ),
                    ]);
                });
                id
            },
            assert: |mut convs, _| {
                assert_eq!(convs.len(), 2);
                convs.sort_by_key(|v| v.0);
                let source = &convs[0].2;
                let fork = &convs[1].2;

                // Source keeps its turns; the fork has none.
                assert_eq!(source.iter().count(), 2);
                assert!(fork.is_empty());
                assert_eq!(fork.iter().count(), 0);

                // The fork's effective config matches the source's, including
                // the mid-conversation delta folded into its base config.
                assert_eq!(fork.config().unwrap(), source.config().unwrap());
                assert!(!fork.config().unwrap().style.code.color);
            },
        }),
        ("with activate", TestCase {
            args: Fork {
                target: PositionalIds::default(),
                activate: true,
                range: TurnSelection::default(),
                title: None,
                compact: CompactFlag::default(),
                no_turns: false,
            },
            setup: |ctx| {
                let id = ConversationId::try_from(ctx.now()).unwrap();
                ctx.workspace.create_conversation_with_id(
                    id,
                    Conversation::default().with_last_activated_at(ctx.now()),
                    ctx.config(),
                );

                let h = ctx.workspace.acquire_conversation(&id).unwrap();
                let lock = ctx.workspace.test_lock(h);
                lock.as_mut().update_events(|e| {
                    e.extend(vec![
                        ConversationEvent::new(
                            ChatRequest::from("foo"),
                            Utc.with_ymd_and_hms(2020, 1, 1, 0, 0, 0).unwrap(),
                        ),
                        ConversationEvent::new(
                            ChatResponse::message("bar"),
                            Utc.with_ymd_and_hms(2020, 1, 1, 0, 1, 0).unwrap(),
                        ),
                    ]);
                });
                id
            },

            assert: |mut convs, source_id| {
                assert_eq!(convs.len(), 2);
                convs.sort_by_key(|v| v.0);

                // source is the first (earlier timestamp)
                assert_eq!(source_id, convs[0].0);
                // fork has a more recent last_activated_at
                assert!(convs[1].1.last_activated_at > convs[0].1.last_activated_at);

                for (_, conv, stream) in &mut convs {
                    conv.last_activated_at = DateTime::<Utc>::UNIX_EPOCH;
                    stream.created_at = DateTime::<Utc>::UNIX_EPOCH;
                }

                assert!(convs[0].0.timestamp() < convs[1].0.timestamp());
                assert_eq!(convs[0].1, convs[1].1);
                convs[0].2.sanitize();
                assert_eq!(convs[0].2, convs[1].2);
            },
        }),
        ("with from", TestCase {
            args: Fork {
                target: PositionalIds::default(),
                activate: false,
                range: selection(&["--from", "2020-01-01T00:01:00Z"]),
                title: None,
                compact: CompactFlag::default(),
                no_turns: false,
            },
            setup: |ctx| {
                let id = ConversationId::try_from(ctx.now()).unwrap();
                ctx.workspace.create_conversation_with_id(
                    id,
                    Conversation::default().with_last_activated_at(ctx.now()),
                    ctx.config(),
                );

                let h = ctx.workspace.acquire_conversation(&id).unwrap();
                let lock = ctx.workspace.test_lock(h);
                lock.as_mut().update_events(|e| e.extend(turns(3)));
                id
            },

            assert: |convs, _| {
                assert_eq!(convs.len(), 2);
                assert_eq!(requests(&convs[0].2), vec!["Q1", "Q2", "Q3"]);
                // 00:01:00 falls inside turn 1 (00:00:00-00:02:00), so the fork
                // starts at turn 2 — the first turn to begin after the cutoff.
                // The bound never splits turn 1.
                assert_eq!(requests(&convs[1].2), vec!["Q2", "Q3"]);
            },
        }),
        ("with to", TestCase {
            args: Fork {
                target: PositionalIds::default(),
                activate: false,
                range: selection(&["--to", "2020-01-01T00:03:00Z"]),
                title: None,
                compact: CompactFlag::default(),
                no_turns: false,
            },
            setup: |ctx| {
                let id = ConversationId::try_from(ctx.now()).unwrap();
                ctx.workspace.create_conversation_with_id(
                    id,
                    Conversation::default().with_last_activated_at(ctx.now()),
                    ctx.config(),
                );

                let h = ctx.workspace.acquire_conversation(&id).unwrap();
                let lock = ctx.workspace.test_lock(h);
                lock.as_mut().update_events(|e| e.extend(turns(3)));
                id
            },

            assert: |convs, _| {
                assert_eq!(convs.len(), 2);
                assert_eq!(requests(&convs[0].2), vec!["Q1", "Q2", "Q3"]);
                // 00:03:00 falls inside turn 2 (00:02:00-00:04:00). `--to` is
                // inclusive of the turn it lands in, so turn 2 is kept whole.
                assert_eq!(requests(&convs[1].2), vec!["Q1", "Q2"]);
            },
        }),
        ("with last (default 1)", TestCase {
            args: Fork {
                target: PositionalIds::default(),
                activate: false,
                range: selection(&["--last"]),
                title: None,
                compact: CompactFlag::default(),
                no_turns: false,
            },
            setup: |ctx| {
                let id = ConversationId::try_from(ctx.now()).unwrap();
                ctx.workspace.create_conversation_with_id(
                    id,
                    Conversation::default().with_last_activated_at(ctx.now()),
                    ctx.config(),
                );

                let h = ctx.workspace.acquire_conversation(&id).unwrap();
                let lock = ctx.workspace.test_lock(h);
                lock.as_mut().update_events(|e| {
                    e.extend(vec![
                        ConversationEvent::new(
                            TurnStart,
                            Utc.with_ymd_and_hms(2020, 1, 1, 0, 0, 0).unwrap(),
                        ),
                        ConversationEvent::new(
                            ChatRequest::from("first question"),
                            Utc.with_ymd_and_hms(2020, 1, 1, 0, 1, 0).unwrap(),
                        ),
                        ConversationEvent::new(
                            ChatResponse::message("first answer"),
                            Utc.with_ymd_and_hms(2020, 1, 1, 0, 2, 0).unwrap(),
                        ),
                        ConversationEvent::new(
                            TurnStart,
                            Utc.with_ymd_and_hms(2020, 1, 1, 0, 3, 0).unwrap(),
                        ),
                        ConversationEvent::new(
                            ChatRequest::from("second question"),
                            Utc.with_ymd_and_hms(2020, 1, 1, 0, 4, 0).unwrap(),
                        ),
                        ConversationEvent::new(
                            ChatResponse::message("second answer"),
                            Utc.with_ymd_and_hms(2020, 1, 1, 0, 5, 0).unwrap(),
                        ),
                        ConversationEvent::new(
                            TurnStart,
                            Utc.with_ymd_and_hms(2020, 1, 1, 0, 6, 0).unwrap(),
                        ),
                        ConversationEvent::new(
                            ChatRequest::from("third question"),
                            Utc.with_ymd_and_hms(2020, 1, 1, 0, 7, 0).unwrap(),
                        ),
                        ConversationEvent::new(
                            ChatResponse::message("third answer"),
                            Utc.with_ymd_and_hms(2020, 1, 1, 0, 8, 0).unwrap(),
                        ),
                    ]);
                });
                id
            },
            assert: |convs, _| {
                assert_eq!(convs.len(), 2);
                // original has all 9 events
                assert_eq!(convs[0].2.iter().count(), 9);
                // forked has last turn: TurnStart(2) + request + response
                assert_eq!(convs[1].2.iter().count(), 3);
                assert_eq!(
                    convs[1].2.first().unwrap().event.timestamp,
                    Utc.with_ymd_and_hms(2020, 1, 1, 0, 6, 0).unwrap(),
                );
                assert_eq!(
                    convs[1].2.last().unwrap().event.timestamp,
                    Utc.with_ymd_and_hms(2020, 1, 1, 0, 8, 0).unwrap(),
                );
            },
        }),
        ("with last 2", TestCase {
            args: Fork {
                target: PositionalIds::default(),
                activate: false,
                range: selection(&["--last", "2"]),
                title: None,
                compact: CompactFlag::default(),
                no_turns: false,
            },
            setup: |ctx| {
                let id = ConversationId::try_from(ctx.now()).unwrap();
                ctx.workspace.create_conversation_with_id(
                    id,
                    Conversation::default().with_last_activated_at(ctx.now()),
                    ctx.config(),
                );

                let h = ctx.workspace.acquire_conversation(&id).unwrap();
                let lock = ctx.workspace.test_lock(h);
                lock.as_mut().update_events(|e| {
                    e.extend(vec![
                        ConversationEvent::new(
                            TurnStart,
                            Utc.with_ymd_and_hms(2020, 1, 1, 0, 0, 0).unwrap(),
                        ),
                        ConversationEvent::new(
                            ChatRequest::from("first question"),
                            Utc.with_ymd_and_hms(2020, 1, 1, 0, 1, 0).unwrap(),
                        ),
                        ConversationEvent::new(
                            ChatResponse::message("first answer"),
                            Utc.with_ymd_and_hms(2020, 1, 1, 0, 2, 0).unwrap(),
                        ),
                        ConversationEvent::new(
                            TurnStart,
                            Utc.with_ymd_and_hms(2020, 1, 1, 0, 3, 0).unwrap(),
                        ),
                        ConversationEvent::new(
                            ChatRequest::from("second question"),
                            Utc.with_ymd_and_hms(2020, 1, 1, 0, 4, 0).unwrap(),
                        ),
                        ConversationEvent::new(
                            ChatResponse::message("second answer"),
                            Utc.with_ymd_and_hms(2020, 1, 1, 0, 5, 0).unwrap(),
                        ),
                        ConversationEvent::new(
                            TurnStart,
                            Utc.with_ymd_and_hms(2020, 1, 1, 0, 6, 0).unwrap(),
                        ),
                        ConversationEvent::new(
                            ChatRequest::from("third question"),
                            Utc.with_ymd_and_hms(2020, 1, 1, 0, 7, 0).unwrap(),
                        ),
                        ConversationEvent::new(
                            ChatResponse::message("third answer"),
                            Utc.with_ymd_and_hms(2020, 1, 1, 0, 8, 0).unwrap(),
                        ),
                    ]);
                });
                id
            },
            assert: |convs, _| {
                assert_eq!(convs.len(), 2);
                assert_eq!(convs[0].2.iter().count(), 9);
                // forked has last 2 turns: TurnStart(1) + 2 events + TurnStart(2) + 2 events
                assert_eq!(convs[1].2.iter().count(), 6);
                assert_eq!(
                    convs[1].2.first().unwrap().event.timestamp,
                    Utc.with_ymd_and_hms(2020, 1, 1, 0, 3, 0).unwrap(),
                );
                assert_eq!(
                    convs[1].2.last().unwrap().event.timestamp,
                    Utc.with_ymd_and_hms(2020, 1, 1, 0, 8, 0).unwrap(),
                );
            },
        }),
        ("with last exceeding turn count", TestCase {
            args: Fork {
                target: PositionalIds::default(),
                activate: false,
                range: selection(&["--last", "10"]),
                title: None,
                compact: CompactFlag::default(),
                no_turns: false,
            },
            setup: |ctx| {
                let id = ConversationId::try_from(ctx.now()).unwrap();
                ctx.workspace.create_conversation_with_id(
                    id,
                    Conversation::default().with_last_activated_at(ctx.now()),
                    ctx.config(),
                );

                let h = ctx.workspace.acquire_conversation(&id).unwrap();
                let lock = ctx.workspace.test_lock(h);
                lock.as_mut().update_events(|e| {
                    e.extend(vec![
                        ConversationEvent::new(
                            TurnStart,
                            Utc.with_ymd_and_hms(2020, 1, 1, 0, 0, 0).unwrap(),
                        ),
                        ConversationEvent::new(
                            ChatRequest::from("only question"),
                            Utc.with_ymd_and_hms(2020, 1, 1, 0, 1, 0).unwrap(),
                        ),
                        ConversationEvent::new(
                            ChatResponse::message("only answer"),
                            Utc.with_ymd_and_hms(2020, 1, 1, 0, 2, 0).unwrap(),
                        ),
                    ]);
                });
                id
            },
            assert: |convs, _| {
                assert_eq!(convs.len(), 2);
                // all events kept since --last 10 > 1 turn
                assert_eq!(convs[0].2.iter().count(), 3);
                assert_eq!(convs[1].2.iter().count(), 3);
            },
        }),
        ("with last and no turn markers", TestCase {
            args: Fork {
                target: PositionalIds::default(),
                activate: false,
                range: selection(&["--last", "1"]),
                title: None,
                compact: CompactFlag::default(),
                no_turns: false,
            },
            setup: |ctx| {
                let id = ConversationId::try_from(ctx.now()).unwrap();
                ctx.workspace.create_conversation_with_id(
                    id,
                    Conversation::default().with_last_activated_at(ctx.now()),
                    ctx.config(),
                );

                let h = ctx.workspace.acquire_conversation(&id).unwrap();
                let lock = ctx.workspace.test_lock(h);
                lock.as_mut().update_events(|e| {
                    e.extend(vec![
                        ConversationEvent::new(
                            ChatRequest::from("foo"),
                            Utc.with_ymd_and_hms(2020, 1, 1, 0, 0, 0).unwrap(),
                        ),
                        ConversationEvent::new(
                            ChatResponse::message("bar"),
                            Utc.with_ymd_and_hms(2020, 1, 1, 0, 1, 0).unwrap(),
                        ),
                    ]);
                });
                id
            },
            assert: |convs, _| {
                assert_eq!(convs.len(), 2);
                // no TurnStart events so --last is a no-op,
                // but sanitize injects a TurnStart
                assert_eq!(convs[0].2.iter().count(), 2);
                assert_eq!(convs[1].2.iter().count(), 3);
            },
        }),
        ("with first (default 1)", TestCase {
            args: Fork {
                target: PositionalIds::default(),
                activate: false,
                range: selection(&["--first"]),
                compact: CompactFlag::default(),
                no_turns: false,
                title: None,
            },
            setup: |ctx| {
                let id = ConversationId::try_from(ctx.now()).unwrap();
                ctx.workspace.create_conversation_with_id(
                    id,
                    Conversation::default().with_last_activated_at(ctx.now()),
                    ctx.config(),
                );

                let h = ctx.workspace.acquire_conversation(&id).unwrap();
                let lock = ctx.workspace.test_lock(h);
                lock.as_mut().update_events(|e| {
                    e.extend(vec![
                        ConversationEvent::new(
                            TurnStart,
                            Utc.with_ymd_and_hms(2020, 1, 1, 0, 0, 0).unwrap(),
                        ),
                        ConversationEvent::new(
                            ChatRequest::from("first question"),
                            Utc.with_ymd_and_hms(2020, 1, 1, 0, 1, 0).unwrap(),
                        ),
                        ConversationEvent::new(
                            ChatResponse::message("first answer"),
                            Utc.with_ymd_and_hms(2020, 1, 1, 0, 2, 0).unwrap(),
                        ),
                        ConversationEvent::new(
                            TurnStart,
                            Utc.with_ymd_and_hms(2020, 1, 1, 0, 3, 0).unwrap(),
                        ),
                        ConversationEvent::new(
                            ChatRequest::from("second question"),
                            Utc.with_ymd_and_hms(2020, 1, 1, 0, 4, 0).unwrap(),
                        ),
                        ConversationEvent::new(
                            ChatResponse::message("second answer"),
                            Utc.with_ymd_and_hms(2020, 1, 1, 0, 5, 0).unwrap(),
                        ),
                        ConversationEvent::new(
                            TurnStart,
                            Utc.with_ymd_and_hms(2020, 1, 1, 0, 6, 0).unwrap(),
                        ),
                        ConversationEvent::new(
                            ChatRequest::from("third question"),
                            Utc.with_ymd_and_hms(2020, 1, 1, 0, 7, 0).unwrap(),
                        ),
                        ConversationEvent::new(
                            ChatResponse::message("third answer"),
                            Utc.with_ymd_and_hms(2020, 1, 1, 0, 8, 0).unwrap(),
                        ),
                    ]);
                });
                id
            },
            assert: |convs, _| {
                assert_eq!(convs.len(), 2);
                assert_eq!(convs[0].2.iter().count(), 9);
                // forked has first turn only: TurnStart + request + response
                assert_eq!(convs[1].2.iter().count(), 3);
                assert_eq!(
                    convs[1].2.first().unwrap().event.timestamp,
                    Utc.with_ymd_and_hms(2020, 1, 1, 0, 0, 0).unwrap(),
                );
                assert_eq!(
                    convs[1].2.last().unwrap().event.timestamp,
                    Utc.with_ymd_and_hms(2020, 1, 1, 0, 2, 0).unwrap(),
                );
            },
        }),
        ("with first 2 and last 1", TestCase {
            args: Fork {
                target: PositionalIds::default(),
                activate: false,
                range: selection(&["--first", "2", "--last", "1"]),
                compact: CompactFlag::default(),
                no_turns: false,
                title: None,
            },
            setup: |ctx| {
                let id = ConversationId::try_from(ctx.now()).unwrap();
                ctx.workspace.create_conversation_with_id(
                    id,
                    Conversation::default().with_last_activated_at(ctx.now()),
                    ctx.config(),
                );

                let h = ctx.workspace.acquire_conversation(&id).unwrap();
                let lock = ctx.workspace.test_lock(h);
                lock.as_mut().update_events(|e| {
                    e.extend(vec![
                        ConversationEvent::new(
                            TurnStart,
                            Utc.with_ymd_and_hms(2020, 1, 1, 0, 0, 0).unwrap(),
                        ),
                        ConversationEvent::new(
                            ChatRequest::from("Q1"),
                            Utc.with_ymd_and_hms(2020, 1, 1, 0, 1, 0).unwrap(),
                        ),
                        ConversationEvent::new(
                            TurnStart,
                            Utc.with_ymd_and_hms(2020, 1, 1, 0, 2, 0).unwrap(),
                        ),
                        ConversationEvent::new(
                            ChatRequest::from("Q2"),
                            Utc.with_ymd_and_hms(2020, 1, 1, 0, 3, 0).unwrap(),
                        ),
                        ConversationEvent::new(
                            TurnStart,
                            Utc.with_ymd_and_hms(2020, 1, 1, 0, 4, 0).unwrap(),
                        ),
                        ConversationEvent::new(
                            ChatRequest::from("Q3"),
                            Utc.with_ymd_and_hms(2020, 1, 1, 0, 5, 0).unwrap(),
                        ),
                        ConversationEvent::new(
                            TurnStart,
                            Utc.with_ymd_and_hms(2020, 1, 1, 0, 6, 0).unwrap(),
                        ),
                        ConversationEvent::new(
                            ChatRequest::from("Q4"),
                            Utc.with_ymd_and_hms(2020, 1, 1, 0, 7, 0).unwrap(),
                        ),
                    ]);
                });
                id
            },
            assert: |convs, _| {
                assert_eq!(convs.len(), 2);
                // First 2 + last 1 of 4 turns — drop turn 3 (Q3).
                let requests: Vec<_> = convs[1]
                    .2
                    .iter()
                    .filter_map(|e| e.event.as_chat_request())
                    .map(|r| r.content.clone())
                    .collect();
                assert_eq!(requests, vec!["Q1", "Q2", "Q4"]);
            },
        }),
        ("with first exceeding turn count", TestCase {
            args: Fork {
                target: PositionalIds::default(),
                activate: false,
                range: selection(&["--first", "10"]),
                compact: CompactFlag::default(),
                no_turns: false,
                title: None,
            },
            setup: |ctx| {
                let id = ConversationId::try_from(ctx.now()).unwrap();
                ctx.workspace.create_conversation_with_id(
                    id,
                    Conversation::default().with_last_activated_at(ctx.now()),
                    ctx.config(),
                );

                let h = ctx.workspace.acquire_conversation(&id).unwrap();
                let lock = ctx.workspace.test_lock(h);
                lock.as_mut().update_events(|e| {
                    e.extend(vec![
                        ConversationEvent::new(
                            TurnStart,
                            Utc.with_ymd_and_hms(2020, 1, 1, 0, 0, 0).unwrap(),
                        ),
                        ConversationEvent::new(
                            ChatRequest::from("only question"),
                            Utc.with_ymd_and_hms(2020, 1, 1, 0, 1, 0).unwrap(),
                        ),
                        ConversationEvent::new(
                            ChatResponse::message("only answer"),
                            Utc.with_ymd_and_hms(2020, 1, 1, 0, 2, 0).unwrap(),
                        ),
                    ]);
                });
                id
            },
            assert: |convs, _| {
                assert_eq!(convs.len(), 2);
                // All events kept since --first 10 > 1 turn.
                assert_eq!(convs[0].2.iter().count(), 3);
                assert_eq!(convs[1].2.iter().count(), 3);
            },
        }),
        ("with custom title", TestCase {
            args: Fork {
                target: PositionalIds::default(),
                activate: false,
                range: TurnSelection::default(),
                compact: CompactFlag::default(),
                no_turns: false,
                title: Some("my custom title".to_owned()),
            },
            setup: |ctx| {
                let id = ConversationId::try_from(ctx.now()).unwrap();
                ctx.workspace.create_conversation_with_id(
                    id,
                    Conversation::new("original title").with_last_activated_at(ctx.now()),
                    ctx.config(),
                );

                let h = ctx.workspace.acquire_conversation(&id).unwrap();
                let _lock = ctx.workspace.test_lock(h);
                id
            },
            assert: |mut convs, source_id| {
                assert_eq!(convs.len(), 2);
                convs.sort_by_key(|v| v.0);
                assert_eq!(source_id, convs[0].0);

                assert_eq!(convs[0].1.title.as_deref(), Some("original title"));
                assert_eq!(convs[1].1.title.as_deref(), Some("my custom title"));
            },
        }),
        ("with from and to", TestCase {
            args: Fork {
                target: PositionalIds::default(),
                activate: false,
                range: selection(&[
                    "--from",
                    "2020-01-01T00:01:00Z",
                    "--to",
                    "2020-01-01T00:03:00Z",
                ]),
                title: None,
                compact: CompactFlag::default(),
                no_turns: false,
            },
            setup: |ctx| {
                let id = ConversationId::try_from(ctx.now()).unwrap();
                ctx.workspace.create_conversation_with_id(
                    id,
                    Conversation::default().with_last_activated_at(ctx.now()),
                    ctx.config(),
                );

                let h = ctx.workspace.acquire_conversation(&id).unwrap();
                let lock = ctx.workspace.test_lock(h);
                lock.as_mut().update_events(|e| e.extend(turns(4)));
                id
            },

            assert: |convs, _| {
                assert_eq!(convs.len(), 2);
                assert_eq!(requests(&convs[0].2), vec!["Q1", "Q2", "Q3", "Q4"]);
                // Both cutoffs fall mid-turn: `--from` starts at turn 2 and
                // `--to` ends at turn 2, leaving exactly that turn.
                assert_eq!(requests(&convs[1].2), vec!["Q2"]);
            },
        }),
        ("with turn range", TestCase {
            args: Fork {
                target: PositionalIds::default(),
                activate: false,
                range: selection(&["--turn", "2..3"]),
                title: None,
                compact: CompactFlag::default(),
                no_turns: false,
            },
            setup: |ctx| {
                let id = ConversationId::try_from(ctx.now()).unwrap();
                ctx.workspace.create_conversation_with_id(
                    id,
                    Conversation::default().with_last_activated_at(ctx.now()),
                    ctx.config(),
                );

                let h = ctx.workspace.acquire_conversation(&id).unwrap();
                let lock = ctx.workspace.test_lock(h);
                lock.as_mut().update_events(|e| e.extend(turns(5)));
                id
            },

            assert: |convs, _| {
                assert_eq!(convs.len(), 2);
                // `--turn A..B` is inclusive on both ends.
                assert_eq!(requests(&convs[1].2), vec!["Q2", "Q3"]);
            },
        }),
        ("with keep flags", TestCase {
            args: Fork {
                target: PositionalIds::default(),
                activate: false,
                range: selection(&["--keep-first", "1", "--keep-last", "1"]),
                title: None,
                compact: CompactFlag::default(),
                no_turns: false,
            },
            setup: |ctx| {
                let id = ConversationId::try_from(ctx.now()).unwrap();
                ctx.workspace.create_conversation_with_id(
                    id,
                    Conversation::default().with_last_activated_at(ctx.now()),
                    ctx.config(),
                );

                let h = ctx.workspace.acquire_conversation(&id).unwrap();
                let lock = ctx.workspace.test_lock(h);
                lock.as_mut().update_events(|e| e.extend(turns(5)));
                id
            },

            assert: |convs, _| {
                assert_eq!(convs.len(), 2);
                // The keep flags protect the first and last turns from the
                // selection, so the fork inherits only the middle three.
                assert_eq!(requests(&convs[1].2), vec!["Q2", "Q3", "Q4"]);
            },
        }),
    ];

    for (name, case) in cases {
        let tmp = tempdir().unwrap();
        let (printer, out, _err) = Printer::memory(OutputFormat::TextPretty);

        let config = AppConfig::new_test();
        let storage = tmp.path().join(".jp");
        let user = tmp.path().join("user");
        let fs = Arc::new(
            FsStorageBackend::new(&storage)
                .unwrap()
                .with_user_storage(&user, None, "abc")
                .unwrap(),
        );
        let workspace = Workspace::new(tmp.path()).with_backend(fs);
        let mut ctx = Ctx::new(
            workspace,
            None,
            Runtime::new().unwrap(),
            Globals::default(),
            config,
            None,
            printer,
        );

        let source_id =
            catch_unwind(AssertUnwindSafe(|| (case.setup)(&mut ctx))).unwrap_or_else(|panic| {
                eprintln!("Test case '{name}' panicked.");
                resume_unwind(panic);
            });

        ctx.set_now(DateTime::<Utc>::UNIX_EPOCH + Duration::from_secs(1));

        let source_handle = ctx.workspace.acquire_conversation(&source_id).unwrap();
        tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(case.args.run(&mut ctx, &[source_handle]))
            .unwrap();
        ctx.printer.flush();
        assert_eq!(*out.lock(), "Conversation forked.\n");

        let mut conversations: Vec<_> = ctx
            .workspace
            .conversations()
            .map(|(id, conv)| (*id, conv.clone()))
            .collect();
        conversations.sort_by_key(|(id, _)| *id);

        let conversations = conversations
            .into_iter()
            .map(|(id, conv)| {
                let h = ctx.workspace.acquire_conversation(&id).unwrap();
                (id, conv, ctx.workspace.events(&h).unwrap().clone())
            })
            .collect();

        let active_id = source_id;

        if let Err(panic) = catch_unwind(AssertUnwindSafe(|| {
            (case.assert)(conversations, active_id);
        })) {
            eprintln!("Test case '{name}' panicked.");
            resume_unwind(panic);
        }
    }
}

/// Create two conversations with distinct content, fork only one, and verify
/// the fork carries the source's events (not the other conversation's).
#[test]
#[expect(clippy::too_many_lines)]
fn fork_targets_correct_source() {
    let tmp = tempdir().unwrap();
    let (printer, _, _) = Printer::memory(OutputFormat::TextPretty);
    let config = AppConfig::new_test();
    let storage = tmp.path().join(".jp");
    let user = tmp.path().join("user");
    let fs = std::sync::Arc::new(
        FsStorageBackend::new(&storage)
            .unwrap()
            .with_user_storage(&user, None, "abc")
            .unwrap(),
    );
    let workspace = Workspace::new(tmp.path()).with_backend(fs);
    let mut ctx = Ctx::new(
        workspace,
        None,
        Runtime::new().unwrap(),
        Globals::default(),
        config,
        None,
        printer,
    );

    // Create conversation A with distinct content.
    let id_a = ConversationId::try_from(ctx.now()).unwrap();
    ctx.workspace.create_conversation_with_id(
        id_a,
        Conversation::new("conv-a").with_last_activated_at(ctx.now()),
        ctx.config(),
    );
    let h_a = ctx.workspace.acquire_conversation(&id_a).unwrap();
    let lock_a = ctx.workspace.test_lock(h_a);
    lock_a.as_mut().update_events(|e| {
        e.extend(vec![
            ConversationEvent::new(
                ChatRequest::from("alpha question"),
                Utc.with_ymd_and_hms(2020, 1, 1, 0, 0, 0).unwrap(),
            ),
            ConversationEvent::new(
                ChatResponse::message("alpha answer"),
                Utc.with_ymd_and_hms(2020, 1, 1, 0, 1, 0).unwrap(),
            ),
        ]);
    });
    drop(lock_a);

    ctx.set_now(ctx.now() + Duration::from_secs(1));

    // Create conversation B with different content.
    let id_b = ConversationId::try_from(ctx.now()).unwrap();
    ctx.workspace.create_conversation_with_id(
        id_b,
        Conversation::new("conv-b").with_last_activated_at(ctx.now()),
        ctx.config(),
    );
    let h_b = ctx.workspace.acquire_conversation(&id_b).unwrap();
    let lock_b = ctx.workspace.test_lock(h_b);
    lock_b.as_mut().update_events(|e| {
        e.extend(vec![
            ConversationEvent::new(
                ChatRequest::from("beta question"),
                Utc.with_ymd_and_hms(2020, 2, 1, 0, 0, 0).unwrap(),
            ),
            ConversationEvent::new(
                ChatResponse::message("beta answer"),
                Utc.with_ymd_and_hms(2020, 2, 1, 0, 1, 0).unwrap(),
            ),
        ]);
    });
    drop(lock_b);

    ctx.set_now(ctx.now() + Duration::from_secs(1));

    // Fork conversation B only.
    let fork = Fork {
        target: PositionalIds::default(),
        activate: false,
        range: TurnSelection::default(),
        compact: CompactFlag::default(),
        no_turns: false,
        title: Some("forked-from-b".to_owned()),
    };
    let handle_b = ctx.workspace.acquire_conversation(&id_b).unwrap();
    tokio::runtime::Runtime::new()
        .unwrap()
        .block_on(fork.run(&mut ctx, &[handle_b]))
        .unwrap();

    // Should now have 3 conversations: A, B, and the fork.
    let all: Vec<_> = ctx
        .workspace
        .conversations()
        .map(|(id, conv)| (*id, conv.clone()))
        .collect();
    assert_eq!(all.len(), 3);

    // Find the forked conversation (the one that is neither A nor B).
    let (fork_id, fork_conv) = all
        .iter()
        .find(|(id, _)| *id != id_a && *id != id_b)
        .unwrap();

    // Title comes from the --title flag, not from the source.
    assert_eq!(fork_conv.title.as_deref(), Some("forked-from-b"));

    // The fork should carry B's content, not A's.
    let fork_handle = ctx.workspace.acquire_conversation(fork_id).unwrap();
    let fork_events = ctx.workspace.events(&fork_handle).unwrap();
    let requests: Vec<&str> = fork_events
        .iter()
        .filter_map(|e| e.event.as_chat_request())
        .map(|r| r.content.as_str())
        .collect();
    assert_eq!(requests, vec!["beta question"]);

    // Conversation A is untouched.
    let handle_a = ctx.workspace.acquire_conversation(&id_a).unwrap();
    let a_events = ctx.workspace.events(&handle_a).unwrap();
    let a_requests: Vec<&str> = a_events
        .iter()
        .filter_map(|e| e.event.as_chat_request())
        .map(|r| r.content.as_str())
        .collect();
    assert_eq!(a_requests, vec!["alpha question"]);
}

/// An out-of-range `--turn` on *any* source must be rejected before *any* fork
/// is created.
///
/// Validating inside the mutation loop would fork the first source, then error
/// on the second — leaving a conversation behind that the failed command
/// appears not to have created.
#[test]
fn turn_out_of_range_on_a_later_source_forks_nothing() {
    let tmp = tempdir().unwrap();
    let (printer, _, _) = Printer::memory(OutputFormat::TextPretty);
    let config = AppConfig::new_test();
    let storage = tmp.path().join(".jp");
    let user = tmp.path().join("user");
    let fs = Arc::new(
        FsStorageBackend::new(&storage)
            .unwrap()
            .with_user_storage(&user, None, "abc")
            .unwrap(),
    );
    let workspace = Workspace::new(tmp.path()).with_backend(fs);
    let mut ctx = Ctx::new(
        workspace,
        None,
        Runtime::new().unwrap(),
        Globals::default(),
        config,
        None,
        printer,
    );

    // Source A has three turns, source B only one.
    let id_a = ConversationId::try_from(ctx.now()).unwrap();
    ctx.workspace.create_conversation_with_id(
        id_a,
        Conversation::default().with_last_activated_at(ctx.now()),
        ctx.config(),
    );
    let h_a = ctx.workspace.acquire_conversation(&id_a).unwrap();
    let lock_a = ctx.workspace.test_lock(h_a);
    lock_a.as_mut().update_events(|e| e.extend(turns(3)));
    drop(lock_a);

    ctx.set_now(ctx.now() + Duration::from_secs(1));

    let id_b = ConversationId::try_from(ctx.now()).unwrap();
    ctx.workspace.create_conversation_with_id(
        id_b,
        Conversation::default().with_last_activated_at(ctx.now()),
        ctx.config(),
    );
    let h_b = ctx.workspace.acquire_conversation(&id_b).unwrap();
    let lock_b = ctx.workspace.test_lock(h_b);
    lock_b.as_mut().update_events(|e| e.extend(turns(1)));
    drop(lock_b);

    ctx.set_now(ctx.now() + Duration::from_secs(1));

    // Turn 3 exists in A but not in B.
    let fork = Fork {
        target: PositionalIds::default(),
        activate: false,
        range: selection(&["--turn", "3"]),
        compact: CompactFlag::default(),
        no_turns: false,
        title: None,
    };
    let handle_a = ctx.workspace.acquire_conversation(&id_a).unwrap();
    let handle_b = ctx.workspace.acquire_conversation(&id_b).unwrap();
    let result = tokio::runtime::Runtime::new()
        .unwrap()
        .block_on(fork.run(&mut ctx, &[handle_a, handle_b]));

    assert!(result.is_err(), "turn 3 is out of range for source B");
    assert_eq!(
        ctx.workspace.conversations().count(),
        2,
        "a rejected multi-source fork must not leave a partial fork behind"
    );
}

/// Regression test: forking a `--local` conversation must keep the fork
/// local-only instead of projecting it into the workspace.
#[test]
fn fork_inherits_local_only_projection() {
    let tmp = tempdir().unwrap();
    let (printer, _, _) = Printer::memory(OutputFormat::TextPretty);
    let config = AppConfig::new_test();
    let storage = tmp.path().join(".jp");
    let user = tmp.path().join("user");
    let fs = Arc::new(
        FsStorageBackend::new(&storage)
            .unwrap()
            .with_user_storage(&user, None, "abc")
            .unwrap(),
    );
    let workspace = Workspace::new(tmp.path()).with_backend(fs);
    let mut ctx = Ctx::new(
        workspace,
        None,
        Runtime::new().unwrap(),
        Globals::default(),
        config,
        None,
        printer,
    );

    let id = ConversationId::try_from(ctx.now()).unwrap();
    ctx.workspace.create_conversation_with_projection(
        id,
        Conversation::default().with_last_activated_at(ctx.now()),
        ctx.config(),
        Projection::LocalOnly,
    );

    let source = ctx.workspace.acquire_conversation(&id).unwrap();
    let lock = fork_conversation(&mut ctx, &source, |_| {}).unwrap();

    assert_eq!(
        lock.projection(),
        Projection::LocalOnly,
        "a fork of a local-only conversation stays local-only"
    );
}
