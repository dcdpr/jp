# Inline the inquire fork and make its prompts closable

- **Status**: Todo
- **Authors**: jp
- **Date**: 2026-11-13
- **Label**: client=cli
- **Label**: package=jp_cli
- **Label**: package=jp_inquire
- **Label**: relevant-rfd=092
- **Label**: type=task

Bring JP's `inquire` fork into `crates/contrib/inquire`, then change its key
reader so prompts it renders can be closed programmatically without user input.

JP uses `inquire` from a personal fork: `JeanMertz/inquire`, branch `merged`,
pinned in `Cargo.lock` at `0b68068` (0.9.1), which carries the
submit-on-valid-parse change of upstream PR 321.
`jp_cli` and `jp_inquire` depend on it, both with the `crossterm` feature only.
Changing `inquire`'s input loop currently requires modifying an external
repository.
RFD 092 defers widget-level cancellation "until the fork is inlined into
`crates/contrib`".
This ticket tracks that inlining and the key-reader change that depends on it.

## The restriction

Prompts rendered by `inquire` return only when a key is pressed.
`CrosstermKeyReader::read_key` (`inquire/src/terminal/crossterm.rs`) loops on
`crossterm::event::read()`, which blocks.
The reader is private to the crate: `InputReader` and `prompt_with_backend` are
`pub(crate)`, so nothing outside `inquire` can substitute one or make it return.

`InlineReply` already supports programmatic closing: it is built on `reedline`,
whose break signal `jp_inquire` sets when `jp_inquire::close_requested()`
returns `true`.
`close_requested` covers both graceful shutdown (`jp_inquire::close_all`) and
the `PromptCloser` a prompt was opened under (used by the tool coordinator to
close the prompt of a settled tool call).
`inquire`'s key reader needs to consult the same check.

`inquire` widgets are constructed at 22 call sites in 10 files:

- `jp_inquire/src/inline_select.rs`: `CustomType`, the single-key prompt behind
  every `InlineSelect`.
  `InlineSelect` is opened at 10 places in `jp_cli`: tool approval, yes/no
  questions and result delivery (`tool/prompter.rs`), the two interrupt menus
  (`interrupt/handler.rs`), conversation confirmations (`shared/confirm.rs`),
  flag prompts (`cli/flag.rs`), label resolution (`label/resolve.rs`), and
  plugin approval and install (`plugin/admission.rs`, `plugin/install.rs`).
- `jp_inquire/src/prompt.rs`: the list select and the password prompt behind
  `PromptBackend::select` and `PromptBackend::password`.
- `jp_cli/src/cmd/plugin/dispatch.rs`: `Text`, `Select` and `MultiSelect` for a
  plugin's `compose` request.
- `jp_cli/src/cmd/target.rs`: the conversation pickers (`Select` twice,
  `MultiSelect` twice).
- `jp_cli/src/cmd/conversation/title.rs`: `Select` and `Text`.
- `jp_cli/src/cmd/lock.rs`: `Select`.
- `jp_cli/src/cmd/conversation/edit.rs`: `Confirm`.
- `jp_cli/src/cmd/init.rs`: `Text` twice, `Select` three times.
- `jp_cli/src/cmd/workspace/target.rs`: `Select` twice.
- `jp_cli/src/bootstrap.rs`: `Select`.

## What it costs today

- **The tool coordinator cannot close most of its prompts** (T-17qrhwx).
  It closes the prompt of a settled call, and only the reply-based ones act on
  that.
  The approval prompt, a yes/no question, the result-delivery confirmation, and
  questions of type select and secret wait for a key.
- **A prompt opened on the thread that polls the command holds a graceful
  shutdown until it is answered.** That is the interrupt menus, the
  confirmations, the conversation and title pickers, the lock-wait select, and
  the editor re-open confirmation.
  Tool prompts and `compose` prompts run on threads of their own, so those are
  left behind at shutdown and do not hold it.
- **`jp init`, `jp workspace` and the workspace picker in `bootstrap` run before
  the signal router exists.** SIGTERM takes its default action there: the
  process ends at once and the terminal stays in raw mode.
  RFD 092 phase 1 (the early router) covers the signal; closing the prompt is
  the second half.

## Inline the fork

The commit sequence T-1dhk659 uses for rustqual: import verbatim, trim,
reformat, fix under the workspace lints, wire in.

- [ ] Import the `inquire` crate from the fork at `0b68068` into
      `crates/contrib/inquire`, verbatim, with its `LICENSE`.
- [ ] Trim what JP does not build: the `termion` and `console` backends, and any
      feature whose dependencies the workspace does not already have.
- [ ] Reformat with the workspace `rustfmt` and `comfort`, and fix it under the
      workspace lints.
- [ ] Make it a workspace member and point the workspace dependency at the path.
- [ ] Remove what named the git source: the `allow-git` entry in `deny.toml` (it
      names the branch `submit-on-valid-parse`, not `merged`) and
      `[policy.inquire]` in `.config/supply-chain/config.toml`.
- [ ] Record the upstream commit in `Cargo.toml`, and keep a README section
      listing what differs from upstream, as `crates/contrib/xct2cli` does.

## Make its prompts closable

- [ ] Make `CrosstermKeyReader::read_key` poll, and consult a check between
      polls.
- [ ] Have `jp_inquire` install `close_requested` as that check.
- [ ] Leave the terminal clean when a prompt returns this way.
- [ ] Cover a closed prompt in a pty.
- [ ] Drop the wait for a prompt that cannot be closed from
      `ToolCoordinator::finish`, if no backend is left that needs it, and close
      T-17qrhwx.

The key reader:

```rust
impl InputReader for CrosstermKeyReader {
    fn read_key(&mut self) -> InquireResult<Key> {
        loop {
            if interrupt_requested() {
                return Err(InquireError::OperationInterrupted);
            }
            if !event::poll(POLL_INTERVAL)? {
                continue;
            }
            if let event::Event::Key(key_event) = event::read()? {
                if KeyEventKind::Press == key_event.kind {
                    return Ok(key_event.into());
                }
            }
        }
    }
}
```

`interrupt_requested` reads a process-wide `fn() -> bool` the host installs
once.
Nothing JP-specific goes into `inquire`: it knows a check, not what the check
means.

`close_requested` reads the `PromptCloser` of the thread it is called on, and
`read_key` runs on the prompt's thread, so all 22 call sites close with no
change of their own.

`Prompt::prompt` returns `OperationInterrupted` without redrawing, so decide per
widget what stays on screen (the list select draws several rows) and erase the
rest.

`crates/jp_inquire/tests/inline_select_pty.rs` pins the single-key prompt's
screens as they are today, and `prompt_probe select <file>` already calls
`close_all` once the file appears.
The test to add opens the prompt, writes the file, and expects
`Err(OperationInterrupted)` with the prompt's rows gone.

Done when a tool approval prompt is closed once its call is settled, and an
interrupt menu or a conversation picker left open does not hold a SIGTERM.
