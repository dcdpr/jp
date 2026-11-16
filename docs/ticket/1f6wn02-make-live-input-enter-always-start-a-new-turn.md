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

RFD 132's live input field maps Enter to a different transition in each phase,
each reused from an existing interrupt action:

| Phase          | Enter                                                                                                                        |
| -------------- | ---------------------------------------------------------------------------------------------------------------------------- |
| Idle           | Starts a new turn.                                                                                                           |
| LLM streaming  | Stops the response and replies within the current turn (`InterruptAction::Reply`).                                           |
| Tool execution | Cancels the running tools and sends the text as their response (`ToolInterruptResult::Cancelled { response, exit: false }`). |

The same key in the same field does three things, depending on timing the user
does not control.
The direction: Enter always starts a new turn.
Mid-turn that is Stop followed by sending the text as the next turn's request,
which makes it the immediate counterpart of a queued reply (RFD 131): one stops
now, the other lets the turn finish, and both start the next turn.

The turn boundary is visible, so this needs a design before it changes:

- **Streaming.** The partial response is committed as Stop commits it, and the
  text opens a new turn instead of continuing the current one.
  The provider sees much the same thread, but turn-scoped behavior moves: turn
  summaries ([RFD 126]), the config epoch (RFD 131: a new turn resolves its
  config again), `--fork N`, and anything else that counts or edits turns.
- **Tool execution.** The tools record their cancellation responses, and the
  text becomes a user request rather than the tools' result.
  Decide whether anything is lost when the assistant no longer reads the text as
  the tools' answer.
- **`query.loop = "never"`.** Mid-turn Enter would start a second turn in a run
  configured to end after one.
  Decide whether an explicit Enter overrides the loop setting.
- **The interrupt menus.** RFD 132 presents mid-stream Enter as `Ctrl+C`, `r` in
  one gesture.
  Either the menus' Reply and "Stop & respond" change the same way ([RFD 092]
  owns the menus), or the live input stops matching them.
- **Partial reasoning.** The replay refusals in [#829] apply to the new turn's
  thread as they do to today's reply.

Until this lands, RFD 132 keeps the three transitions.

## Comments

-----

- **From**: jp
- **Date**: 2026-11-07T12:57:32Z

Suggestion: make what Enter does a setting rather than replacing today's
behavior.

| Value        | Enter in the live input field                                                                                       |
| ------------ | ------------------------------------------------------------------------------------------------------------------- |
| `contextual` | Today's behavior: a new turn when idle, Reply within the turn while streaming, the tools' response while tools run. |
| `new_turn`   | Always a new turn: stop whatever is running, then send the text as the next turn's query.                           |

Keeping `contextual` as the default means `new_turn` can ship opt-in, so the
questions above need answers only for the users who choose it, and the default's
behavior stays matched to the interrupt menus.

Open points:

- **Where the key lives.** A sibling key such as `query.live_input_enter`, or
  `query.live_input` becoming a table (`enabled`, `enter`) in RFD 132 before it
  is accepted, while that is still free.
- **The interrupt menus.** Whether the setting also governs the menus' Reply and
  "Stop & respond" ([RFD 092] owns them), or stays specific to the field.
- **`query.loop = "never"`** still needs a rule under `new_turn`.

[#829]: https://github.com/dcdpr/jp/issues/829
[RFD 092]: ../rfd/092-predictable-and-responsive-interrupt-escalation.md
[RFD 092]: ../rfd/092-predictable-and-responsive-interrupt-escalation.md
[RFD 126]: ../rfd/126-turn-summaries.md
