# Remove the Abort interrupt action

- **Status**: Todo
- **Authors**: jp
- **Date**: 2026-11-08
- **Label**: client=cli
- **Label**: package=jp_cli
- **Label**: package=jp_config
- **Label**: package=jp_plugin
- **Label**: relevant-rfd=092
- **Label**: type=task

[RFD 092] Phase 8.
Independent of the rest of 092.

Abort promises "discard & exit", but the turn loop writes each event to disk as
it commits, so all it can still discard is the block the provider has not closed
yet.
Stop does everything Abort usefully did; removing or replacing a response is a
history edit (`jp query --replay`, `jp conversation edit`).

`abort` stays readable wherever it was ever written, because conversations keep
their config deltas forever and plugins already send it:

- `interrupt.streaming.action = "abort"`: hidden (`#[variant(hidden)]`, as
  `ProviderId::Test`), resolved as `stop`, no warning, never written by JP.
- The plugin protocol's `InterruptAction::Abort` (`jp_plugin::message`):
  accepted from plugins and handled as `stop`.

## Checklist

- [ ] Drop `[a] Abort (discard & exit)` from the streaming menu
      (`interrupt/handler.rs`).
- [ ] Hide `StreamingInterruptAction::Abort` and resolve it to `stop`; drop it
      from the `action` doc comment (`jp_config::interrupt`).
- [ ] Map the plugin protocol's `WireAction::Abort` to `InterruptAction::Stop`
      in `requested_action` (`cmd/plugin/dispatch.rs`), and rewrite the doc
      comments in `jp_plugin::message` that describe it as a discard.
- [ ] Remove `InterruptAction::Abort`, `TurnPhase::Aborted`, and
      `StreamingInterruptResult::Abort`, with their arms in
      `turn/coordinator.rs`, `interrupt/signals.rs`, `interrupt/channel.rs`, and
      `turn_loop.rs`.
- [ ] Tests: a recorded `action = "abort"` still loads and behaves as `stop`; a
      plugin `abort` request stops the turn; update the existing Abort cases in
      `handler_tests.rs` and `interrupt_tests.rs`.
- [ ] Update the query stream pipeline page (W-00dxyb6), which shows `[a] Abort`
      in its menus and the `Aborted` phase in its state table.

[RFD 092]: ../rfd/092-predictable-and-responsive-interrupt-escalation.md
