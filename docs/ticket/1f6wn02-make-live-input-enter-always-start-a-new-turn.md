# Make live-input Enter always start a new turn

- **Status**: Todo
- **Authors**: jp
- **Date**: 2026-11-07
- **Label**: client=cli
- **Label**: domain=conversation
- **Label**: package=jp_cli
- **Label**: relevant-rfd=092
- **Label**: relevant-rfd=132
- **Label**: relevant-rfd=131
- **Label**: type=pre-rfd

In [RFD 132], pressing `Enter` in the live input field triggers a different
transition in each phase, reusing the existing interrupt actions:

| Phase          | Enter                                                                                                                        |
| -------------- | ---------------------------------------------------------------------------------------------------------------------------- |
| Idle           | Starts a new turn.                                                                                                           |
| LLM streaming  | Stops the response and replies within the current turn (`InterruptAction::Reply`).                                           |
| Tool execution | Cancels the running tools and sends the text as their response (`ToolInterruptResult::Cancelled { response, exit: false }`). |

Pressing the same key in the same field does three different things depending on
phase timing that the user does not control.
Instead, `Enter` should always start a new turn.
Mid-turn, pressing `Enter` would stop the current turn and send the buffer as
the next turn's request, making it the immediate counterpart of a queued reply
([RFD 131]): one stops the current turn immediately, the other lets it finish,
and both start the next turn.

Because the turn boundary is user-visible, changing this requires a design
decision on several points:

- **LLM streaming.** Partial output is committed just as Stop commits it, and
  the submitted text opens a new turn instead of continuing the current one.
  The provider sees nearly the same thread, but turn-scoped behavior changes:
  turn summaries ([RFD 126]), per-turn config resolution ([RFD 131]), `--fork
  N`, and any other operation that counts or edits turns.
- **Tool execution.** Running tools record their cancellation responses, and the
  submitted text becomes a new user `ChatRequest` rather than the tools' result
  payload.
  Determine whether the assistant loses useful context when the text is no
  longer attached to the cancelled tool calls.
- **`query.loop = "never"`.** Pressing `Enter` mid-turn would start a second
  turn in a run configured to exit after one turn.
  Determine whether an explicit mid-turn `Enter` overrides `query.loop =
  "never"`.
- **Interrupt menus.** [RFD 132] models mid-stream `Enter` as the single-step
  equivalent of `Ctrl+C` followed by `[r] Reply`.
  Decide whether `[r] Reply` and `[r] Stop & respond` in the interrupt menus
  ([RFD 092]) also change to start a new turn, or whether live input diverges
  from the menu actions.
- **Partial reasoning.** The partial-reasoning replay refusals in [#829] apply
  to the new turn's thread just as they apply to mid-turn replies today.

Until these questions are resolved, [RFD 132] retains the per-phase transitions
above.

## Comments

-----

- **From**: jp
- **Date**: 2026-11-07T12:57:32Z

Suggestion: make `Enter` behavior configurable rather than replacing the
per-phase behavior outright:

| Value        | Enter in the live input field                                                                                        |
| ------------ | -------------------------------------------------------------------------------------------------------------------- |
| `contextual` | Per-phase behavior: start a new turn when idle, reply within the turn while streaming, or respond to cancelled tools |
| `new_turn`   | Always start a new turn: stop the active turn, then send the buffer as the next turn's query.                        |

Keeping `contextual` as the default allows `new_turn` to ship as an opt-in
setting, scoping the open questions above to users who enable `new_turn` while
keeping the default aligned with the interrupt menus.

Open questions for this setting:

- **Config key placement.** Add a sibling key such as `query.live_input_enter`,
  or make `query.live_input` a table (`enabled`, `enter`) in [RFD 132] before it
  is accepted.
- **Interrupt menus.** Decide whether the setting also governs `[r] Reply` and
  `[r] Stop & respond` in the interrupt menus ([RFD 092]) or applies only to the
  live input field.
- **`query.loop = "never"`.** Define how `new_turn` interacts with `query.loop =
  "never"`.

[#829]: https://github.com/dcdpr/jp/issues/829
[RFD 092]: ../rfd/092-predictable-and-responsive-interrupt-escalation.md
[RFD 126]: ../rfd/126-turn-summaries.md
[RFD 131]: ../rfd/131-queued-replies.md
[RFD 132]: ../rfd/132-repl-mode-with-live-streaming-input.md
