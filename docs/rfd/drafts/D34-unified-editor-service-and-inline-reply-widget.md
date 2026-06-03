# RFD D34: Unified Editor Service and Inline Reply Widget

- **Status**: Draft
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-05-08

## Summary

Consolidate JP's three independent editor-invocation paths onto a single
`EditorBackend` service that fully respects `EditorConfig`, and replace the
interrupt-menu reply prompt with a richer inline editing widget that accepts
short replies inline, supports multi-line input, and escalates to the configured
editor on demand.

## Motivation

JP today opens an external editor through three separate code paths, each with
different fidelity to the user's `EditorConfig`:

| Path                           | Where                                      | Accepts                             | Fidelity                               |
| ------------------------------ | ------------------------------------------ | ----------------------------------- | -------------------------------------- |
| **A. Duct-based, file-based**  | `jp_cli/src/editor.rs::open()`             | full `duct::Expression` from        | full                                   |
|                                |                                            | `EditorConfig::command()` (incl.    |                                        |
|                                |                                            | shell-style `cmd`)                  |                                        |
| **B. `open-editor`-based**     | `jp_editor::TerminalEditorBackend` (used   | single `Utf8PathBuf` from           | partial — args dropped by the          |
|                                | in `prompter.rs` for tool argument         | `EditorConfig::path()`              | `Utf8PathBuf` return type, even though |
|                                | editing, skip-reason, result edit)         |                                     | `EditorConfig` now carries them        |
| **C. `inquire::Editor`-based** | \`jp\_inquire::prompt::TerminalPromptBacke | nothing — reads `EDITOR` / `VISUAL` | none — ignores `editor.cmd`,           |
|                                | nd::text\_input\` (used by                 | directly via `inquire`              | `editor.envs`, `JP_EDITOR`             |
|                                | `InterruptHandler` for the `Reply:`        |                                     |                                        |
|                                | prompt in streaming and tool interrupts)   |                                     |                                        |

The reported symptom is path C: pressing `r` after a `Ctrl+C` opens whatever
`$EDITOR` resolves to, ignoring JP's editor configuration entirely.
Path B has a less visible companion bug: its `Utf8PathBuf` return type cannot
carry argument vectors, so even with the env-var fix landed in `EditorConfig`
(see [ubiquitous-language: CommandConfig][cmd-cfg]) it loses any flags the user
attached — `EditorConfig::command()` carries them, but the path-based wiring
throws them away.
Migrating path B to `EditorConfig::command()` closes that gap.

The structural cause is that JP has no first-class concept of "the act of
running the user's editor."
`EditorConfig` describes which editor to use; nothing in the codebase represents
the invocation.
Three call sites each filled the gap differently.
The fix is to introduce that concept and migrate the call sites onto it.

The interrupt-menu reply UX is also weak independently of the bug.
Today's `r` flow goes straight into `inquire::Editor`'s "press `e` to edit,
`Enter` to submit" two-step prompt, which forces the editor for every reply,
however short.
A first-class inline editor — with a rich editing experience and a `Ctrl+E`
escape hatch — covers both short replies and long ones without forcing a
process-spawn for the trivial case.

## Design

### User-facing behavior

#### `r` (Reply) in the streaming-interrupt menu

When the user presses `Ctrl+C` during streaming and chooses `r`:

1. An inline reply prompt appears with the cursor inside an editable buffer.
2. The user types a reply directly.
   Standard line-editing keybindings work (Ctrl+A/E for line nav, Ctrl+W to kill
   word, arrow keys, Ctrl+L to clear, word movement, kill-ring, etc.).
3. **Enter** submits a non-empty buffer; on an empty buffer Enter is ignored.
4. **Shift+Enter** (on terminals supporting the kitty keyboard protocol),
   **Alt+Enter**, or **Ctrl+J** insert a newline.
   The portable fallback is advertised in the help line.
5. **Ctrl+E** opens the configured editor seeded with the current buffer
   contents.
6. **Esc** returns the user to the interrupt menu without sending anything.

After the editor closes (when invoked via `Ctrl+E`):

- If the editor's output is empty, control returns to the interrupt menu.
- If the editor's output is non-empty, the inline reply prompt re-appears with
  the editor's output as the buffer; the user must press Enter to send.

#### `s` (Stop & Reply) in the tool-interrupt menu

Same flow as `r`, with one difference: an empty Enter or Esc does not return to
the menu — it falls through to today's `DEFAULT_TOOL_CANCELLED_RESPONSE` canned
message, which is delivered to the LLM.
This preserves the existing "interrupt a tool with no explanation" shortcut.

#### `jp query` (initial prompt)

Unchanged behavior.
The user's first prompt opens directly in the configured editor, and an empty
save cancels the query — the existing `editor::edit_query` flow already uses
`EditorConfig::command()` correctly.

#### Tool argument editing, skip reasoning, result editing

All three currently route through `ToolPrompter` via `TerminalEditorBackend`.
After this RFD they use the same `Arc<dyn EditorBackend>` as everything else,
which means a `JP_EDITOR="subl -w"` value finally honors the `-w` flag (the
silent arg-drop bug closes as a side effect).

### Architecture

#### `EditorBackend` becomes the canonical editor service

`jp_editor::EditorBackend` keeps its existing trait shape:

```rust
pub trait EditorBackend: Send + Sync {
    fn edit(&self, content: &str) -> Result<String, EditorError>;
}
```

The terminal implementation changes from path-based to invocation-based:

```rust
pub struct TerminalEditorBackend {
    cmd: duct::Expression,
}

impl EditorBackend for TerminalEditorBackend {
    fn edit(&self, content: &str) -> Result<String, EditorError> {
        // Write `content` to a tempfile, run `cmd` with the tempfile path
        // appended as an argument, read the result back, delete the tempfile.
    }
}
```

The tempfile-and-run logic is the same dance that `jp_cli/src/editor.rs::open()`
performs today; we extract it into `jp_editor` so there is one implementation.

`open-editor` is removed as a dependency from both `jp_editor` and `jp_llm`.
The `ToolError::OpenEditorError` variant in `jp_llm/src/error.rs` becomes a
generic `EditorError` exported by `jp_editor`.

#### One construction site

`turn_loop.rs` builds the editor backend once per turn alongside the existing
`prompter`:

```rust
let editor: Option<Arc<dyn EditorBackend>> = build_editor_backend(&cfg.editor);

let prompter = Arc::new(ToolPrompter::new(
    printer.clone(),
    editor.clone(),
    prompt_backend.clone(),
));

// InterruptHandler::with_backend gets the same editor:
let handler = InterruptHandler::with_backend(backend, editor.clone());
```

`build_editor_backend` is a thin free function in `jp_cli/src/editor.rs`.
It calls `EditorConfig::command()` and wraps the result in
`TerminalEditorBackend`.
Promoting the function to `jp_editor` or `jp_config` is deferred until a second
consumer needs it (YAGNI).

#### `jp_cli::editor::edit_query` keeps its specialized form

The query editor flow has genuinely different semantics: a persistent
`QUERY_MESSAGE.md` file in the conversation root, a `RevertFileGuard`, custom
CWD support, and a TOML preamble for inline config edits.
Forcing it through the `String → String` `EditorBackend` shape would lose
those.

`edit_query` continues to call `EditorConfig::command()` directly.
The `build_editor_backend` helper and `edit_query` share the same `command()`
source, so they stay consistent.
If a future RFD adds a richer trait method (`edit_file_with_seed`, etc.),
`edit_query` can migrate then.

#### `InlineReply` widget in `jp_inquire`

A new widget alongside `InlineSelect`, built on [reedline]:

```rust
pub struct InlineReply {
    message: String,
    initial_text: String,
    help_message: Option<String>,
}

impl InlineReply {
    pub fn new(message: impl Into<String>) -> Self;
    pub fn with_initial_text(self, text: impl Into<String>) -> Self;
    pub fn with_help_message(self, msg: impl Into<String>) -> Self;
    pub fn prompt(&self) -> Result<ReplyOutcome, InquireError>;
}

pub enum ReplyOutcome {
    /// User pressed Enter on a non-empty buffer (or empty, where the caller's
    /// policy permits it).
    Submit(String),
    /// User pressed Esc.
    Cancelled,
    /// User pressed Ctrl+E. Caller opens the editor seeded with `current_text`.
    OpenEditor { current_text: String },
}
```

Reedline provides the rich editing experience out of the box: emacs-style
keybindings, multi-line cursor navigation, kill-ring, undo, word movement.
Custom bindings layered on top:

| Key         | Action           | Mechanism                                                          |
| ----------- | ---------------- | ------------------------------------------------------------------ |
| Enter       | submit (default) | reedline default                                                   |
| Shift+Enter | newline          | reedline `EditCommand::InsertChar('\n')` (requires kitty protocol) |
| Alt+Enter   | newline          | same edit command (universal portable)                             |
| Ctrl+J      | newline          | same edit command (readline convention)                            |
| Esc         | cancel           | mapped to a custom `ReedlineEvent` that returns `Cancelled`        |
| Ctrl+E      | open editor      | mapped to a custom `ReedlineEvent` that returns `OpenEditor`       |

The widget enables `KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES` during
the prompt so Shift+Enter works on supporting terminals; Alt+Enter and Ctrl+J
cover the rest.
The help line advertises the portable fallback.

The widget itself does **not** open the editor.
It signals intent via `OpenEditor`; the caller (the `InterruptHandler`, or
`ToolPrompter` if the widget is reused there later) owns the editor decision.
This keeps `jp_inquire` free of editor concerns.

#### `PromptBackend::inline_reply`

Replaces the removed `text_input` method:

```rust
pub trait PromptBackend: Send + Sync {
    // ... existing methods ...

    fn inline_reply(
        &self,
        message: &str,
        initial_text: &str,
    ) -> Result<ReplyOutcome, InquireError>;
}
```

`MockPromptBackend` gains `with_reply_outcomes(impl IntoIterator<Item =
ReplyOutcome>)` so tests can script the entire flow including editor escapes:
`[OpenEditor { ... }, Submit("done")]`.

#### `InterruptHandler` becomes a loop

Today's `handle_streaming_interrupt` and `handle_tool_interrupt` are
single-shot: they show the menu, get a choice, return.
The new design is a loop, so `Cancelled` from the inline reply returns to the
menu:

```rust
pub fn handle_streaming_interrupt(
    &self,
    stream_alive: bool,
) -> InterruptAction {
    loop {
        let choice = self.backend.inline_select(...).unwrap_or('s');
        match choice {
            'c' if stream_alive => return InterruptAction::Resume,
            'c'                   => return InterruptAction::Continue,
            's'                   => return InterruptAction::Stop,
            'a'                   => return InterruptAction::Abort,
            'r' => match self.collect_reply("Reply:") {
                Some(text) => return InterruptAction::Reply(text),
                None       => continue, // back to menu
            },
            _ => unreachable!(),
        }
    }
}

fn collect_reply(&self, message: &str) -> Option<String> {
    let mut buffer = String::new();
    loop {
        match self.backend.inline_reply(message, &buffer).ok()? {
            ReplyOutcome::Submit(text) if !text.is_empty() => return Some(text),
            ReplyOutcome::Submit(_)    => return None, // empty: back to menu
            ReplyOutcome::Cancelled    => return None,
            ReplyOutcome::OpenEditor { current_text } => {
                let editor = self.editor.as_ref()?;
                let edited = editor.edit(&current_text).ok()?;
                if edited.trim().is_empty() {
                    return None; // empty editor: back to menu
                }
                buffer = edited; // re-seed the inline prompt
            }
        }
    }
}
```

The tool-interrupt `s` flow uses the same `collect_reply` helper but substitutes
`DEFAULT_TOOL_CANCELLED_RESPONSE` for `None`, preserving today's canned-message
semantics.

### Empty-Enter policy

The widget always returns `Submit(text)` on Enter — empty or not.
Per-call-site policy:

| Call site                                      | Empty-text policy                                                |
| ---------------------------------------------- | ---------------------------------------------------------------- |
| Streaming `r`                                  | Empty → `Cancelled` (back to menu)                               |
| Tool `s`                                       | Empty → fall through to canned `DEFAULT_TOOL_CANCELLED_RESPONSE` |
| Tool permission `r` (`prompter.rs::edit_text`) | Empty → `None` (skip with no reason)                             |

Keeping the policy out of the widget matches the project's "code where it
belongs" principle — the meaning of "empty" is the caller's domain.

### Ubiquitous-language additions

Two terms enter the glossary:

- **EditorBackend** — the trait abstracting "open the user's editor with
  content X, get content back."
  The single seam for ephemeral string-in/string-out editing.
- **InlineReply** — the `jp_inquire` widget for short replies in interrupt
  menus; supports inline typing with a `Ctrl+E` escape to the `EditorBackend`.

## Drawbacks

- **Reedline dependency.** Adds ~6 transitive dependencies to `jp_inquire`
  (`reedline`, `nu-ansi-term`, `unicode-segmentation`, `unicode-width`, plus
  small utilities).
  `crossterm`, `serde`, and `strip-ansi-escapes` are already in the tree.
- **Reedline draws directly to stdout** rather than accepting a `&mut dyn
  Write`.
  The `PromptBackend::inline_reply` writer parameter is dropped (the existing
  widgets use the writer for the prompt-line prefix; reedline owns its own
  prompt rendering via the `Prompt` trait).
  This is consistent with how `inquire::Editor` already behaves today.
- **More code in `jp_editor`.** Replacing the `open-editor` one-liner with a
  duct-based tempfile dance is 50–80 LOC of real terminal-process plumbing
  (Tesler's Law: the complexity has to live somewhere; the right somewhere is
  here).
- **Behavior change for the `r` flow.** Pressing `r` no longer goes straight to
  the editor.
  The user has not released JP yet and explicitly accepts this; flagged for
  completeness.

## Alternatives

### Alt 1: targeted patch — wire JP's editor into `inquire::Editor`

Configure `inquire::Editor` with `with_editor_command` / `with_args` from the
resolved JP config.
**Rejected.** Inquire's `Editor` only takes a single binary plus arg slice; it
cannot represent shell-style `cmd: Some("foo && bar")`.
Adds a parameter to `text_input` that no other prompt method has.
Doesn't fix path B. Solves the symptom without addressing the design.

### Alt 2: strangle `text_input` only, leave `EditorBackend` shape unchanged

Push editor invocation up to the `InterruptHandler` call site using the existing
`EditorBackend` trait.
Closes path C but leaves path B's silent arg-drop in place.
**Rejected** in favor of doing both together — they touch the same wiring and
splitting them means rewriting the wiring twice.

### Alt 3: defer to RFD 080

[RFD 080] restructures editor invocation around config resolution.
**Rejected as a substitute, accepted as orthogonal.** RFD 080 is about *which*
editor config wins; this RFD is about *how the resolved editor is invoked*.
Both can land independently.

The env-var parsing fix (shlex-split values like `JP_EDITOR="code -w"`) already
landed at the `EditorConfig` layer ahead of this RFD, so path B's remaining
defect is purely the `Utf8PathBuf`-return-type discarding of args.
Migrating to `EditorConfig::command()` closes it without any further
config-shape work.

### Alt 4: build a custom inline editor on raw crossterm

Avoid the reedline dependency by hand-rolling a multi-line editor in
`jp_inquire`.
**Rejected.** Reedline is well-maintained (used by nushell) and gives us
emacs-style line editing, kill-ring, multi-line cursor navigation, and undo for
free.
Reimplementing those badly is a worse use of time than the dependency cost.

## Non-Goals

- **Full unification of path A.** `jp_cli::editor::edit_query` keeps its
  specialized form.
  Folding it into `EditorBackend` requires either a richer trait method or a
  separate trait.
  Deferred.
- **Backward compatibility for the `r` flow.** Pre-release; UX changes are fair
  game.
- **Editor selection at runtime.** This RFD does not introduce per-context
  editor configuration (e.g., a different editor for inline replies vs. the
  query prompt).
  One editor is configured; one editor is used.
- **Arrow-key UX inside `InlineSelect`** or other existing widgets.
  Out of scope.

## Risks and Open Questions

- **Reedline / `Printer` coordination.** Reedline draws directly to stdout.
  JP's `Printer` synchronizes streamed LLM output, tool renderings, and prompt
  output through a shared queue.
  The widget needs to drain `Printer` before taking over the terminal and
  restore cleanly after.
  The current `InlineSelect` widget already handles this via
  `Printer::flush_instant()` and `Printer::prompt_writer()`; the `InlineReply`
  widget needs the same treatment.
  Validate during implementation.
- **Reedline's prompt rendering.** Reedline's `Prompt` trait is opinionated
  about how the prompt prefix renders (with built-in indicators for vi mode,
  history search, etc.).
  The widget's `Prompt` impl must match JP's existing prompt style (the
  `[c,r,s,a,?]?`-style line).
  Likely doable in 30 LOC but worth a spike.
- **`Ctrl+E` collision.** Emacs-style line editing binds `Ctrl+E` to "move to
  end of line."
  Overriding it in this widget breaks the muscle memory of users who expect that
  binding inside reedline.
  Considered acceptable for a reply prompt (the buffer is short; "move to end of
  line" matters less than "open editor"), but flagged.
- **`PromptBackend::inline_reply` returning `OpenEditor` from a mock.** Tests
  need to script editor-escape flows.
  The mock implementation is straightforward — script a vector of
  `ReplyOutcome` values — but verify it composes cleanly with the existing
  `MockEditorBackend` for full end-to-end tests of the loop.

## Implementation Plan

### Phase 1: structural — `EditorBackend` becomes canonical

- Re-shape `TerminalEditorBackend` around `duct::Expression`.
  Extract the tempfile-and-run dance from `jp_cli/src/editor.rs::open()`.
- Drop `open-editor` from `jp_editor` and `jp_llm`.
  Replace `ToolError::OpenEditorError` with a generic `EditorError` exported by
  `jp_editor`.
- Add `build_editor_backend` helper in `jp_cli/src/editor.rs`.
- Update `ToolPrompter` to receive `Option<Arc<dyn EditorBackend>>` instead of
  `Option<Utf8PathBuf>`; update both `turn_loop.rs` construction sites.
- Update tests using `cfg.editor.path()` style construction.

Reviewable independently.
Closes path B's arg-drop bug as a side effect; no user-visible UX change yet.

Estimated diff: ~300 LOC.

### Phase 2: `InlineReply` widget

- Add `reedline` dependency to `jp_inquire`.
- Implement `InlineReply` widget with the keybindings and `ReplyOutcome` enum
  described above.
- Implement a minimal `Prompt` impl that matches JP's prompt-line style.
- Snapshot tests for keybinding behavior using reedline's testable input stream
  (or a thin shim).

Reviewable independently.
No call-site changes yet — pure addition.

Estimated diff: ~250 LOC.

### Phase 3: `PromptBackend` integration and `InterruptHandler` rewiring

- Remove `text_input` from `PromptBackend`.
  Drop the `editor` feature on `inquire`.
- Add `inline_reply` method to `PromptBackend` and update
  `TerminalPromptBackend` and `MockPromptBackend`.
- Add `editor: Option<Arc<dyn EditorBackend>>` field to `InterruptHandler`;
  thread the editor through both `InterruptHandler::with_backend` call sites in
  `interrupt/signals.rs`.
- Rewrite `handle_streaming_interrupt` and `handle_tool_interrupt` as loops with
  the `collect_reply` helper.
- Update existing handler tests; add tests for the `Cancelled → menu → submit`
  and `OpenEditor → empty → menu` paths.

Depends on Phases 1 and 2.
Closes path C. Ships the new `r` UX.

Estimated diff: ~400 LOC, mostly tests.

### Phase 4: glossary and docs

- Add **EditorBackend** and **InlineReply** to
  `docs/architecture/ubiquitous-language.md`.
- Update any user-facing docs that describe the `r` flow.

Reviewable independently after Phase 3.

## References

- [RFD 045]: Layered Interrupt Handler Stack — context for the existing
  interrupt-handler architecture
- [RFD 080]: Editor as a Config Source — orthogonal concern; resolves *which*
  editor config wins, not *how* the editor is invoked
- [reedline] — line-editor crate proposed for `InlineReply`
- `crates/jp_editor/src/lib.rs` — current `EditorBackend` trait
- `crates/jp_inquire/src/prompt.rs` — current `PromptBackend` trait, including
  the `text_input` method to be removed
- `crates/jp_cli/src/cmd/query/interrupt/handler.rs` — current
  `InterruptHandler` (path C)
- `crates/jp_cli/src/cmd/query/tool/prompter.rs` — current `ToolPrompter`
  construction (path B)
- `crates/jp_cli/src/editor.rs` — current `editor::open()` and
  `editor::edit_query` (path A)
- `crates/jp_config/src/editor.rs` — `EditorConfig::command()` and `path()`

[RFD 045]: ../045-layered-interrupt-handler-stack.md
[RFD 080]: ../080-editor-as-a-config-source.md
[cmd-cfg]: ../../architecture/ubiquitous-language.md#commandconfig
[reedline]: https://crates.io/crates/reedline
