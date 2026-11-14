# Shutdown waits for an open prompt to be answered

- **Status**: Done
- **Authors**: jp
- **Date**: 2026-10-01
- **Label**: client=cli
- **Label**: package=jp_cli
- **Label**: package=jp_inquire
- **Label**: type=follow-up

A prompt runs on a blocking thread and reads the terminal until the user
answers.
Nothing outside the prompt can stop it: `jp_inquire`'s backends
(`inline_select`, `inline_reply`, `text`, `select`, `password`) take no
cancellation.

When a turn ends while a prompt is open, the prompt keeps running.
The main case is SIGTERM while a tool's approval prompt waits.
`lib.rs` races the command against the shutdown token and drops the command
future, but the blocking task keeps the prompt's writer and terminal reader.
JP builds its runtime in `build_runtime` and drops it without `shutdown_timeout`
or `shutdown_background`, and a dropped runtime waits for its blocking tasks.
So JP does not exit until somebody answers the abandoned prompt.

Question and result-review prompts have run this way since before PR #1175.
That PR moved approval prompts onto a blocking thread too.
Before, the approval prompt blocked the thread `rt.block_on` polls, so shutdown
did not even see SIGTERM until the prompt was answered.

The damage is contained and visible: the prompt is on screen, and one key or
Ctrl-C ends it.
A closed terminal fails the read, and the prompt exits on its own.

## Proposal

1. Give the prompt backends a stop handle that wakes their input wait and
   returns `InquireError::OperationCanceled`, for example by polling crossterm
   events with a timeout and checking a token.
2. Pass the turn's cancellation, or the shutdown token, to every prompt
   `ToolPrompter` opens, so `ToolCoordinator::abandon` and shutdown both close
   the prompt they leave behind.
3. Regression test: hold an approval prompt open, request shutdown, and check
   that the prompt's worker exits without an answer.

Raised in review of PR #1175 (comment 4155441004).

## Comments

-----

- **From**: jp
- **Date**: 2026-11-13T11:34:19Z

Fixed by a different route than the proposal above.

The proposal needs a stop handle inside the widgets.
`inquire` blocks in crossterm's `event::read()` with no abort, so that takes a
change in the `inquire` fork (RFD 092 defers it until the fork is inlined), and
the main case here, the approval prompt, is an `inquire` widget.

What changed instead:

- Tool prompts (approval, question, result review) run on threads of their own
  (`spawn_prompt` in `tool/coordinator.rs`), not on the runtime's blocking pool.
  A runtime waits for its blocking tasks before it shuts down, which was the
  hang; it does not wait for a plain thread.
- A prompt left open this way still has the terminal in raw mode.
  `jp_inquire::release_terminal` leaves raw mode, undoes bracketed paste and the
  keyboard enhancement flags a reply set, resets styling, shows the cursor, and
  moves below the prompt.
  `Ctx` calls it on drop when `jp_inquire::terminal_held()`.

Regression tests: `a_runtime_shuts_down_while_an_approval_prompt_is_open` in
`coordinator_tests.rs` holds an approval prompt open and shuts the runtime down
without answering it, and `crates/jp_inquire/tests/abandoned_prompt_pty.rs`
exits a process with a select and a reply open in a real pty.

Not covered:

- Closing a prompt while the run goes on.
  The abandoned prompt is only left behind at exit, so T-17qrhwx still needs the
  stop handle.
- A prompt run inline on the thread that polls the command (an interrupt menu, a
  confirmation, a plugin's `compose`).
  Nothing polls the shutdown token while it blocks, so SIGTERM is still not
  acted on until it is answered.
- An immediate exit (SIGQUIT, or a signal after shutdown began) while a prompt
  is open goes through `std::process::exit` and leaves the terminal in raw mode.
  Raw mode turns Ctrl-C and Ctrl-\ into keys the widget reads, so this takes a
  signal sent with `kill`.

-----

- **From**: jp
- **Date**: 2026-11-13T12:21:56Z
- **Re**: #1

The three items listed as not covered above have moved since:

- **Closing a prompt while the run goes on**: the tool coordinator closes a
  prompt once its call is settled, and `InlineReply` acts on that.
  A prompt `inquire` draws does not, the approval prompt included (T-17qrhwx,
  T-1jbt5wm).
- **A prompt on the thread that polls the command**: a shutdown request closes
  every `InlineReply` (`jp_inquire::close_all`, wired in `run_inner`), and a
  plugin's `compose` prompt runs on a thread of its own.
  An `inquire` prompt opened inline still holds a shutdown: the interrupt menus,
  the confirmations, the conversation and title pickers, the lock-wait select,
  and the editor re-open confirmation (T-1jbt5wm).
- **An immediate exit while a prompt is open**: the signal router restores the
  terminal before `std::process::exit`.
