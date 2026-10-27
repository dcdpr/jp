# Let the tool coordinator close a prompt it no longer needs

- **Status**: Todo
- **Kind**: Feature
- **Authors**: jp
- **Date**: 2026-10-24
- **Label**: client=cli
- **Label**: package=jp_cli
- **Label**: package=jp_inquire
- **Label**: type=follow-up

A tool call can be settled while its approval, question, or result-review prompt
is still on screen.
Two paths reach that state:

- The Host loses its connection to the JP MCP Server while another call's prompt
  is open.
  Every unsettled call is recorded as lost at once.
- A client driving the turn from outside the process stops or restarts the calls
  while a local prompt is open.
  `ToolCoordinator::finish` applies client interrupts even while a prompt is
  active, and `abandon_parked` settles calls parked in `Wait::Approval` or
  `Wait::Review`.

`finish` waits for an open prompt before it hands the calls back, and an answer
given after its call was settled is ignored (PR #1247).
Nothing is lost, but the user still has to answer a prompt for a call that is
already decided before the turn can end.

The prompts run in `spawn_blocking` through `ToolPrompter` and `PromptBackend`
(`inline_select`, `inline_reply`, `select`, `password`), and none of them can be
stopped from outside.
Closing a prompt the coordinator no longer needs would take:

- A cancellation token on each `PromptBackend` method, threaded through
  `ToolPrompter` and every test backend.
- Widgets that poll for input and check the token.
  `InlineSelect` blocks inside inquire's `CustomType::prompt_with_writer`, which
  appears to have no external abort; confirm that against inquire's source
  before choosing between replacing the widget and wrapping it.
  `inline_reply` is our own widget.
- Terminal cleanup when a prompt is stopped part-way: leave raw mode and erase
  the partly drawn prompt.
- A decision for the `$EDITOR` path behind result editing, which can only be
  stopped by ending the editor process.

The wait in `finish` stays either way: a cancelled prompt still reports back
from its blocking task, and the coordinator has to receive that before the batch
goes away.

Done when a call settled while its prompt is open closes that prompt without
user input, the terminal is left clean, and a test shows `finish` returning
without an answer being sent.
