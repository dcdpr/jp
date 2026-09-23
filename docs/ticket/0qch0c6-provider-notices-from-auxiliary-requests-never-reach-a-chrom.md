# Provider notices from auxiliary requests never reach a chrome sink

- **Status**: Todo
- **Kind**: Bug
- **Authors**: jp
- **Date**: 2026-09-23
- **Implements**: 090
- **Label**: domain=llm
- **Label**: package=jp_cli
- **Label**: package=jp_llm
- **Label**: package=jp_task
- **Label**: type=bug

`Event::Notice` reaches the user on the query path only.
`TurnCoordinator` prints it to stderr
(`jp_cli/src/cmd/query/turn/coordinator.rs`), but the three callers of
`collect_with_retry` do not:

| Caller                                 | What happens to a notice                    |
| -------------------------------------- | ------------------------------------------- |
| `jp_llm::title::generate`              | dropped by `event_builder::structured_data` |
| `jp_cli::cmd::query::tool::inquiry`    | dropped, never matched                      |
| `jp_cli::cmd::conversation::summarize` | `tracing::warn!`, invisible without `--log` |

RFD 090 makes the notice the audit trail for a money decision: *"switching from
fixed-cost to per-token billing is a money decision, and the notice is the audit
trail."* A title generation that falls through to `api_key` keeps that promise
only for the turn that triggered it.

`collect_with_retry` now returns notices at the front of its event list, and
keeps the ones from an attempt that later failed, so the remaining work is
delivery rather than plumbing inside `jp_llm`.

Delivery is the awkward part: none of the three callers holds a `Printer`, and
title generation runs in a `jp_task` background task that has no terminal at
all.
A sink has to be threaded in, or notices routed through the task handler's
existing reporting.

Contained for now: the cooldown that prompts a switch is persisted, so the next
main-path turn surfaces `skipping profile:… cooling down`.
The user learns one turn late rather than never.
