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

Implements [RFD 092] Phase 8, independent of the rest of [RFD 092].

`[a] Abort (discard & exit)` promises to discard the turn, but the turn loop
persists each event to disk as it commits, so Abort can discard only the
in-flight streaming block that the provider has not closed yet.
`[s] Stop` already covers stopping a turn cleanly, while removing or replacing a
committed response is a history edit (`jp query --replay` or `jp conversation
edit`).

Because conversations retain their config deltas permanently and command plugins
can send `abort` over the plugin protocol, `abort` remains accepted as a hidden
alias for `stop`:

- `interrupt.streaming.action = "abort"`: hidden via `#[variant(hidden)]`
  (matching `ProviderId::Test`), resolved to `stop` without a warning, and never
  written by JP.
- `InterruptAction::Abort` in the plugin protocol (`jp_plugin::message`):
  accepted from plugins and handled as `stop`.

## Checklist

- [ ] Remove `[a] Abort (discard & exit)` from the streaming interrupt menu
      (`interrupt/handler.rs`).
- [ ] Hide `StreamingInterruptAction::Abort`, resolve it to `stop`, and remove
      it from the `action` doc comment (`jp_config::interrupt`).
- [ ] Map `WireAction::Abort` to `InterruptAction::Stop` in `requested_action`
      (`cmd/plugin/dispatch.rs`), and update the doc comments in
      `jp_plugin::message` so they no longer describe `Abort` as discarding the
      turn.
- [ ] Remove `InterruptAction::Abort`, `TurnPhase::Aborted`, and
      `StreamingInterruptResult::Abort`, along with their match arms in
      `turn/coordinator.rs`, `interrupt/signals.rs`, `interrupt/channel.rs`, and
      `turn_loop.rs`.
- [ ] Add tests verifying that a recorded `action = "abort"` config delta loads
      and behaves as `stop` and that a plugin `abort` request stops the turn;
      update existing Abort test cases in `handler_tests.rs` and
      `interrupt_tests.rs`.
- [ ] Update the query stream pipeline wiki page (W-00dxyb6), which lists `[a]
      Abort` in its menus and the `Aborted` phase in its state table.

[RFD 092]: ../rfd/092-predictable-and-responsive-interrupt-escalation.md
