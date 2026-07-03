# RFD 088: Unified Editor Service and Inline Reply Widget

- **Status**: Accepted
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-05-08
- **Requires**: [RFD 045]
- **Extends**: [RFD 048]
- **Extended by**: [RFD 093]

## Summary

Consolidate JP's four independent editor-invocation paths onto a single
`EditorBackend` trait — with `edit_text` (string in/out) and `edit_file`
(path-based) methods — that fully respects `EditorConfig`, and replace the
interrupt-menu reply prompt with a richer inline editing widget, built on a
vendored copy of [reedline], that accepts short replies inline, supports
multi-line input, and escalates to the configured editor on demand.
A per-context `compose_in_editor` setting (`false`/`true`/`"always"`/`"never"`)
chooses where composition happens, from inline-only to editor-only.

## Motivation

JP today opens an external editor through four separate code paths, each with
different fidelity to the user's `EditorConfig`:

| Path                           | Where                                          | Accepts                                          | Fidelity                                                |
| ------------------------------ | ---------------------------------------------- | ------------------------------------------------ | ------------------------------------------------------- |
| **A. Duct-based, file-based**  | `jp_cli/src/editor.rs::open()`                 | full `duct::Expression` from `command()`         | full                                                    |
| **B. `open-editor`-based**     | `jp_editor::TerminalEditorBackend` (tool args, | single `Utf8PathBuf` from `EditorConfig::path()` | partial — args dropped by the `Utf8PathBuf` return type |
|                                | skip-reason, result edit in `prompter.rs`)     |                                                  |                                                         |
| **C. `inquire::Editor`-based** | `TerminalPromptBackend::text_input` (the       | nothing — reads `EDITOR` / `VISUAL` directly     | none — ignores `editor.cmd`, `editor.envs`, `JP_EDITOR` |
|                                | `Reply:` prompt in streaming/tool interrupts)  |                                                  |                                                         |
| **D. Duct-based, file-based**  | `jp conversation edit`                         | full `duct::Expression` from `command()`         | full — already correct, but unnamed by the original     |
|                                | (`cmd/conversation/edit.rs`)                   |                                                  | "three paths" framing                                   |

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
Four call sites each filled the gap differently — paths A and D already honor
`EditorConfig::command()`, paths B and C do not.
The fix is to introduce a first-class `EditorBackend` and migrate the call sites
onto it.

The interrupt-menu reply UX is also weak independently of the bug.
Today's `r` flow goes straight into `inquire::Editor`'s "press `e` to edit,
`Enter` to submit" two-step prompt, which forces the editor for every reply,
however short.
A first-class inline editor — with a rich editing experience and a `Ctrl+X`
escape hatch (inspired by readline's `edit-and-execute-command`) — covers both
short replies and long ones without forcing a process-spawn for the trivial
case.

## Design

### User-facing behavior

#### `r` (Reply) in the streaming-interrupt menu

When the user presses `Ctrl+C` during streaming and chooses `r`:

1. An inline reply prompt appears with the cursor inside an editable buffer.
2. The user types a reply directly.
   Standard line-editing keybindings work (Ctrl+A/E for line nav, Ctrl+W to kill
   word, arrow keys, Ctrl+L to clear, word movement, kill-ring, etc.).
3. **Enter** submits the buffer; an empty buffer follows the call site's
   empty-Enter policy (for streaming `r`, returns to the menu — see
   [Empty-Enter policy](#empty-enter-policy)).
4. **Shift+Enter** (on terminals that speak the kitty keyboard protocol —
   kitty, Ghostty, WezTerm, foot) or **Alt+Enter** (portable fallback) insert a
   newline.
   The fallback is advertised in the help line.
5. **Ctrl+X** opens the configured editor seeded with the current buffer
   contents.
6. **Ctrl+C** returns the user to the interrupt menu without sending anything.
   A second **Ctrl+C** at the menu escalates to graceful shutdown (see [RFD
   045]).

After the editor closes (when invoked via `Ctrl+X`), the inline reply prompt
always re-appears — the editor is never a terminal action:

- A non-empty result re-seeds the buffer; the user presses Enter to send.
- An empty result (or an aborted editor) re-appears with an empty buffer; the
  user can type again, submit empty, or `Ctrl+C` back to the menu.

**Opt-in: straight to the editor.** Setting
`interrupt.streaming.compose_in_editor = true` skips the inline widget for `r`
and opens the configured editor immediately, seeded empty.
A non-empty result is sent as-is; an empty or cancelled result drops into the
inline prompt (where the user can type, submit empty, or `Ctrl+C` back to the
menu).
This serves users who always want a full editor for replies and would rather not
pass through the inline step.

#### `r` (Stop & respond) in the tool-interrupt menu

Same flow as the streaming `r`, with one difference at the inline prompt: an
empty Enter does not return to the menu — it stops the tool with the
`DEFAULT_TOOL_CANCELLED_RESPONSE` canned message, preserving the "interrupt a
tool with no explanation" shortcut.
`Ctrl+C` still backs out to the menu (a second `Ctrl+C` there escalates), and
the `Ctrl+X` editor escape still always returns to the inline prompt.

The `interrupt.tool_call.compose_in_editor` opt-in applies here too: when set,
`r` opens the editor directly; an empty or cancelled editor result drops into
the inline prompt (where an empty submit then sends the canned message).

#### `jp query` (initial prompt)

Unchanged behavior.
The user's first prompt opens directly in the configured editor, and an empty
save cancels the query — the existing `editor::edit_query` flow already uses
`EditorConfig::command()` correctly.

#### Tool argument editing, skip reasoning, result editing

All three currently route through `ToolPrompter` via `TerminalEditorBackend`,
each gated on a configured editor at its own menu/prompt site:
`permission_options` (`r`/`e`), `prompt_result_confirmation` (the `e` result
option), and the coordinator's `can_prompt` check for `ResultMode::Edit`.
After this RFD all three become `InlineReply` widgets seeded with the current
text — the JSON arguments, the skip-reason placeholder, the tool result — with
`Ctrl+X` escaping to the configured editor on demand.

Because `InlineReply` needs only a tty, the permission-menu options `r` ("Skip
and reply") and `e` ("Edit arguments") are no longer gated on a configured
editor; they appear whenever a prompt can be shown, and only the `Ctrl+X` escape
requires `editor.command`.
The result-delivery confirmation is un-gated the same way: "Edit result first"
(`e`) appears whenever a tty is present, and `ResultMode::Edit` prompts on any
tty rather than requiring an editor.
In every case `Ctrl+X` with no editor configured is a no-op — the inline widget
stays open.
The `JP_EDITOR="subl -w"` arg-drop bug closes as a side effect of routing that
escape through `EditorConfig::command()`.

The argument editor keeps its parse-error re-prompt loop: on invalid JSON the
caller re-seeds the `InlineReply` buffer with the user's text and re-prompts,
surfacing the error in the prompt line — no process re-spawn, edits preserved,
and the old "Re-open editor?
y/n" confirmation step drops out.
`Ctrl+X` still escapes to the full editor for a large rewrite, and its result
re-validates through the same loop.
An emptied buffer abandons the edit and falls back to the Ask prompt with the
arguments unchanged — it never submits empty JSON or runs the tool with cleared
arguments.

### Architecture

#### `EditorBackend`: the frontend seam for editor invocation

Two invariants govern every editor open in JP:

1. **Any** string-in/string-out edit goes through `EditorBackend`.
2. **Any** editor spawned by a *command-spawning* frontend (terminal, native
   app) resolves its invocation through `EditorConfig::command()` and shared
   open/wait logic.

`EditorBackend` gains a second method so it covers both editing shapes a
frontend offers:

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

`edit_text` returns `(EditOutcome, String)`; on `Cancelled` the string is
meaningless and callers ignore it.
An `Err` (`EditorError::Spawn` / `Io`) is distinct from `Cancelled`: it means
the editor never ran or file I/O failed, so callers surface it and recover (fall
back to the inline widget, keep the typed buffer) rather than treating it as a
user cancellation.
Because `editor.cmd` defaults to `shell = false` (direct spawn), a missing
editor binary lands here as a spawn error rather than a shell's non-zero exit.
Whether the content was *modified*, *unchanged*, or *emptied* is a content-delta
question the caller answers inline (`new == old`, `new.trim().is_empty()`) —
not the backend's concern, and a `classify` helper that only wraps `==` is not
worth the indirection.
The terminal backend maps a **non-zero editor exit to `Cancelled`**, matching
git's commit-message convention; a true spawn failure (binary not found) is an
`Err(EditorError)`.

Backends are named by *frontend environment*, not storage medium:

- `TerminalEditorBackend` — tempfile + `duct` spawn via
  `EditorConfig::command()` (the only implementation this RFD ships).
- `WebEditorBackend`, `NativeEditorBackend` — future frontends (per the web-UI
  and native-UI RFDs).
  A web frontend mediates file editing through its server; a native frontend
  spawns the application on the path.
  Both implement the same two methods; only the *command-spawning* ones go
  through `EditorConfig::command()`.
- `MockEditorBackend` — scripts outcomes for tests, including end-to-end flows
  that exercise the editor escape without spawning anything.

"No editor available" stays modeled as `Option<Arc<dyn EditorBackend>>`
(`None`); there is no `Null` backend.

The `TerminalEditorBackend`'s tempfile-and-run dance is the logic
`jp_cli/src/editor.rs::open()` performs today; we extract it into `jp_editor` so
there is one implementation.

`open-editor` is dropped from both `jp_editor` and `jp_llm`.
The dead `ToolError::OpenEditorError` variant in `jp_llm/src/error.rs` is
**deleted** (it is constructed nowhere); editor failures live in
`jp_cli`/`jp_editor` as a generic `EditorError`.
`jp_llm` gains no dependency on `jp_editor` — the LLM crate does not know a
human editor exists.

#### One construction helper

A single free function `build_editor_backend(&cfg.editor)` in
`jp_cli/src/editor.rs` (it calls `EditorConfig::command()` and wraps the result
in a `TerminalEditorBackend`) is the *only* way any context obtains an editor,
so the call sites can't drift:

- **Query startup** (`cmd/query.rs`) builds it for `edit_query`.
- **The turn loop** (`turn_loop.rs`) builds it once per turn and shares the same
  `Arc<dyn EditorBackend>` with `ToolPrompter` and `InterruptHandler`.
- **`jp conversation edit`** (`cmd/conversation/edit.rs`) builds it for its
  `edit_file` call.

The turn-loop site:

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

Promoting the helper out of `jp_cli` (into `jp_editor` or `jp_config`) is
deferred (YAGNI) — three call sites in one crate don't justify the move yet.

#### File-based flows route through `edit_file`

`jp_cli::editor::edit_query` and `jp conversation edit` (path D) keep their
bespoke *surrounding* policy at the call site — `edit_query`'s persistent
`QUERY_MESSAGE.md`, `RevertFileGuard`, and TOML preamble; `conversation edit`'s
validate-and-revert and projection sync.
That policy is the caller's domain and does not belong on the trait.

The editor open in both now goes through `edit_file`, so they share one
open/wait path with the rest of JP and become frontend-polymorphic for free when
the web/native backends land.
The `EditRequest` carries the working directory (`edit_query` opens with `cwd =
conversation_root`, as it does today); frontends that don't spawn a local
process ignore it.
`edit_file` only promises "open the editor on these path(s) in this directory,
block until done"; the caller reads the files back and applies its own
validation, revert, and content checks.

`edit_file` reports interaction via `EditOutcome`, and each file caller acts on
`Cancelled` itself: `edit_query` cancels the query (sends nothing);
`conversation edit` restores its pre-edit snapshots and reports — the same
effect its non-zero exit path has today.

#### `InlineReply` widget in `jp_inquire`

A new widget alongside `InlineSelect`, built on a vendored copy of [reedline]
(see [Terminal ownership](#terminal-ownership-and-the-vendored-reedline) below):

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
    pub fn prompt(&self, writer: &mut dyn Write) -> Result<ReplyOutcome, InquireError>;
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

Reedline provides the rich editing experience out of the box: emacs-style
keybindings, multi-line input, cursor navigation, kill-ring, undo, word
movement, plus the unicode-width / line-wrap / paste / resize handling that is
expensive to reimplement correctly.
JP keeps every familiar default binding, including the Meta/Alt-based ones
(`Meta+B/F/D`, `Meta+Backspace`, …).
Custom bindings layered on top:

| Key         | Action           | Mechanism                                                    |
| ----------- | ---------------- | ------------------------------------------------------------ |
| Enter       | submit (default) | reedline default                                             |
| Shift+Enter | newline          | `EditCommand::InsertNewline` (kitty protocol; incl. Ghostty) |
| Alt+Enter   | newline          | same edit command (portable fallback)                        |
| Ctrl+X      | open editor      | custom `ReedlineEvent` that returns `OpenEditor`             |
| Ctrl+C      | cancel           | reedline `Signal::CtrlC`, surfaced as `Cancelled`            |

`Ctrl+C` is the single, mode-independent cancel: in raw mode it arrives as byte
`0x03`, reedline returns `Signal::CtrlC`, and the widget maps it to `Cancelled`.
We deliberately do **not** bind `Esc` (the Meta prefix in emacs mode, the
insert→normal toggle in vi mode) or `Ctrl+Q` (intercepted by some terminal
emulators).

The widget enables `KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES` during
the prompt so `Shift+Enter` works on kitty-protocol terminals (kitty, Ghostty,
WezTerm, foot); `Alt+Enter` covers the rest and is advertised in the help line.

**Edit mode is configurable.** Reedline ships `Emacs` (default) and `Vi` edit
modes, selected via `editor.inline.edit_mode` (see [Configuration
changes](#configuration-changes)).
The custom bindings above are registered into each mode's keymap separately,
since reedline keeps per-mode keybinding tables; in vi mode JP also enables
`with_cursor_config` so the cursor shape tracks insert/normal.

The widget itself does **not** open the editor.
It signals intent via `OpenEditor`; the caller (the `InterruptHandler`, or
`ToolPrompter`) owns the editor decision.
This keeps `jp_inquire` free of editor concerns.

#### Terminal ownership and the vendored reedline

RFD 048 (Four-Channel Output Model, Implemented) requires interactive prompts to
render on `/dev/tty`, so they survive `jp query | jq` and `jp query 2> err.txt`.
Reedline does not meet this against the published crate:

- **Output.** Reedline's painter is hardcoded to **stderr** (`pub type W =
  BufWriter<Stderr>`, built in `Reedline::create()`).
  Rendering to stderr breaks `2> err.txt` — the prompt is swallowed.
- **Cursor probing.** Reedline calls `crossterm::cursor::position()` during
  prompt init and resize, and crossterm hardcodes that `ESC[6n` query to process
  **stdout** (`cursor/sys/unix.rs`).
  Under `| jq` that contaminates the data stream, and the query does not
  round-trip when stdout is redirected.
- **Input is fine.** Reedline reads via crossterm's event source, whose
  `tty_fd()` uses stdin when it is a TTY and otherwise opens `/dev/tty`; raw
  mode uses the same fd.
  Input already respects `/dev/tty`.

Closing the output gap needs an upstream reedline change; closing the cursor
probe needs an upstream *crossterm* change (or for reedline to stop calling
`position()`) — two upstreams, one foundational.
Rather than gate this RFD on them, JP **vendors reedline** at
`crates/contrib/reedline` and carries two local patches, both reachable now that
the call sites are ours:

1. Type-erase the painter writer (`W = BufWriter<Box<dyn Write + Send>>`) and
   add `Reedline::with_output(...)`, pointed at the same `/dev/tty` writer
   `Printer` uses for its `Tty` target (`Printer::prompt_writer()`).
2. Replace reedline's two `cursor::position()` calls with a helper that writes
   the `ESC[6n` query to that tty writer and reads the reply back through the
   (already `/dev/tty`) event source.

The `with_output` patch is clean and should be upstreamed, shrinking the
standing local delta to the cursor-probe change.
The widget drains `Printer` before taking over the terminal and restores after,
as `InlineSelect` does today.

#### `PromptBackend::inline_reply`

Replaces the removed `text_input` method:

```rust
pub trait PromptBackend: Send + Sync {
    // ... existing methods ...

    fn inline_reply(
        &self,
        message: &str,
        initial_text: &str,
        edit_mode: ReplyEditMode,
        editor_escape: bool, // `false` unwires the `Ctrl+X` editor escape
        writer: &mut dyn Write,
    ) -> Result<ReplyOutcome, InquireError>;
}
```

Like the other `PromptBackend` methods, `inline_reply` is **writer-aware**: the
`jp_cli` call site passes `printer.prompt_writer()` (the `/dev/tty` target), and
`TerminalPromptBackend` feeds that writer to the vendored reedline's
`with_output`.
`jp_inquire` stays writer-agnostic and gains no `jp_printer` dependency — the
RFD 048 writer-passing boundary is preserved.

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
than an `Option`, so prompt errors and `Ctrl+C` are handled explicitly instead
of being swallowed by `.ok()?` — the regression [RFD 045] warns about.
The `writer` (the `/dev/tty` prompt target) is threaded from the menu loop into
`collect_reply` and on to `inline_reply`, matching the writer-passing pattern of
the other `PromptBackend` methods.
The tool-interrupt `r` flow uses the same helper but substitutes
`DEFAULT_TOOL_CANCELLED_RESPONSE` for an empty/cancelled reply, preserving
today's canned-message semantics.
`compose_in_editor` is the `ComposeInEditor` enum (`false`/`true`/`"always"`/
`"never"`); `starts_in_editor()` and `editor_escape()` select the path above,
and `editor_unavailable` applies the `true`-vs-`"always"` failure fallback.

This sketch is the `action = prompt` path.
The `[interrupt].*.action` config wraps it: a non-`prompt` action skips the menu
and runs directly, and a configured `reply` / `respond` calls `collect_reply`
directly — the cancel fallback for those menu-less paths is pinned in
[Configuration changes](#configuration-changes), not left to implementation.
The `compose_in_editor` argument is read from the same per-context config
struct.

### Empty-Enter policy

The widget always returns `Submit(text)` on Enter — empty or not.
Per-call-site policy:

| Call site                            | Empty-text policy                                                |
| ------------------------------------ | ---------------------------------------------------------------- |
| Streaming `r`                        | Empty → `Cancelled` (back to menu)                               |
| Tool `r`                             | Empty → fall through to canned `DEFAULT_TOOL_CANCELLED_RESPONSE` |
| Tool permission `r` (skip reason)    | Empty → `None` (skip with no reason)                             |
| Tool permission `e` (edit arguments) | Empty → fall back to Ask (args unchanged)                        |
| Tool result edit                     | Empty → `None` (fall back to Ask)                                |

Keeping the policy out of the widget matches the project's "code where it
belongs" principle — the meaning of "empty" is the caller's domain.

### Configuration changes

Two keys are added under `interrupt.{streaming,tool_call}` and one under
`editor.inline`:

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

- **`editor.cmd`** — accepts a string (`cmd = "code --wait"`) or a table (`cmd
  = { program = "code", args = ["--wait"], shell = false }`), reusing the
  `CommandConfig` shape already used by local tools.
  A string (and `shell = false`, the default) runs the program **directly**, so
  the edited path is a real argument and a missing editor surfaces as a spawn
  error rather than a silent non-zero exit.
  `shell = false` is the cross-platform form and the recommended choice on
  Windows.
  Set `shell = true` for pipes, `&&`, or subshells; on Unix the edited path(s)
  are forwarded to the shell command via `"$@"`.
  On non-Unix platforms `shell = true` is unsupported: it is logged and the
  program is spawned **directly** (the `"$@"` path-forwarding convention is
  Unix-only) — wrap any shell logic in a script and point `program` at it
  (`shell = false`) instead.

- **`compose_in_editor`** — where the reply/response is composed; a spectrum
  from inline-only to editor-only.
  Accepts a bool or a string:

  - `false` *(default)* — start in the inline widget; `Ctrl+X` escapes to
    `editor.cmd` on demand.
  - `true` — open `editor.cmd` directly; if it can't open, fall back to the
    inline widget (with a chrome notice).
  - `"always"` — open `editor.cmd` directly; if it can't open, return to the
    menu — **never** the inline widget (for users who can't use it).
    A chrome notice surfaces the failure.
  - `"never"` — inline widget only; the `Ctrl+X` editor escape is unwired and
    unadvertised (for users who don't want the editor launched).

  In every editor-first mode (`true` / `"always"`), a non-empty saved result is
  sent and an empty or cancelled editor returns to the menu (the user bailed).
  In non-interactive / no-tty mode there is no prompt, so the key has no effect.

- **`editor.inline.edit_mode`** — selects reedline's edit mode for the inline
  widget: the *editing style* of the inline buffer, orthogonal to which external
  editor `Ctrl+X` opens (that is `editor.command`).

**Cancel / empty behavior matrix.** Defined for every context, including the
menu-less configured-action paths, so nothing is left to implementation:

| Context                  | Menu? | Empty Enter         | Ctrl+C                        |
| ------------------------ | ----- | ------------------- | ----------------------------- |
| streaming `r`            | yes   | back to menu        | back to menu (2nd → escalate) |
| streaming `action=reply` | no    | resume the response | resume the response           |
| tool `r`                 | yes   | canned message      | back to menu (2nd → escalate) |
| tool `action=respond`    | no    | canned message      | canned message                |

"Escalate" is RFD 045's graceful shutdown.
The menu-less configured-action rows have no menu to return to, so cancel
resolves to the context's natural fallback.

`Ctrl+C` inside `InlineReply` is a *local* reply cancellation (back to the
menu), not RFD 045 prompt escalation; only `Ctrl+C` at the interrupt menu
escalates.
The reply widget sits one level below the menu, so its cancel pops up one level,
and a second cancel at the menu escalates.

### Ubiquitous-language additions

Two terms enter the glossary:

- **EditorBackend** — the frontend seam for invoking the user's configured
  editor, with `edit_text` (string in/out) and `edit_file` (path-based) methods.
  Each frontend (terminal, web, native, mock) is one implementation.
- **InlineReply** — the `jp_inquire` widget for short replies in interrupt
  menus; supports inline typing with a `Ctrl+X` escape to the `EditorBackend`.

## Drawbacks

- **Vendored reedline.** JP carries a patched copy of reedline at
  `crates/contrib/reedline` (see [Terminal
  ownership](#terminal-ownership-and-the-vendored-reedline)), a maintenance line
  item: tracking upstream releases and rebasing the two local patches (Lehman's
  Law).
  The `with_output` patch is intended for upstream, to shrink the standing delta
  to the cursor-probe change.
  The vendored crate pulls in `nu-ansi-term`, `unicode-segmentation`, and
  `unicode-width`; `crossterm`, `serde`, and `strip-ansi-escapes` are already in
  the tree.
- **Writer threading into reedline.** `PromptBackend::inline_reply` stays
  writer-aware like its siblings (`&mut dyn Write`), but the vendored reedline
  wants to own its output.
  The `with_output` patch is shaped to render to the borrowed
  `Printer::prompt_writer()` the call site passes, so `jp_inquire` never depends
  on `jp_printer` and the RFD 048 writer-passing boundary is preserved.
- **More code in `jp_editor`.** Replacing the `open-editor` one-liner with a
  duct-based tempfile dance is 50–80 LOC of real terminal-process plumbing
  (Tesler's Law: the complexity has to live somewhere; the right somewhere is
  here).
- **Behavior change for the `r` flow.** By default `r` opens the inline reply
  widget instead of the `inquire::Editor` "press `e`" two-step.
  (The old flow never went *straight* to the editor either — it always showed
  that intermediate prompt; `compose_in_editor` is what makes straight-to-editor
  possible for the first time.)
  JP is pre-release, so the default change is acceptable; flagged for
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

Hand-roll a multi-line editor in `jp_inquire`, writing through
`Printer::prompt_writer()` directly (which would satisfy RFD 048 without any
vendoring).
**Rejected.** Reedline (used by nushell) has solved the expensive, quirky parts
— unicode width, line wrapping, multi-line cursor navigation, bracketed paste,
kitty-protocol disambiguation, resize, undo, kill-ring, and pluggable emacs/vi
edit modes.
A naive clone would hit those head-first.
Vendoring and patching reedline's two I/O seams (output writer, cursor probe) is
a far smaller and safer investment than reimplementing that surface.
The `/dev/tty` requirement does not force a hand-roll, because the vendored copy
renders through `Printer`'s tty writer.

## Non-Goals

- **Frontend backends beyond terminal.** Only `TerminalEditorBackend` ships
  here.
  `WebEditorBackend` / `NativeEditorBackend` are designed-for on the trait but
  implemented by their own RFDs when the web/native UIs land.
- **Backward compatibility for the `r` flow.** Pre-release; UX changes are fair
  game.
- **Editor selection at runtime.** This RFD does not introduce per-context
  editor *selection* (e.g., a different external editor for inline replies vs.
  the query prompt).
  One external editor is configured; one is used.
  (`compose_in_editor` controls *whether* a reply uses that editor, and
  `editor.inline.edit_mode` controls the *inline widget's* editing style — both
  separate axes from *which* external editor opens, so neither conflicts with
  this non-goal.)
- **Arrow-key UX inside `InlineSelect`** or other existing widgets.
  Out of scope.

## Risks and Open Questions

- **Vendored-reedline / `Printer` coordination.** The vendored reedline renders
  through `Printer::prompt_writer()` (the `/dev/tty` target), but JP's `Printer`
  still synchronizes streamed output, tool renderings, and prompt output through
  a shared queue.
  The widget must drain `Printer` before taking over the terminal and restore
  cleanly after, as `InlineSelect` does today via `Printer::flush_instant()` /
  `Printer::prompt_writer()`.
  Validate during implementation.
- **Vendored-reedline patch surface.** The cursor-probe patch (rerouting
  `cursor::position()` to the tty writer) has no upstream equivalent yet; verify
  it behaves under `| jq` and `2> err.txt`, and that `terminal::size()` reads
  the tty fd rather than stdout.
- **Reedline's prompt rendering.** Reedline's `Prompt` trait is opinionated
  about how the prompt prefix renders (with built-in indicators for vi mode,
  history search, etc.).
  The widget's `Prompt` impl must match JP's existing prompt style (the
  `[c,r,s,a,?]?`-style line).
  Likely doable in 30 LOC but worth a spike.
- **`PromptBackend::inline_reply` returning `OpenEditor` from a mock.** Tests
  need to script editor-escape flows.
  The mock implementation is straightforward — script a vector of
  `ReplyOutcome` values — but verify it composes cleanly with the existing
  `MockEditorBackend` for full end-to-end tests of the loop.
- **Edit-mode keymap coverage.** The custom bindings (`Ctrl+X`, newline,
  `Ctrl+C`) must be registered into both the emacs and vi keymaps, and the vi
  normal-mode path tested.
  (The cancel-semantics question for menu-less configured actions is resolved in
  [Configuration changes](#configuration-changes), not left open here.)

## Implementation Plan

### Phase 1: structural — `EditorBackend` becomes canonical

- Add `edit_text` and `edit_file` to `EditorBackend`, plus the `EditOutcome` and
  `EditRequest` types.
  Re-shape `TerminalEditorBackend` around `duct::Expression`, extracting the
  tempfile-and-run dance from `jp_cli/src/editor.rs::open()`; map a non-zero
  editor exit to `EditOutcome::Cancelled`.
- Drop `open-editor` from `jp_editor` and `jp_llm`.
  **Delete** the dead `ToolError::OpenEditorError` variant; export a generic
  `EditorError` from `jp_editor`.
  `jp_llm` gains no `jp_editor` dependency.
- Add `build_editor_backend` helper in `jp_cli/src/editor.rs`.
- Update `ToolPrompter` to receive `Option<Arc<dyn EditorBackend>>` instead of
  `Option<Utf8PathBuf>`; update both `turn_loop.rs` construction sites.
- Route `edit_query` and `jp conversation edit` through `edit_file`, keeping
  their surrounding policy at the call site.
- Update tests using `cfg.editor.path()` style construction.

Reviewable independently.
Closes path B's arg-drop bug as a side effect; no user-visible UX change yet.

Estimated diff: ~300 LOC.

### Phase 2: `InlineReply` widget

- Vendor reedline at `crates/contrib/reedline`; apply the two I/O patches
  (type-erased painter writer + `with_output`; tty-routed cursor probe).
- Implement `InlineReply` on the vendored reedline with the keybindings and
  `ReplyOutcome` enum described above, rendering through
  `Printer::prompt_writer()`.
- Wire `editor.inline.edit_mode` to reedline's `Emacs`/`Vi` modes, registering
  the custom bindings into each keymap.
- Implement a minimal `Prompt` impl that matches JP's prompt-line style.
- Snapshot tests for keybinding behavior using reedline's testable input stream
  (or a thin shim).

Reviewable independently.
No call-site changes yet — pure addition.

Estimated diff: ~250 LOC.

### Phase 3: `PromptBackend` integration and `InterruptHandler` rewiring

- Remove `text_input` from `PromptBackend`.
  Drop the `editor` feature on `inquire`.
- Add `inline_reply` to `PromptBackend` and update `TerminalPromptBackend` and
  `MockPromptBackend` (with `with_reply_outcomes`).
- Migrate the `ToolPrompter` argument, skip-reasoning, and result edits to
  `InlineReply` (seeded text + `Ctrl+X` escape).
  Un-gate all three editor-dependence points so only the escape needs
  `editor.command`: `permission_options` (`r`/`e`), the `e` option in
  `prompt_result_confirmation`, and the `prompter.has_editor()` term in
  `coordinator.rs`'s `can_prompt` gate (so `ResultMode::Edit` prompts on any
  tty).
- Add `editor: Option<Arc<dyn EditorBackend>>` and `compose_in_editor` to
  `InterruptHandler`; thread the editor through both
  `InterruptHandler::with_backend` call sites in `interrupt/signals.rs`.
- Rewrite `handle_streaming_interrupt` and `handle_tool_interrupt` as loops with
  `collect_reply` returning `ReplyResult`; map reedline `Signal::CtrlC` to
  `Cancelled` and a cancelled *menu* to RFD 045's `Escalated`.
- Add `interrupt.{streaming,tool_call}.compose_in_editor` to `jp_config` and
  apply the cancel/empty behavior matrix (incl. the menu-less configured-action
  rows).
- Update existing handler tests; add tests for `Cancelled → menu → submit`,
  `OpenEditor → empty → menu`, the `compose_in_editor` straight-to-editor
  path, and the configured-action cancel fallbacks.

Depends on Phases 1 and 2.
Closes path C. Ships the new `r` UX.

Estimated diff: ~400 LOC, mostly tests.

### Phase 4: glossary and docs

- Add **EditorBackend** and **InlineReply** to
  `docs/architecture/ubiquitous-language.md`.
- Document `interrupt.*.compose_in_editor` and `editor.inline.edit_mode` in the
  config reference.
- Update any user-facing docs that describe the `r` flow.

Reviewable independently after Phase 3.

## References

- [RFD 045]: Layered Interrupt Handler Stack — the Ctrl+C escalation direction
  the inline reply hooks into
- [RFD 048]: Four-Channel Output Model — the `/dev/tty` requirement the
  vendored reedline must satisfy
- [RFD 080]: Editor as a Config Source — orthogonal concern; resolves *which*
  editor config wins, not *how* the editor is invoked
- [reedline] — line-editor crate, vendored at `crates/contrib/reedline`
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
- `crates/jp_config/src/interrupt.rs` — the `interrupt.{streaming,tool_call}`
  config; the `action` field ships separately, `compose_in_editor` is added here

[RFD 045]: 045-layered-interrupt-handler-stack.md
[RFD 048]: 048-four-channel-output-model.md
[RFD 080]: 080-editor-as-a-config-source.md
[RFD 093]: 093-inline-first-query-composition.md
[cmd-cfg]: ../architecture/ubiquitous-language.md#commandconfig
[reedline]: https://crates.io/crates/reedline
