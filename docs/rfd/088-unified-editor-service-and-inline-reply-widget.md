# RFD 088: Unified Editor Service and Inline Reply Widget

- **Status**: Implemented
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-05-08
- **Extends**: [RFD 048]
- **Extended by**: [RFD 092], [RFD 093]
- **Summary**: Unifies external editor calls under `EditorBackend` and adds a
  `reedline`-based `InlineReply` widget for interrupt-menu replies.

## Summary

This RFD consolidates JP's four editor-invocation paths onto a single
`EditorBackend` trait (`edit_text` and `edit_file`) that honors `EditorConfig`.
It also replaces the interrupt-menu reply prompt with a [reedline]-based
`InlineReply` widget that supports multi-line inline input and on-demand
escalation to the external editor, controlled by a per-context
`compose_in_editor` setting.

## Motivation

JP opens an external editor through four separate code paths, each with
different fidelity to `EditorConfig`:

| Path                           | Where                                          | Accepts                                          | Fidelity                                                |
| ------------------------------ | ---------------------------------------------- | ------------------------------------------------ | ------------------------------------------------------- |
| **A. Duct-based, file-based**  | `jp_cli/src/editor.rs::open()`                 | full `duct::Expression` from `command()`         | full                                                    |
| **B. `open-editor`-based**     | `jp_editor::TerminalEditorBackend` (tool args, | single `Utf8PathBuf` from `EditorConfig::path()` | partial: args dropped by the `Utf8PathBuf` return type  |
|                                | skip-reason, result edit in `prompter.rs`)     |                                                  |                                                         |
| **C. `inquire::Editor`-based** | `TerminalPromptBackend::text_input` (the       | nothing: reads `EDITOR` / `VISUAL` directly      | none: ignores `editor.cmd`, `editor.envs`, `JP_EDITOR`  |
|                                | `Reply:` prompt in streaming/tool interrupts)  |                                                  |                                                         |
| **D. Duct-based, file-based**  | `jp conversation edit`                         | full `duct::Expression` from `command()`         | full: already correct, though omitted from the original |
|                                | (`cmd/conversation/edit.rs`)                   |                                                  | "three paths" framing                                   |

The user-visible bug is path C: pressing `r` after `Ctrl+C` opens whatever
`$EDITOR` resolves to and ignores JP's editor configuration.
Path B has a less visible bug: `EditorConfig::path()` returns a `Utf8PathBuf`
that cannot carry argument vectors.
Even with the environment-variable parsing fix in `EditorConfig` (see
[ubiquitous-language: CommandConfig][cmd-cfg]), path B drops any flags the user
configured because `EditorConfig::command()` carries them and the path-based
wiring discards them.
Migrating path B to `EditorConfig::command()` closes that gap.

The root cause is that JP has no abstraction for invoking the user's editor.
`EditorConfig` describes which editor to run, but nothing represents the
invocation itself.
Paths A and D honor `EditorConfig::command()`, while paths B and C do not.
Introducing a first-class `EditorBackend` and migrating all four call sites onto
it fixes both bugs.

The interrupt-menu reply UX is also weak on its own.
The `r` flow enters `inquire::Editor`'s two-step prompt ("press `e` to edit,
`Enter` to submit"), which spawns an external editor for every reply, no matter
how short.
An inline editor with standard line-editing bindings and a `Ctrl+X` shortcut to
open the external editor (modeled on readline's `edit-and-execute-command`)
handles both short and long replies without spawning a process for trivial
input.

## Design

### User-facing behavior

#### `r` (Reply) in the streaming-interrupt menu

When the user presses `Ctrl+C` during streaming and chooses `r`:

1. An inline reply prompt appears with the cursor in an editable buffer.
2. The user types a reply directly.
   Standard line-editing keybindings work (`Ctrl+A` and `Ctrl+E` for line
   navigation, `Ctrl+W` to delete a word, arrow keys, `Ctrl+L` to clear, word
   movement, and the kill ring).
3. **Enter** submits the buffer.
   An empty buffer follows the call site's empty-Enter policy (for streaming
   `r`, returning to the menu; see [Empty-Enter policy](#empty-enter-policy)).
4. **Shift+Enter** (on terminals that support the kitty keyboard protocol:
   kitty, Ghostty, WezTerm, foot) or **Alt+Enter** (portable fallback) inserts a
   newline.
   The help line advertises the fallback.
5. **Ctrl+X** opens the configured editor seeded with the current buffer
   contents.
6. **Ctrl+C** returns to the interrupt menu without sending anything.
   A second **Ctrl+C** at the menu escalates to graceful shutdown (see [RFD
   045]).

When invoked via `Ctrl+X`, closing the external editor always returns to the
inline reply prompt rather than submitting directly:

- A non-empty result replaces the buffer contents, and the user presses Enter to
  send.
- An empty result or aborted editor leaves the buffer empty.
  The user can type again, submit an empty buffer, or press `Ctrl+C` to return
  to the menu.

**Opt-in: straight to the editor.** Setting
`interrupt.streaming.compose_in_editor = true` skips the inline widget for `r`
and opens the configured editor immediately with an empty buffer.
A non-empty saved result is sent directly.
An empty or cancelled result falls back to the inline prompt, where the user can
type, submit an empty buffer, or press `Ctrl+C` to return to the menu.

#### `r` (Stop & respond) in the tool-interrupt menu

This uses the same flow as streaming `r`, except at the inline prompt: pressing
Enter on an empty buffer stops the tool with the canned
`DEFAULT_TOOL_CANCELLED_RESPONSE` message, preserving the shortcut to interrupt
a tool without explanation.
`Ctrl+C` still returns to the menu (where a second `Ctrl+C` escalates), and
`Ctrl+X` still returns to the inline prompt after the editor closes.

Setting `interrupt.tool_call.compose_in_editor = true` opens the editor
immediately; an empty or cancelled result falls back to the inline prompt, where
an empty submit sends the canned message.

#### `jp query` (initial prompt)

Behavior is unchanged.
The initial prompt opens directly in the configured editor, and saving an empty
file cancels the query.
The existing `editor::edit_query` flow already uses `EditorConfig::command()`.

#### Tool argument editing, skip reasoning, result editing

All three flows currently route through `ToolPrompter` via
`TerminalEditorBackend`, and each is gated on a configured editor at its prompt
site: `permission_options` (`r` and `e`), `prompt_result_confirmation` (the `e`
result option), and the coordinator's `can_prompt` check for `ResultMode::Edit`.
This RFD converts all three to `InlineReply` widgets seeded with the current
text (the JSON arguments, the skip-reason placeholder, or the tool result), with
`Ctrl+X` opening the configured editor on demand.

Because `InlineReply` requires only a TTY, the permission-menu options `r`
("Skip and reply") and `e` ("Edit arguments") no longer require a configured
editor.
They appear whenever a prompt can be shown, and only the `Ctrl+X` shortcut
requires `editor.cmd`.
Result-delivery confirmation is ungated the same way: "Edit result first" (`e`)
appears whenever a TTY is present, and `ResultMode::Edit` prompts on any TTY
rather than requiring an editor.
When no editor is configured, `Ctrl+X` is a no-op and the inline widget stays
open.
Routing `Ctrl+X` through `EditorConfig::command()` also fixes the
`JP_EDITOR="subl -w"` argument-dropping bug.

The argument editor keeps its parse-error loop: on invalid JSON, the caller
re-seeds the `InlineReply` buffer with the user's edits and re-prompts with the
parse error in the prompt line.
This preserves edits without respawning a process and removes the "Re-open
editor?
y/n" confirmation step.
`Ctrl+X` still opens the external editor for larger rewrites, and its result
validates through the same loop.
Submitting an empty buffer abandons the edit and returns to the Ask prompt with
the original arguments unchanged, rather than submitting empty JSON or running
the tool with cleared arguments.

### Architecture

#### `EditorBackend`: the frontend seam for editor invocation

Two invariants govern every editor invocation in JP:

1. Every string-in/string-out edit goes through `EditorBackend`.
2. Every editor spawned by a command-spawning frontend (terminal or native app)
   resolves its command through `EditorConfig::command()` and shares open/wait
   logic.

`EditorBackend` gains a second method to cover both string and file editing:

```rust
pub trait EditorBackend: Send + Sync {
    /// Ephemeral string editing: content in, edited content out.
    fn edit_text(&self, content: &str) -> Result<(EditOutcome, String), EditorError>;

    /// Open the user's editor on the requested path(s) and block until editing
    /// is done.
    /// The edited content is read back from disk by the caller.
    fn edit_file(&self, req: EditRequest<'_>) -> Result<EditOutcome, EditorError>;
}

/// Frontend-agnostic request data for `edit_file`.
/// Backend-specific context (a web session, a native window handle, …) lives
/// in the backend's own fields, set at construction — *not* here — so
/// `EditorBackend` stays object-safe behind `Arc<dyn EditorBackend>`.
pub struct EditRequest<'a> {
    pub paths: &'a [Utf8PathBuf],
    /// Working directory for a spawned editor; frontends that don't spawn a
    /// local process ignore it.
    pub cwd: Option<&'a Utf8Path>,
}

/// The interaction outcome — the part only the backend can know.
pub enum EditOutcome {
    /// User saved and closed (terminal editor exited 0; explicit save in a
    /// GUI).
    Saved,
    /// User aborted (terminal editor exited non-zero; explicit cancel in a
    /// GUI).
    Cancelled,
}
```

`edit_text` returns `(EditOutcome, String)`.
On `Cancelled`, callers ignore the returned string.
`TerminalEditorBackend` maps a non-zero editor exit to `Cancelled`, matching
Git's commit-message convention.
An `Err` (`EditorError::Spawn` or `Io`) is distinct from `Cancelled`: it means
the editor process failed to start or file I/O failed, so callers report the
error and recover (falling back to the inline widget while preserving the typed
buffer) rather than treating it as a user cancellation.
Because `editor.cmd` defaults to `shell = false` (direct spawn), a missing
editor binary returns a spawn error rather than a shell's non-zero exit code.
Checking whether the content was modified, left unchanged, or emptied (`new ==
old`, `new.trim().is_empty()`) is the caller's responsibility rather than the
backend's.

Backends are named after their frontend environment rather than their storage
medium:

- `TerminalEditorBackend`: writes a temporary file and spawns the editor with
  `duct` via `EditorConfig::command()`.
  This is the only implementation shipped in this RFD.
- `WebEditorBackend`, `NativeEditorBackend`: future frontends covered by the
  web-UI and native-UI RFDs.
  A web frontend mediates file editing through its server, while a native
  frontend spawns the application on the file path.
  Both implement the same two methods, and only command-spawning backends use
  `EditorConfig::command()`.
- `MockEditorBackend`: scripts outcomes for tests, including end-to-end flows
  that exercise `Ctrl+X` without spawning a process.

When no editor is configured, the editor handle is `None` (`Option<Arc<dyn
EditorBackend>>`) rather than a no-op backend.

`TerminalEditorBackend` extracts the temporary-file creation and execution logic
from `jp_cli/src/editor.rs::open()` into `jp_editor` so all callers share one
implementation.

`open-editor` is removed from both `jp_editor` and `jp_llm`.
The unused `ToolError::OpenEditorError` variant in `jp_llm/src/error.rs` is
deleted, and editor failures are represented by `EditorError` in `jp_editor` and
`jp_cli`.
`jp_llm` does not depend on `jp_editor`.

#### Single construction helper

A single function, `build_editor_backend(&cfg.editor)` in
`jp_cli/src/editor.rs`, calls `EditorConfig::command()` and wraps the result in
`TerminalEditorBackend`.
Every call site obtains its editor through this helper:

- **Query startup** (`cmd/query.rs`) builds it for `edit_query`.
- **The turn loop** (`turn_loop.rs`) builds it once per turn and shares the
  `Arc<dyn EditorBackend>` between `ToolPrompter` and `InterruptHandler`.
- **`jp conversation edit`** (`cmd/conversation/edit.rs`) builds it for
  `edit_file`.

The turn-loop construction site:

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

Moving `build_editor_backend` out of `jp_cli` into `jp_editor` or `jp_config` is
deferred because all three call sites live in `jp_cli`.

#### File-based flows route through `edit_file`

`jp_cli::editor::edit_query` and `jp conversation edit` (path D) keep their
file-management policies at the call site: `edit_query` manages the persistent
`QUERY_MESSAGE.md`, `RevertFileGuard`, and TOML preamble, while `conversation
edit` handles validation, rollback, and projection sync.
Those policies belong to the callers rather than `EditorBackend`.

Both commands invoke the editor through `edit_file`, sharing the open/wait
implementation with the rest of JP and supporting future web and native backends
without caller changes.
`EditRequest` carries the working directory (`edit_query` sets `cwd =
conversation_root`), which frontends that do not spawn a local process ignore.
`edit_file` opens the editor on the requested paths in that directory and blocks
until the editor closes; the caller then reads the files back and runs its own
validation, rollback, and content checks.

`edit_file` returns `EditOutcome`, and each caller handles `Cancelled` directly:
`edit_query` cancels the query without sending, and `conversation edit` restores
its pre-edit snapshots and reports the cancellation.

#### `InlineReply` widget in `jp_inquire`

`InlineReply` is a new widget alongside `InlineSelect`, built on a patched fork
of [reedline] (see [Terminal
ownership](#terminal-ownership-and-the-reedline-fork)):

```rust
pub struct InlineReply {
    message: String,
    initial_text: String,
    help_message: Option<String>,
    edit_mode: ReplyEditMode,
    editor_escape: bool,
}

impl InlineReply {
    pub fn new(message: impl Into<String>) -> Self;
    pub fn with_initial_text(self, text: impl Into<String>) -> Self;
    pub fn with_help_message(self, msg: impl Into<String>) -> Self;
    pub fn with_edit_mode(self, mode: ReplyEditMode) -> Self;
    pub fn with_editor_escape(self, enabled: bool) -> Self;
    pub fn prompt(&self, output: Box<dyn Write + Send>) -> Result<ReplyOutcome, InquireError>;
}

pub enum ReplyOutcome {
    /// User pressed Enter on a non-empty buffer (or empty, where the caller's
    /// policy permits it).
    Submit(String),
    /// User cancelled the prompt with `Ctrl+C`.
    /// The caller decides what this means — return to a menu, use a fallback,
    /// or escalate.
    Cancelled,
    /// User pressed Ctrl+X.
    /// Caller opens the editor seeded with `current_text`.
    OpenEditor { current_text: String },
}
```

Reedline provides emacs and vi keybindings, multi-line input, cursor navigation,
a kill ring, undo, word movement, Unicode width calculation, line wrapping,
bracketed paste, and terminal resize handling.
JP keeps reedline's default bindings, including Meta/Alt bindings (`Meta+B`,
`Meta+F`, `Meta+D`, `Meta+Backspace`), and layers five bindings on top:

| Key         | Action           | Mechanism                                                    |
| ----------- | ---------------- | ------------------------------------------------------------ |
| Enter       | submit (default) | reedline default                                             |
| Shift+Enter | newline          | `EditCommand::InsertNewline` (kitty protocol; incl. Ghostty) |
| Alt+Enter   | newline          | same edit command (portable fallback)                        |
| Ctrl+X      | open editor      | custom `ReedlineEvent` that returns `OpenEditor`             |
| Ctrl+C      | cancel           | reedline `Signal::CtrlC`, surfaced as `Cancelled`            |

`Ctrl+C` is the mode-independent cancel key: in raw mode it arrives as `0x03`,
reedline returns `Signal::CtrlC`, and the widget maps it to `Cancelled`.
`InlineReply` does not bind `Esc` (which serves as the Meta prefix in emacs mode
and the normal-mode switch in vi mode) or `Ctrl+Q` (which some terminal
emulators intercept).

During the prompt, the widget enables
`KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES` so `Shift+Enter` works on
terminals supporting the kitty keyboard protocol (kitty, Ghostty, WezTerm,
foot).
`Alt+Enter` provides a portable fallback and is shown in the help line.

**Configurable edit mode.** Reedline provides `Emacs` (default) and `Vi` edit
modes, configured via `editor.inline.edit_mode` (see [Configuration
changes](#configuration-changes)).
Because reedline maintains separate keybinding tables per mode, `InlineReply`
registers the custom bindings above in both keymaps.
In vi mode, it also enables `with_cursor_config` so the cursor shape reflects
insert and normal modes.

`InlineReply` does not invoke the external editor itself.
It returns `OpenEditor`, leaving editor invocation to the caller
(`InterruptHandler` or `ToolPrompter`) so `jp_inquire` stays independent of
`jp_editor`.

#### Terminal ownership and the reedline fork

[RFD 048] requires interactive prompts to render to `/dev/tty` so they remain
visible under `jp query | jq` and `jp query 2> err.txt`.
Upstream reedline does not support `/dev/tty` output:

- **Output.** Reedline's painter hardcodes stderr (`pub type W =
  BufWriter<Stderr>`, constructed in `Reedline::create()`).
  Redirecting stderr with `2> err.txt` swallows the prompt.
- **Cursor probing.** Reedline calls `crossterm::cursor::position()` during
  prompt initialization and terminal resize, and crossterm writes the `ESC[6n`
  query to stdout (`cursor/sys/unix.rs`).
  Piping stdout (`| jq`) corrupts the output stream, and the cursor query fails
  when stdout is redirected.
- **Input.** Reedline reads events through crossterm, whose `tty_fd()` uses
  stdin when it is a TTY and opens `/dev/tty` otherwise, using the same file
  descriptor for raw mode.
  Input already works when stdio is redirected.

Fixing output requires an upstream change in reedline, and fixing cursor probing
requires a change in crossterm (or removing `position()` calls from reedline).
To avoid blocking on two upstream releases, JP depends on a patched reedline
fork (`JeanMertz/reedline`, branch `changes`, declared as a Git dependency in
the workspace `Cargo.toml`) with two changes:

1. Type-erase the painter writer (`W = BufWriter<Box<dyn Write + Send>>`) and
   add `Reedline::with_output(...)`, passing the `/dev/tty` writer from
   `Printer::owned_prompt_writer()`.
2. Replace reedline's two `cursor::position()` calls with a helper that writes
   the `ESC[6n` query to that TTY writer and reads the response from the
   `/dev/tty` event source.

The `with_output` change can be upstreamed to reedline, leaving only the
cursor-probe patch in the fork.
Before taking over the terminal, `InlineReply` flushes `Printer`, and it
restores terminal state afterward, matching `InlineSelect`.

#### `PromptBackend::inline_reply`

`inline_reply` replaces `text_input` on `PromptBackend`:

```rust
pub trait PromptBackend: Send + Sync {
    // ... existing methods ...

    fn inline_reply(
        &self,
        message: &str,
        initial_text: &str,
        edit_mode: ReplyEditMode,
        editor_escape: bool, // `false` unwires the `Ctrl+X` editor escape
        help: Option<&str>,  // shown after the widget's own key hints
        output: Box<dyn Write + Send>,
    ) -> Result<ReplyOutcome, InquireError>;
}
```

Like the other `PromptBackend` methods, `inline_reply` accepts an explicit
output writer: the `jp_cli` caller passes `printer.owned_prompt_writer()` (the
`/dev/tty` target), and `TerminalPromptBackend` passes it to
`Reedline::with_output`.
Because reedline takes ownership of its writer for the duration of the prompt,
`inline_reply` takes `Box<dyn Write + Send>` rather than `&mut dyn Write`.
The boxed writer still routes through the printer's serialized worker, keeping
prompt output ordered with other terminal output while preserving the [RFD 048]
boundary (`jp_inquire` does not depend on `jp_printer`).

`MockPromptBackend` adds `with_reply_outcomes(impl IntoIterator<Item =
ReplyOutcome>)` so tests can script multi-step interactions including editor
escapes (for example, `[OpenEditor { ... }, Submit("done")]`).

#### `InterruptHandler` loop

`handle_streaming_interrupt` and `handle_tool_interrupt` become loops so that
cancelling the inline reply returns to the interrupt menu:

```rust
pub fn handle_streaming_interrupt(
    &self,
    stream_alive: bool,
    writer: &mut dyn Write,
) -> InterruptAction {
    loop {
        // A cancelled menu (Ctrl+C) escalates to graceful shutdown. Escalation
        // is RFD 045's `InterruptOutcome::Escalated` — a handler outcome, not a
        // new `InterruptAction` variant — so the real handler returns through
        // 045's outcome type; this sketch elides that as `escalate()`.
        let choice = match self.backend.inline_select(...) {
            Ok(c)  => c,
            Err(_) => return escalate(),
        };
        match choice {
            'c' if stream_alive => return InterruptAction::Resume,
            'c'                   => return InterruptAction::Continue,
            's'                   => return InterruptAction::Stop,
            'a'                   => return InterruptAction::Abort,
            'r' => match self.collect_reply("Reply:", self.compose_in_editor, writer) {
                ReplyResult::Reply(text) => return InterruptAction::Reply(text),
                // Empty submit or Ctrl+C: re-show the menu.
                ReplyResult::Empty | ReplyResult::Cancelled => continue,
            },
            _ => unreachable!(),
        }
    }
}

fn collect_reply(
    &self,
    message: &str,
    compose: ComposeInEditor,
    writer: &mut dyn Write,
) -> ReplyResult {
    // Inline-first modes (`false` / `"never"`): the Ctrl+X escape is wired only
    // for `false` (`compose.editor_escape()`).
    if !compose.starts_in_editor() {
        return self.collect_reply_inline(message, compose.editor_escape(), writer);
    }

    // Editor-first modes (`true` / `"always"`): open the editor directly.
    let Some(editor) = self.editor.as_ref() else {
        return self.editor_unavailable(message, compose, writer, None);
    };
    match editor.edit_text("") {
        Ok((EditOutcome::Saved, text)) if !text.trim().is_empty() => ReplyResult::Reply(text),
        // Empty/cancelled editor: the user bailed → back to the menu.
        Ok(_)  => ReplyResult::Cancelled,
        // Editor couldn't run: `editor_unavailable` falls back to the inline
        // widget (`true`) or returns to the menu (`"always"`), reporting the
        // failure on the chrome channel.
        Err(e) => self.editor_unavailable(message, compose, writer, Some(e)),
    }
}

fn collect_reply_inline(
    &self,
    message: &str,
    editor_escape: bool,
    writer: &mut dyn Write,
) -> ReplyResult {
    let mut buffer = String::new();
    loop {
        // Prompt errors and Ctrl+C are handled explicitly, never swallowed.
        match self.backend.inline_reply(message, &buffer, editor_escape, writer) {
            Ok(ReplyOutcome::OpenEditor { current_text }) => {
                // The editor escape always returns to the inline prompt.
                buffer = current_text;
                if let Some(editor) = self.editor.as_ref() {
                    match editor.edit_text(&buffer) {
                        Ok((EditOutcome::Saved, edited)) => buffer = edited, // even if empty
                        Ok((EditOutcome::Cancelled, _))  => {}               // keep buffer
                        Err(e) => self.report(e),                            // keep buffer
                    }
                }
            }
            Ok(ReplyOutcome::Submit(text)) if !text.trim().is_empty() => {
                return ReplyResult::Reply(text)
            }
            Ok(ReplyOutcome::Submit(_))          => return ReplyResult::Empty,     // send nothing
            Ok(ReplyOutcome::Cancelled) | Err(_) => return ReplyResult::Cancelled, // back a level
        }
    }
}
```

`collect_reply` returns `ReplyResult { Reply(String), Empty, Cancelled }` rather
than `Option<String>`, ensuring prompt errors and `Ctrl+C` are handled
explicitly rather than swallowed by `.ok()?` (avoiding the regression noted in
[RFD 045]).
The `/dev/tty` `writer` is passed from the menu loop through `collect_reply` to
`inline_reply`, matching the other `PromptBackend` methods.
The tool-interrupt `r` flow calls the same helper and maps an empty or cancelled
reply to `DEFAULT_TOOL_CANCELLED_RESPONSE`.
`compose_in_editor` is a `ComposeInEditor` value (`false`, `true`, `"always"`,
or `"never"`); `starts_in_editor()` and `editor_escape()` select the composition
path, and `editor_unavailable` handles the fallback difference between `true`
and `"always"`.

The code above shows the `action = prompt` path.
When `[interrupt].*.action` is set to a non-`prompt` action, the handler skips
the menu and runs that action directly; `reply` and `respond` call
`collect_reply` with the `compose_in_editor` setting from the same per-context
config struct, using the cancel fallbacks defined in [Configuration
changes](#configuration-changes).

### Empty-Enter policy

`InlineReply` always returns `Submit(text)` on Enter, even when `text` is empty.
Each call site defines its own empty-text policy:

| Call site                            | Empty-text policy                                                |
| ------------------------------------ | ---------------------------------------------------------------- |
| Streaming `r`                        | Empty → `Cancelled` (back to menu)                               |
| Tool `r`                             | Empty → fall through to canned `DEFAULT_TOOL_CANCELLED_RESPONSE` |
| Tool permission `r` (skip reason)    | Empty → `None` (skip with no reason)                             |
| Tool permission `e` (edit arguments) | Empty → fall back to Ask (args unchanged)                        |
| Tool result edit                     | Empty → `None` (fall back to Ask)                                |

Keeping empty-input handling in the caller separates widget mechanics from
call-site policy.

### Configuration changes

This RFD adds `compose_in_editor` under `interrupt.{streaming,tool_call}` and
`edit_mode` under `editor.inline`:

```toml
[editor]
cmd = "code --wait" # string form (shell = false), or a table (below)

[interrupt.streaming]
compose_in_editor = false # false | true | "always" | "never"

[interrupt.tool_call]
compose_in_editor = false # false | true | "always" | "never"

[editor.inline]
edit_mode = "emacs" # "emacs" | "vi"
```

- **`editor.cmd`**: accepts a string (`cmd = "code --wait"`) or a table (`cmd =
  { program = "code", args = ["--wait"], shell = false }`), reusing the
  `CommandConfig` format used by local tools.
  The string form and `shell = false` (the default) spawn the program directly,
  passing the edited file path as a positional argument and surfacing a missing
  editor binary as a spawn error rather than a non-zero shell exit.
  `shell = false` works across platforms and is recommended on Windows.
  Set `shell = true` to use pipes, `&&`, or subshells; on Unix, the edited file
  paths are passed to the shell command via `"$@"`.
  On non-Unix platforms, `shell = true` is unsupported because `"$@"` path
  forwarding is Unix-only: JP logs a warning and spawns `program` directly.
  Put shell logic in a script and reference it with `shell = false` instead.

- **`compose_in_editor`**: controls where replies and responses are composed.
  Accepts a boolean or a string:

  - `false` *(default)*: start in the inline widget, with `Ctrl+X` available to
    open `editor.cmd` on demand.
  - `true`: open `editor.cmd` directly.
    If the editor fails to start, log a notice on the chrome channel and fall
    back to the inline widget.
  - `"always"`: open `editor.cmd` directly.
    If the editor fails to start, log a notice on the chrome channel and return
    to the menu without opening the inline widget.
  - `"never"`: use the inline widget only, disabling and hiding the `Ctrl+X`
    shortcut.

  In both editor-first modes (`true` and `"always"`), saving a non-empty buffer
  sends the reply, while saving an empty buffer or cancelling the editor returns
  to the menu.
  When no TTY is available, prompts are skipped and this setting has no effect.

- **`editor.inline.edit_mode`**: selects the reedline keybinding mode (`"emacs"`
  or `"vi"`) for the inline widget, independent of the external editor
  configured in `editor.cmd`.

**Cancel and empty-submit behavior.** The table below defines the behavior
across all interrupt contexts, including configured actions that skip the menu:

| Context                  | Menu? | Empty Enter         | Ctrl+C                        |
| ------------------------ | ----- | ------------------- | ----------------------------- |
| streaming `r`            | yes   | back to menu        | back to menu (2nd → escalate) |
| streaming `action=reply` | no    | resume the response | resume the response           |
| tool `r`                 | yes   | canned message      | back to menu (2nd → escalate) |
| tool `action=respond`    | no    | canned message      | canned message                |

"Escalate" triggers graceful shutdown as defined in [RFD 045].
When a configured `action` skips the menu, cancelling has no menu to return to
and falls back to the context's default action (resuming the stream or sending
the canned tool cancellation message).

Pressing `Ctrl+C` inside `InlineReply` cancels the reply and returns to the
interrupt menu; only pressing `Ctrl+C` at the interrupt menu escalates to
graceful shutdown.

### Ubiquitous-language additions

Two terms are added to the glossary:

- **EditorBackend**: the frontend trait for invoking the user's configured
  editor, providing `edit_text` (string in/out) and `edit_file` (path-based).
  Each frontend (terminal, web, native, mock) provides its own implementation.
- **InlineReply**: the `jp_inquire` widget for composing replies in interrupt
  and tool prompts, supporting inline editing and `Ctrl+X` escalation to
  `EditorBackend`.

## Drawbacks

- **Forked reedline.** JP depends on a patched fork of reedline (see [Terminal
  ownership](#terminal-ownership-and-the-reedline-fork)), which requires
  rebasing two patches across upstream releases.
  Upstreaming the `with_output` patch will reduce the fork to the cursor-probe
  change.
  The fork adds `nu-ansi-term`, `unicode-segmentation`, and `unicode-width` to
  the dependency tree (`crossterm`, `serde`, and `strip-ansi-escapes` are
  already present).
- **Owned writer in `PromptBackend::inline_reply`.** `inline_reply` takes an
  owned writer (`Box<dyn Write + Send>`) whereas sibling `PromptBackend` methods
  take `&mut dyn Write`, because reedline owns its output stream.
  `Printer::owned_prompt_writer()` supplies the boxed writer, keeping
  `jp_inquire` independent of `jp_printer` as required by [RFD 048].
- **Additional process-spawning code in `jp_editor`.** Replacing the
  `open-editor` call with `duct`-based temporary-file management adds 50-80
  lines of process-handling code to `jp_editor`.
- **Changed default for `r`.** By default, pressing `r` opens the inline reply
  widget rather than `inquire::Editor`'s two-step "press `e`" prompt
  (`compose_in_editor = true` opens the external editor directly).
  Because JP is pre-release, changing the default requires no migration.

## Alternatives

### Alt 1: pass JP's editor config to `inquire::Editor`

Configure `inquire::Editor` with `with_editor_command` and `with_args` from the
resolved `EditorConfig`.
**Rejected.** `inquire::Editor` only accepts a binary path and argument slice
and cannot represent shell commands such as `cmd: Some("foo && bar")`.
It also requires adding an editor parameter to `text_input` that no other prompt
method uses, and it leaves path B's argument-dropping bug unfixed.

### Alt 2: replace `text_input` only and leave `EditorBackend` unchanged

Move editor invocation to the `InterruptHandler` call site using the existing
`EditorBackend` trait.
**Rejected.** This fixes path C but leaves path B's argument-dropping bug in
place.
Because both paths share the same turn-loop wiring, fixing them together avoids
rewriting that wiring twice.

### Alt 3: defer to RFD 080

Defer editor unification to [RFD 080], which restructures editor configuration
resolution.
**Rejected as a substitute; accepted as orthogonal.** [RFD 080] determines
*which* editor configuration wins, whereas this RFD determines *how* the
resolved editor is invoked.
Both can land independently.

Environment-variable splitting (`JP_EDITOR="code -w"`) already landed in
`EditorConfig`, so path B's remaining bug is only that `EditorConfig::path()`
returns a `Utf8PathBuf` without arguments.
Switching path B to `EditorConfig::command()` fixes it without further changes
to `EditorConfig`.

### Alt 4: build a custom inline editor on raw crossterm

Implement a custom multi-line editor in `jp_inquire` that writes directly
through `Printer::prompt_writer()`, satisfying [RFD 048] without forking
reedline.
**Rejected.** Reedline already handles Unicode width, line wrapping, multi-line
cursor navigation, bracketed paste, kitty keyboard protocol disambiguation,
terminal resize, undo, the kill ring, and emacs and vi editing modes.
Patching reedline's two I/O seams (output writer and cursor probe) is much
smaller and lower-risk than reimplementing a line editor.

## Non-Goals

- **Frontend backends beyond terminal.** Only `TerminalEditorBackend` ships in
  this RFD.
  `WebEditorBackend` and `NativeEditorBackend` are accommodated by the trait and
  will be implemented in the web-UI and native-UI RFDs.
- **Backward compatibility for the `r` flow.** JP is pre-release, so changing
  the default reply UX requires no compatibility shim.
- **Per-context external editor selection.** One external editor is configured
  and used across JP (`compose_in_editor` controls *whether* a reply opens that
  editor, and `editor.inline.edit_mode` controls the inline widget's
  keybindings).
- **Arrow-key navigation in `InlineSelect` or other existing widgets.**

## Risks and Open Questions

- **Reedline fork and `Printer` coordination.** The fork renders through
  `Printer::owned_prompt_writer()` (the `/dev/tty` target), while `Printer`
  synchronizes streamed output, tool output, and prompts through a shared queue.
  `InlineReply` must flush `Printer` before taking over the terminal and restore
  state afterward, matching `InlineSelect`'s use of `Printer::flush_instant()`
  and `Printer::prompt_writer()`.
- **Reedline fork patch surface.** Rerouting `cursor::position()` to the TTY
  writer has no upstream equivalent.
  Implementation must verify that it works under `| jq` and `2> err.txt`, and
  that `terminal::size()` queries the TTY file descriptor rather than stdout.
- **Reedline prompt rendering.** Reedline's `Prompt` trait has built-in
  indicators for vi mode and history search.
  `InlineReply`'s `Prompt` implementation must match JP's existing prompt style
  (such as `[c,r,s,a,?]?`).
- **Mocking `OpenEditor` in `PromptBackend::inline_reply`.** `MockPromptBackend`
  scripts a sequence of `ReplyOutcome` values; tests must verify that it
  composes with `MockEditorBackend` to cover full editor-escape loops.
- **Edit-mode keymap coverage.** Custom bindings (`Ctrl+X`, newline, `Ctrl+C`)
  must be registered in both the emacs and vi keymaps, with tests covering vi
  normal mode.

## Implementation Plan

### Phase 1: unify editor invocation under `EditorBackend`

- Add `edit_text`, `edit_file`, `EditOutcome`, and `EditRequest` to
  `EditorBackend`.
  Update `TerminalEditorBackend` to take a `duct::Expression`, extract the
  temporary-file execution logic from `jp_cli/src/editor.rs::open()`, and map a
  non-zero editor exit to `EditOutcome::Cancelled`.
- Remove `open-editor` from `jp_editor` and `jp_llm`.
  Delete the unused `ToolError::OpenEditorError` variant and export
  `EditorError` from `jp_editor` without adding a `jp_editor` dependency to
  `jp_llm`.
- Add `build_editor_backend` in `jp_cli/src/editor.rs`.
- Change `ToolPrompter` to accept `Option<Arc<dyn EditorBackend>>` instead of
  `Option<Utf8PathBuf>`, and update both construction sites in `turn_loop.rs`.
- Route `edit_query` and `jp conversation edit` through `edit_file`, keeping
  their file-management policies at the call sites.
- Update tests that construct editors via `cfg.editor.path()`.

This phase is independently reviewable, fixes path B's argument-dropping bug,
and makes no user-visible UX changes (~300 LOC).

### Phase 2: `InlineReply` widget

- Fork reedline and apply the two I/O patches (type-erased painter writer with
  `with_output`, and TTY-routed cursor probe).
- Implement `InlineReply` with the custom keybindings and `ReplyOutcome` enum,
  rendering through `Printer::owned_prompt_writer()`.
- Wire `editor.inline.edit_mode` to reedline's `Emacs` and `Vi` modes,
  registering the custom bindings in both keymaps.
- Implement a minimal `Prompt` trait implementation matching JP's prompt style.
- Add snapshot tests for keybinding behavior.

This phase is a standalone addition with no call-site changes (~250 LOC).

### Phase 3: `PromptBackend` integration and `InterruptHandler` loop

- Remove `text_input` from `PromptBackend` and disable the `editor` feature on
  `inquire`.
- Add `inline_reply` to `PromptBackend` and implement it on
  `TerminalPromptBackend` and `MockPromptBackend` (via `with_reply_outcomes`).
- Migrate `ToolPrompter` argument editing, skip reasoning, and result editing to
  `InlineReply` with `Ctrl+X` editor escalation.
  Remove the editor-presence gates from `permission_options` (`r` and `e`),
  `prompt_result_confirmation` (`e`), and `prompter.has_editor()` in
  `coordinator.rs`'s `can_prompt` check so `ResultMode::Edit` prompts on any
  TTY.
- Add `editor: Option<Arc<dyn EditorBackend>>` and `compose_in_editor` to
  `InterruptHandler`, and pass the editor through both
  `InterruptHandler::with_backend` call sites in `interrupt/signals.rs`.
- Rewrite `handle_streaming_interrupt` and `handle_tool_interrupt` as loops
  using `collect_reply` and `ReplyResult`, mapping reedline's `Signal::CtrlC` to
  `Cancelled` and a cancelled interrupt menu to `Escalated` ([RFD 045]).
- Add `interrupt.{streaming,tool_call}.compose_in_editor` to `jp_config` and
  implement the cancel and empty-submit behavior matrix, including the menu-less
  configured-action paths.
- Update existing handler tests and add tests for `Cancelled -> menu -> submit`,
  `OpenEditor -> empty -> menu`, `compose_in_editor`, and configured-action
  cancel fallbacks.

Depends on Phases 1 and 2.
Fixes path C and ships the new `r` UX (~400 LOC, mostly tests).

### Phase 4: glossary and documentation

- Add **EditorBackend** and **InlineReply** to
  `docs/architecture/ubiquitous-language.md`.
- Document `interrupt.*.compose_in_editor` and `editor.inline.edit_mode` in the
  configuration reference.
- Update user-facing documentation covering the `r` interrupt flow.

Independently reviewable after Phase 3.

## References

- [RFD 045]: Layered Interrupt Handler Stack, defining the `Ctrl+C` escalation
  model that `InlineReply` integrates with
- [RFD 048]: Four-Channel Output Model, defining the `/dev/tty` prompt
  requirement satisfied by the reedline fork
- [RFD 080]: Editor as a Config Source, which resolves *which* editor
  configuration wins rather than *how* the editor is invoked
- [reedline]: line-editor crate; JP uses a patched fork (`JeanMertz/reedline`,
  branch `changes`)
- `crates/jp_editor/src/lib.rs`: `EditorBackend` trait
- `crates/jp_inquire/src/prompt.rs`: `PromptBackend` trait, including the
  removed `text_input` method
- `crates/jp_cli/src/cmd/query/interrupt/handler.rs`: `InterruptHandler` (path
  C)
- `crates/jp_cli/src/cmd/query/tool/prompter.rs`: `ToolPrompter` construction
  (path B)
- `crates/jp_cli/src/editor.rs`: `editor::open()` and `editor::edit_query` (path
  A)
- `crates/jp_config/src/editor.rs`: `EditorConfig::command()` and `path()`
- `crates/jp_config/src/interrupt.rs`: `interrupt.{streaming,tool_call}`
  configuration (`action` ships separately; `compose_in_editor` is added here)

[RFD 045]: 045-layered-interrupt-handler-stack.md
[RFD 048]: 048-four-channel-output-model.md
[RFD 080]: 080-editor-as-a-config-source.md
[RFD 092]: 092-predictable-and-responsive-interrupt-escalation.md
[RFD 093]: 093-inline-first-query-composition.md
[cmd-cfg]: ../wiki/00dxybd-ubiquitous-language.md#commandconfig
[reedline]: https://crates.io/crates/reedline
