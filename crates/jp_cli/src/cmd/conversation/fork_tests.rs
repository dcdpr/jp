use std::panic::{AssertUnwindSafe, catch_unwind, resume_unwind};

use camino_tempfile::tempdir;
use chrono::TimeZone as _;
use jp_config::AppConfig;
use jp_conversation::{
    Conversation, ConversationEvent, ConversationStream,
    event::{ChatRequest, ChatResponse, TurnStart},
};
use jp_printer::{OutputFormat, Printer};
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
        ("with from and until", TestCase {
            args: Fork {
                target: PositionalIds::default(),
                activate: false,
                from: Some(Utc.with_ymd_and_hms(2020, 1, 1, 0, 1, 0).unwrap()),
                until: Some(Utc.with_ymd_and_hms(2020, 1, 1, 0, 2, 0).unwrap()),
                last: None,
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
        let workspace = Workspace::new(tmp.path())
            .persisted_at(&storage)
            .unwrap()
            .with_local_storage_at(&user, "test", "abc")
            .unwrap();
        let mut ctx = Ctx::new(
            workspace,
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
