# Shutdown waits for an open prompt to be answered

- **Status**: Todo
- **Kind**: Bug
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
