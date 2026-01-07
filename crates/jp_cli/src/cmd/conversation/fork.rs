use std::{str::FromStr as _, time::Duration};

use jp_conversation::ConversationId;
use time::UtcDateTime;

use crate::{Output, cmd::Success, ctx::Ctx};

#[derive(Debug, clap::Args)]
pub(crate) struct Fork {
    /// Conversation ID to fork.
    ///
    /// Defaults to the active conversation if not specified.
    id: Option<ConversationId>,

    #[arg(short, long, default_value = "false")]
    activate: bool,

    /// Ignore all conversation events *before* the specified timestamp.
    ///
    /// Timestamp can be relative (5days, 2mins, etc) or absolute. Can be used
    /// in combination with `--until`.
    #[arg(long, value_parser = parse_duration)]
    from: Option<UtcDateTime>,

    /// Ignore all conversation events *after* the specified timestamp.
    ///
    /// Timestamp can be relative (5days, 2mins, etc) or absolute. Can be used
    /// in combination with `--until`.
    #[arg(long, value_parser = parse_duration)]
    until: Option<UtcDateTime>,
}

fn parse_duration(s: &str) -> Result<UtcDateTime, String> {
    humantime::Duration::from_str(s)
        .map(|d| UtcDateTime::now() - Duration::from(d))
        .map_err(|e| e.to_string())
        .or_else(|_| {
            humantime::parse_rfc3339_weak(s)
                .map(Into::into)
                .map_err(|e| e.to_string())
        })
}

impl Fork {
    pub(crate) fn run(self, ctx: &mut Ctx) -> Output {
        let now = ctx.now();

        let original_id = self
            .id
            .unwrap_or_else(|| ctx.workspace.active_conversation_id());

        let mut new_conversation = ctx.workspace.try_get_conversation(&original_id)?.clone();
        new_conversation.last_activated_at = now;
        new_conversation.expires_at = None;

        let mut new_events = ctx
            .workspace
            .try_get_events(&original_id)?
            .clone()
            .with_created_at(now);

        new_events.retain(|event| {
            self.from.is_none_or(|from| event.timestamp >= from)
                && self.until.is_none_or(|until| event.timestamp <= until)
        });

        let new_id = ConversationId::try_from(ctx.now())?;
        ctx.workspace.create_conversation_with_id(
            new_id,
            new_conversation,
            new_events.base_config(),
        );

        ctx.workspace
            .try_get_events_mut(&new_id)?
            .extend(new_events);

        // TODO:
        // 2. Then fork the current active conversation
        // 3. Then ask LLM to implement `Printer` struct

        if self.activate {
            ctx.workspace.set_active_conversation_id(new_id, now)?;
        }

        Ok(Success::Message("Conversation forked.".into()))
    }
}

#[cfg(test)]
mod tests {
    use std::panic::{AssertUnwindSafe, catch_unwind, resume_unwind};

    use assert_matches::assert_matches;
    use jp_config::{
        AppConfig, Config as _, PartialAppConfig,
        conversation::tool::RunMode,
        model::id::{PartialModelIdConfig, ProviderId},
    };
    use jp_conversation::{
        Conversation, ConversationEvent, ConversationStream,
        event::{ChatRequest, ChatResponse},
    };
    use jp_workspace::Workspace;
    use tempfile::tempdir;
    use time::macros::utc_datetime;
    use tokio::runtime::Runtime;

    use super::*;
    use crate::Globals;

    #[test]
    #[expect(clippy::too_many_lines)]
    fn test_conversation_fork() {
        struct TestCase {
            args: Fork,
            setup: fn(&mut Ctx),
            assert: fn(Vec<(ConversationId, Conversation, ConversationStream)>, ConversationId),
        }

        jp_id::global::set("foo".to_owned());

        let cases = vec![
            ("no conversation", TestCase {
                args: Fork {
                    id: None,
                    activate: false,
                    from: None,
                    until: None,
                },
                setup: |ctx| {
                    let id = ConversationId::try_from(ctx.now()).unwrap();
                    ctx.workspace.create_conversation_with_id(
                        id,
                        Conversation::default().with_last_activated_at(ctx.now()),
                        ctx.config(),
                    );
                    ctx.workspace
                        .set_active_conversation_id(id, ctx.now())
                        .unwrap();
                },

                assert: |mut convs, active_id| {
                    assert_eq!(convs.len(), 2);
                    assert_eq!(active_id, convs[0].0);

                    assert!(convs[0].1.last_activated_at < convs[1].1.last_activated_at);
                    assert!(convs[0].2.created_at < convs[1].2.created_at);

                    for (_, conv, stream) in &mut convs {
                        conv.last_activated_at = UtcDateTime::UNIX_EPOCH;
                        stream.created_at = UtcDateTime::UNIX_EPOCH;
                    }

                    assert!(convs[0].0.timestamp() < convs[1].0.timestamp());
                    assert_eq!(convs[0].1, convs[1].1);
                    assert_eq!(convs[0].2, convs[1].2);
                },
            }),
            ("conversation with events", TestCase {
                args: Fork {
                    id: None,
                    activate: false,
                    from: None,
                    until: None,
                },
                setup: |ctx| {
                    let id = ConversationId::try_from(ctx.now()).unwrap();
                    ctx.workspace.create_conversation_with_id(
                        id,
                        Conversation::default().with_last_activated_at(ctx.now()),
                        ctx.config(),
                    );

                    ctx.workspace
                        .set_active_conversation_id(id, ctx.now())
                        .unwrap();
                    ctx.workspace.get_events_mut(&id).unwrap().extend(vec![
                        ConversationEvent::new(
                            ChatRequest::from("foo"),
                            utc_datetime!(2020-01-01 0:00),
                        ),
                        ConversationEvent::new(
                            ChatResponse::message("bar"),
                            utc_datetime!(2020-01-01 0:01),
                        ),
                    ]);
                },
                assert: |mut convs, active_id| {
                    assert_eq!(convs.len(), 2);
                    assert_eq!(active_id, convs[0].0);

                    assert!(convs[0].1.last_activated_at < convs[1].1.last_activated_at);
                    assert!(convs[0].2.created_at < convs[1].2.created_at);

                    for (_, conv, stream) in &mut convs {
                        conv.last_activated_at = UtcDateTime::UNIX_EPOCH;
                        stream.created_at = UtcDateTime::UNIX_EPOCH;
                    }

                    assert!(convs[0].0.timestamp() < convs[1].0.timestamp());
                    assert_eq!(convs[0].1, convs[1].1);
                    assert_eq!(convs[0].2, convs[1].2);
                },
            }),
            ("with activate", TestCase {
                args: Fork {
                    id: None,
                    activate: true,
                    from: None,
                    until: None,
                },
                setup: |ctx| {
                    let id = ConversationId::try_from(ctx.now()).unwrap();
                    ctx.workspace.create_conversation_with_id(
                        id,
                        Conversation::default().with_last_activated_at(ctx.now()),
                        ctx.config(),
                    );
                    ctx.workspace
                        .set_active_conversation_id(id, ctx.now())
                        .unwrap();
                    ctx.workspace.get_events_mut(&id).unwrap().extend(vec![
                        ConversationEvent::new(
                            ChatRequest::from("foo"),
                            utc_datetime!(2020-01-01 0:00),
                        ),
                        ConversationEvent::new(
                            ChatResponse::message("bar"),
                            utc_datetime!(2020-01-01 0:01),
                        ),
                    ]);
                },

                assert: |mut convs, active_id| {
                    assert_eq!(convs.len(), 2);

                    // active conversation is always the first one
                    assert_eq!(active_id, convs[0].0);

                    assert!(convs[0].1.last_activated_at > convs[1].1.last_activated_at);
                    assert!(convs[0].2.created_at > convs[1].2.created_at);

                    for (_, conv, stream) in &mut convs {
                        conv.last_activated_at = UtcDateTime::UNIX_EPOCH;
                        stream.created_at = UtcDateTime::UNIX_EPOCH;
                    }

                    assert!(convs[0].0.timestamp() > convs[1].0.timestamp());
                    assert_eq!(convs[0].1, convs[1].1);
                    assert_eq!(convs[0].2, convs[1].2);
                },
            }),
            ("with from", TestCase {
                args: Fork {
                    id: None,
                    activate: false,
                    from: Some(utc_datetime!(2020-01-01 0:01)),
                    until: None,
                },
                setup: |ctx| {
                    let id = ConversationId::try_from(ctx.now()).unwrap();
                    ctx.workspace.create_conversation_with_id(
                        id,
                        Conversation::default().with_last_activated_at(ctx.now()),
                        ctx.config(),
                    );
                    ctx.workspace
                        .set_active_conversation_id(id, ctx.now())
                        .unwrap();
                    ctx.workspace.get_events_mut(&id).unwrap().extend(vec![
                        ConversationEvent::new(
                            ChatRequest::from("foo"),
                            utc_datetime!(2020-01-01 0:00),
                        ),
                        ConversationEvent::new(
                            ChatResponse::message("bar"),
                            utc_datetime!(2020-01-01 0:01),
                        ),
                        ConversationEvent::new(
                            ChatResponse::message("baz"),
                            utc_datetime!(2020-01-01 0:02),
                        ),
                    ]);
                },

                assert: |convs, _| {
                    assert_eq!(convs.len(), 2);
                    assert_eq!(convs[0].2.iter().count(), 3);
                    assert_eq!(convs[1].2.iter().count(), 2);
                    assert_eq!(
                        convs[0].2.first().unwrap().event.timestamp,
                        utc_datetime!(2020-01-01 0:00)
                    );
                    assert_eq!(
                        convs[1].2.first().unwrap().event.timestamp,
                        utc_datetime!(2020-01-01 0:01)
                    );
                },
            }),
            ("with until", TestCase {
                args: Fork {
                    id: None,
                    activate: false,
                    from: None,
                    until: Some(utc_datetime!(2020-01-01 0:01)),
                },
                setup: |ctx| {
                    let id = ConversationId::try_from(ctx.now()).unwrap();
                    ctx.workspace.create_conversation_with_id(
                        id,
                        Conversation::default().with_last_activated_at(ctx.now()),
                        ctx.config(),
                    );
                    ctx.workspace
                        .set_active_conversation_id(id, ctx.now())
                        .unwrap();
                    ctx.workspace.get_events_mut(&id).unwrap().extend(vec![
                        ConversationEvent::new(
                            ChatRequest::from("foo"),
                            utc_datetime!(2020-01-01 0:00),
                        ),
                        ConversationEvent::new(
                            ChatResponse::message("bar"),
                            utc_datetime!(2020-01-01 0:01),
                        ),
                        ConversationEvent::new(
                            ChatResponse::message("baz"),
                            utc_datetime!(2020-01-01 0:02),
                        ),
                    ]);
                },

                assert: |convs, _| {
                    assert_eq!(convs.len(), 2);
                    assert_eq!(convs[0].2.iter().count(), 3);
                    assert_eq!(convs[1].2.iter().count(), 2);
                    assert_eq!(
                        convs[0].2.last().unwrap().event.timestamp,
                        utc_datetime!(2020-01-01 0:02)
                    );
                    assert_eq!(
                        convs[1].2.last().unwrap().event.timestamp,
                        utc_datetime!(2020-01-01 0:01)
                    );
                },
            }),
            ("with from and until", TestCase {
                args: Fork {
                    id: None,
                    activate: false,
                    from: Some(utc_datetime!(2020-01-01 0:01)),
                    until: Some(utc_datetime!(2020-01-01 0:02)),
                },
                setup: |ctx| {
                    let id = ConversationId::try_from(ctx.now()).unwrap();
                    ctx.workspace.create_conversation_with_id(
                        id,
                        Conversation::default().with_last_activated_at(ctx.now()),
                        ctx.config(),
                    );
                    ctx.workspace
                        .set_active_conversation_id(id, ctx.now())
                        .unwrap();
                    ctx.workspace.get_events_mut(&id).unwrap().extend(vec![
                        ConversationEvent::new(
                            ChatRequest::from("foo"),
                            utc_datetime!(2020-01-01 0:00),
                        ),
                        ConversationEvent::new(
                            ChatResponse::message("bar"),
                            utc_datetime!(2020-01-01 0:01),
                        ),
                        ConversationEvent::new(
                            ChatResponse::message("baz"),
                            utc_datetime!(2020-01-01 0:02),
                        ),
                        ConversationEvent::new(
                            ChatResponse::message("qux"),
                            utc_datetime!(2020-01-01 0:03),
                        ),
                    ]);
                },

                assert: |convs, _| {
                    assert_eq!(convs.len(), 2);
                    assert_eq!(convs[0].2.iter().count(), 4);
                    assert_eq!(convs[1].2.iter().count(), 2);
                    assert_eq!(
                        convs[0].2.first().unwrap().event.timestamp,
                        utc_datetime!(2020-01-01 0:00)
                    );
                    assert_eq!(
                        convs[1].2.first().unwrap().event.timestamp,
                        utc_datetime!(2020-01-01 0:01)
                    );
                    assert_eq!(
                        convs[0].2.last().unwrap().event.timestamp,
                        utc_datetime!(2020-01-01 0:03)
                    );
                    assert_eq!(
                        convs[1].2.last().unwrap().event.timestamp,
                        utc_datetime!(2020-01-01 0:02)
                    );
                },
            }),
        ];

        for (name, case) in cases {
            let tmp = tempdir().unwrap();

            let mut partial = PartialAppConfig::empty();
            partial.conversation.tools.defaults.run = Some(RunMode::Ask);
            partial.assistant.model.id = PartialModelIdConfig {
                provider: Some(ProviderId::Anthropic),
                name: Some("test".parse().unwrap()),
            }
            .into();

            let config = AppConfig::from_partial(partial).unwrap();
            let workspace = Workspace::new(tmp.path());
            let mut ctx = Ctx::new(
                workspace,
                Runtime::new().unwrap(),
                Globals::default(),
                config,
            );

            if let Err(panic) = catch_unwind(AssertUnwindSafe(|| {
                (case.setup)(&mut ctx);
            })) {
                eprintln!("Test case '{name}' panicked.");
                resume_unwind(panic);
            }

            ctx.set_now(UtcDateTime::UNIX_EPOCH + Duration::from_secs(1));

            let msg =
                assert_matches!(case.args.run(&mut ctx).unwrap(), Success::Message(msg) => msg);
            assert_eq!(&msg, "Conversation forked.", "failed test case: '{name}'");

            let conversations = ctx
                .workspace
                .conversations()
                .map(|(id, conv)| (*id, conv.clone()))
                .collect::<Vec<_>>();

            let conversations = conversations
                .into_iter()
                .map(|(id, conv)| (id, conv, ctx.workspace.get_events(&id).unwrap().clone()))
                .collect();

            let active_id = ctx.workspace.active_conversation_id();

            if let Err(panic) = catch_unwind(AssertUnwindSafe(|| {
                (case.assert)(conversations, active_id);
            })) {
                eprintln!("Test case '{name}' panicked.");
                resume_unwind(panic);
            }
        }
    }
}
