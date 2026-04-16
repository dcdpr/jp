use std::{
    panic::{AssertUnwindSafe, catch_unwind, resume_unwind},
    sync::Arc,
};

use camino_tempfile::tempdir;
use chrono::TimeZone as _;
use jp_config::AppConfig;
use jp_conversation::{
    Conversation, ConversationEvent, ConversationId, ConversationStream,
    event::{ChatRequest, ChatResponse, TurnStart},
};
use jp_printer::{OutputFormat, Printer};
use jp_storage::backend::FsStorageBackend;
use jp_workspace::Workspace;
use tokio::runtime::Runtime;

use super::*;
use crate::{Globals, cmd::conversation_id::PositionalIds};

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
                from: None,
                until: None,
                last: None,
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
                from: None,
                until: None,
                last: None,
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
        ("with activate", TestCase {
            args: Fork {
                target: PositionalIds::default(),
                activate: true,
                from: None,
                until: None,
                last: None,
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
                from: Some(Utc.with_ymd_and_hms(2020, 1, 1, 0, 1, 0).unwrap()),
                until: None,
                last: None,
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
                            ChatRequest::from("foo"),
                            Utc.with_ymd_and_hms(2020, 1, 1, 0, 0, 0).unwrap(),
                        ),
                        ConversationEvent::new(
                            ChatResponse::message("bar"),
                            Utc.with_ymd_and_hms(2020, 1, 1, 0, 1, 0).unwrap(),
                        ),
                        ConversationEvent::new(
                            ChatResponse::message("baz"),
                            Utc.with_ymd_and_hms(2020, 1, 1, 0, 2, 0).unwrap(),
                        ),
                    ]);
                });
                id
            },

            assert: |convs, _| {
                assert_eq!(convs.len(), 2);
                assert_eq!(convs[0].2.iter().count(), 3);
                // +1 for injected TurnStart
                assert_eq!(convs[1].2.iter().count(), 3);
                assert_eq!(
                    convs[0].2.first().unwrap().event.timestamp,
                    Utc.with_ymd_and_hms(2020, 1, 1, 0, 0, 0).unwrap()
                );
                assert_eq!(
                    convs[1].2.first().unwrap().event.timestamp,
                    Utc.with_ymd_and_hms(2020, 1, 1, 0, 1, 0).unwrap()
                );
            },
        }),
        ("with until", TestCase {
            args: Fork {
                target: PositionalIds::default(),
                activate: false,
                from: None,
                until: Some(Utc.with_ymd_and_hms(2020, 1, 1, 0, 1, 0).unwrap()),
                last: None,
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
                            ChatRequest::from("foo"),
                            Utc.with_ymd_and_hms(2020, 1, 1, 0, 0, 0).unwrap(),
                        ),
                        ConversationEvent::new(
                            ChatResponse::message("bar"),
                            Utc.with_ymd_and_hms(2020, 1, 1, 0, 1, 0).unwrap(),
                        ),
                        ConversationEvent::new(
                            ChatResponse::message("baz"),
                            Utc.with_ymd_and_hms(2020, 1, 1, 0, 2, 0).unwrap(),
                        ),
                    ]);
                });
                id
            },

            assert: |convs, _| {
                assert_eq!(convs.len(), 2);
                assert_eq!(convs[0].2.iter().count(), 3);
                // +1 for injected TurnStart
                assert_eq!(convs[1].2.iter().count(), 3);
                assert_eq!(
                    convs[0].2.last().unwrap().event.timestamp,
                    Utc.with_ymd_and_hms(2020, 1, 1, 0, 2, 0).unwrap()
                );
                assert_eq!(
                    convs[1].2.last().unwrap().event.timestamp,
                    Utc.with_ymd_and_hms(2020, 1, 1, 0, 1, 0).unwrap()
                );
            },
        }),
        ("with last (default 1)", TestCase {
            args: Fork {
                target: PositionalIds::default(),
                activate: false,
                from: None,
                until: None,
                last: Some(None),
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
                from: None,
                until: None,
                last: Some(Some(2)),
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
                from: None,
                until: None,
                last: Some(Some(10)),
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
                // all events kept since --last 10 > 1 turn
                assert_eq!(convs[0].2.iter().count(), 3);
                assert_eq!(convs[1].2.iter().count(), 3);
            },
        }),
        ("with last and no turn markers", TestCase {
            args: Fork {
                target: PositionalIds::default(),
                activate: false,
                from: None,
                until: None,
                last: Some(Some(1)),
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
        ("with custom title", TestCase {
            args: Fork {
                target: PositionalIds::default(),
                activate: false,
                from: None,
                until: None,
                last: None,
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
        ("with from and until", TestCase {
            args: Fork {
                target: PositionalIds::default(),
                activate: false,
                from: Some(Utc.with_ymd_and_hms(2020, 1, 1, 0, 1, 0).unwrap()),
                until: Some(Utc.with_ymd_and_hms(2020, 1, 1, 0, 2, 0).unwrap()),
                last: None,
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
                            ChatRequest::from("foo"),
                            Utc.with_ymd_and_hms(2020, 1, 1, 0, 0, 0).unwrap(),
                        ),
                        ConversationEvent::new(
                            ChatResponse::message("bar"),
                            Utc.with_ymd_and_hms(2020, 1, 1, 0, 1, 0).unwrap(),
                        ),
                        ConversationEvent::new(
                            ChatResponse::message("baz"),
                            Utc.with_ymd_and_hms(2020, 1, 1, 0, 2, 0).unwrap(),
                        ),
                        ConversationEvent::new(
                            ChatResponse::message("qux"),
                            Utc.with_ymd_and_hms(2020, 1, 1, 0, 3, 0).unwrap(),
                        ),
                    ]);
                });
                id
            },

            assert: |convs, _| {
                assert_eq!(convs.len(), 2);
                assert_eq!(convs[0].2.iter().count(), 4);
                // +1 for injected TurnStart
                assert_eq!(convs[1].2.iter().count(), 3);
                assert_eq!(
                    convs[0].2.first().unwrap().event.timestamp,
                    Utc.with_ymd_and_hms(2020, 1, 1, 0, 0, 0).unwrap()
                );
                assert_eq!(
                    convs[1].2.first().unwrap().event.timestamp,
                    Utc.with_ymd_and_hms(2020, 1, 1, 0, 1, 0).unwrap()
                );
                assert_eq!(
                    convs[0].2.last().unwrap().event.timestamp,
                    Utc.with_ymd_and_hms(2020, 1, 1, 0, 3, 0).unwrap()
                );
                assert_eq!(
                    convs[1].2.last().unwrap().event.timestamp,
                    Utc.with_ymd_and_hms(2020, 1, 1, 0, 2, 0).unwrap()
                );
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
                .with_user_storage(&user, "test", "abc")
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
        case.args.run(&mut ctx, &[source_handle]).unwrap();
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
            .with_user_storage(&user, "test", "abc")
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
        from: None,
        until: None,
        last: None,
        title: Some("forked-from-b".to_owned()),
    };
    let handle_b = ctx.workspace.acquire_conversation(&id_b).unwrap();
    fork.run(&mut ctx, &[handle_b]).unwrap();

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
