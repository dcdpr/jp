# RFD 093: Inline-First Query Composition

- **Status**: Discussion
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-07-02
- **Extends**: [RFD 088]
- **Summary**: Composes bare `jp query` prompts inline by default, with `Ctrl+X`
  editor escalation and configurable `query.compose_in_editor` modes.

## Summary

A bare `jp query` composes the query in the inline editor (the reedline-based
widget from [RFD 088]) instead of opening the external editor.
`Ctrl+X` opens the full query document in the external editor, and a leftover
query draft seeds whichever editor opens.
A new `query.compose_in_editor` setting selects the editor using the existing
`ComposeInEditor` options, and `InlineReply` is renamed to `InlineEditor` to
match the user-facing config vocabulary.

## Motivation

`jp query` has five entry paths today, and none of them use the inline editor:

| Invocation          | Behavior                                                  |
| ------------------- | --------------------------------------------------------- |
| `jp q` (bare, tty)  | Opens the external editor with the full query document.   |
|                     | Errors with `MissingEditor` if no editor resolves.        |
| `jp q <query>`      | Sends directly.                                           |
| `echo x \| jp q`    | Sends directly.                                           |
| `jp q [<query>] -e` | Opens the external editor, seeded with `<query>`.         |
| `jp q -E`           | No editing; replays the last request or sends `continue`. |

This has three problems:

1. **A bare `jp q` requires a configured external editor.** The inline editor
   works on any tty without configuration.
   Requiring `$EDITOR` for the zero-argument form of `jp query` adds an
   unnecessary setup step.
2. **Short queries pay for a full editor round-trip.** Spawning an external
   editor to write and save a single line is slow for the most common
   interaction.
3. **Query composition lacks `compose_in_editor` configuration.** [RFD 088]
   added `compose_in_editor` modes (inline-first, editor-first, editor-only,
   inline-only) for interrupt replies and tool responses, leaving the initial
   query as the only frequent composition path that still requires the external
   editor.

## Design

### Behavior

| Invocation                          | New behavior                                             |
| ----------------------------------- | -------------------------------------------------------- |
| `jp q` (tty)                        | Opens the inline editor. `Ctrl+X` opens the full query   |
|                                     | document in the external editor, seeded with the buffer. |
|                                     | A cancelled or empty external editor returns to the      |
|                                     | inline editor with the buffer intact. `Ctrl+C` aborts.   |
| `jp q` + `compose_in_editor = true` | Opens the external editor first, falling back to the     |
|                                     | inline editor if spawning fails.                         |
| `... = "always"`                    | Opens the external editor or errors; never uses the      |
|                                     | inline editor.                                           |
| `... = "never"`                     | Opens the inline editor only; `Ctrl+X` is unbound.       |
| `jp q [<query>] -e`                 | Forces composition and prefers the external editor,      |
|                                     | seeded with `<query>` when given. Preserves `"always"`   |
|                                     | (no inline fallback on spawn failure).                   |
| `jp q` (tty), empty submit          | Submitting an empty or whitespace-only buffer in the     |
|                                     | inline editor aborts with the existing "Query is empty,  |
|                                     | ignoring." notice and keeps the query draft.             |
| `jp q --quote`                      | Forces composition; the editor surface follows           |
|                                     | `query.compose_in_editor`.                               |
| `jp q <query>`, piped stdin, `-E`   | Unchanged: sends directly or skips composition.          |
| No tty                              | Unchanged: `query.compose_in_editor` has no effect       |
|                                     | without `/dev/tty` (external editor or `MissingEditor`). |

With this default, a bare `jp q` requires no editor configuration, and one-line
queries avoid an external editor round-trip while `Ctrl+X` keeps the full query
document one keystroke away.

Empty submission is intentionally asymmetric across the two editors.
Exiting the external editor with an empty document returns to the inline editor
so text already typed in the inline buffer is not lost.
Submitting an empty buffer in the inline editor has nothing to preserve and ends
the invocation, matching the existing empty-query behavior.

### Configuration

```toml
[query]
compose_in_editor = false # false | true | "always" | "never"
```

A new top-level `[query]` section configures query composition, matching how
`[interrupt]` configures query interrupts.
The value type is the existing `ComposeInEditor` enum, which moves from
`jp_config::interrupt` to `jp_config::editor` because both `[interrupt]` and
`[query]` use it.
Moving the enum does not change the serialized format.

Fallback behavior depends on the caller: interrupts fall back to the interrupt
menu, whereas query composition has no menu.
`ComposeInEditor`'s doc comment becomes context-neutral, and each config field
documents its own fallback behavior.
For `query.compose_in_editor`:

- `false` (default): opens the inline editor first; `Ctrl+X` opens the external
  editor.
- `true`: opens the external editor first, falling back to the inline editor if
  the external editor cannot open.
- `"always"`: opens the external editor only, aborting with an error if it
  cannot open.
- `"never"`: opens the inline editor only and disables `Ctrl+X`.

Without `/dev/tty`, `query.compose_in_editor` has no effect and composition uses
the external editor (or errors with `MissingEditor` if no editor resolves),
matching `interrupt.*.compose_in_editor` in non-interactive mode.

`-e`/`--edit`, `-E`/`--no-edit`, and `--quote` remain outside `AppConfig`.
`apply_cli_config` ignores them, so they are not recorded in the conversation's
config delta like `--cfg query.compose_in_editor=...`.
Instead, `compose_query` applies them after config resolution and delta
recording.
These flags control whether composition runs (`-e` and `--quote` force it; `-E`
skips it).
`-e` also prefers the external editor for the current invocation: `false` and
`"never"` behave as `true`, while `true` and `"always"` stay unchanged so `-e`
does not re-enable inline fallback when `"always"` is set.

### Query draft lifecycle

A leftover `QUERY_MESSAGE.md` draft participates in composition on both editor
surfaces.
Because a draft is kept after an empty query, an interrupt, or a failed turn,
its pre-filled config preamble can outlive the invocation that wrote it.
If a later invocation resolves a different config, unmodified values in the kept
preamble are stale defaults rather than user edits.

To distinguish user edits from defaults, the draft records its initial preamble
values as a *config seed* in a hidden `<!-- CONFIG_SEED: ... -->` comment below
the preamble.
Before either editor reads the preamble, JP reconciles it with the current
invocation's config ([RFD 080]):

- Pre-filled fields that the user did not edit take the current invocation's
  value.
- Edited fields are kept as written.
- The config seed is updated to match the current invocation.
- If the draft has no config seed, every pre-filled field takes the current
  invocation's value because user edits cannot be distinguished from defaults.
- If the preamble is invalid TOML, it is left untouched with its existing config
  seed until the syntax error is repaired (see step 3).

After reconciliation, both editors run the same config pipeline: parse the
`QueryDocument` structure, parse the reconciled TOML preamble, resolve model
aliases, compute the notice delta against the conversation's current config, and
return the parsed `PartialAppConfig` for recording.
The two editors differ only in where the query text comes from; both record the
parsed partial in full (and will adopt same-turn config resolution from [RFD
080] uniformly when it lands).

1. **Query text seeding.** Query text is seeded in priority order: the request
   built from the invocation (`--quote`, piped stdin, `<query>`, or the replay
   seed) when non-empty, then the draft's `doc.query`, and otherwise an empty
   buffer.
   The draft's config preamble and history metadata stay in `QUERY_MESSAGE.md`
   and never enter the inline editor.
   When invocation text replaces a non-empty `doc.query`, JP prints a chrome
   notice; cancelling composition restores the draft file, so the replaced draft
   text is discarded only after a successful send.
   Direct sends (`jp q <query>` or piped stdin without `-e` or `--quote`) skip
   composition and do not read, modify, or remove the draft.
2. **Inline submission.** Submitting a non-empty inline buffer applies the
   draft's reconciled config preamble just like the external editor, recording
   the parsed `PartialAppConfig` on the conversation as a config delta.
   Because reconciliation updates unedited fields to the current invocation's
   config, a leftover draft applies only the user's edits.
   Submitting an empty or whitespace-only buffer follows the empty-query rule:
   no config delta is recorded and the draft is kept.
   When the preamble produces a non-empty delta against the conversation's
   current config, JP prints `Applying config from the query draft.` (suffixed
   with `(Ctrl+X to review)` when `Ctrl+X` is enabled and an external editor
   resolves).
   Textual differences in the preamble that produce an empty config delta do not
   trigger the notice.
3. **Parse error escalation.** A draft that fails to parse opens in the external
   editor instead of the inline editor.
   On a TOML preamble error, JP re-opens the full document with the error
   annotated (using the existing re-open mechanism) and uses the inline buffer
   as the query text.
   On a structural error where `QUERY_MESSAGE.md` cannot be parsed as a
   `QueryDocument`, JP opens the raw file in the external editor and reports the
   error as a chrome notice.
   This replaces the existing `unwrap_or_default()` fallback, which silently
   rebuilds a malformed draft from an empty document and destroys its query
   text.
   When the external editor cannot open (because `compose_in_editor = "never"`,
   no editor is configured, or spawning fails), composition aborts, prints the
   parse error, and keeps the draft.
   The abort message lists the recovery options: configure `editor.cmd` and run
   `jp q -e`, or edit or delete `QUERY_MESSAGE.md` directly.
4. **Escaping with `Ctrl+X`.** Pressing `Ctrl+X` overwrites `doc.query` with the
   inline buffer before opening the external editor.
   Instead of `edit_query`'s existing behavior (which uses the passed query only
   when `doc.query` is empty), the inline buffer always wins as the most recent
   version of the query text.
   The config preamble is reconciled before the editor opens so the user reviews
   the current invocation's config with their earlier edits on top.
   Saving and closing the editor parses the preamble and records it as a
   conversation config delta, which applies on the next turn until [RFD 080]
   moves editor config into same-turn resolution.
5. **Draft removal.** A successful turn removes `QUERY_MESSAGE.md` regardless of
   which editor composed the query, rather than only after external-editor
   composition, so the next bare `jp q` does not re-seed stale text after an
   inline submission.
   Removing the draft loses no config because the preamble was already applied
   on inline submit (step 2) or saved through the external editor on `Ctrl+X`
   (step 4), and replaced query text was already reported (step 1).

### Composition loop

A `compose_query` function in `jp_cli` (next to `edit_query`) runs the
composition loop: it opens the inline editor via `jp_inquire`, opens the
external editor via `edit_query`, and reports editor failures via
`report_editor_failure`.

This loop is deliberately not shared with `InterruptHandler::collect_reply`.
The two loops have different fallback targets (interrupt menu vs. abort) and
return types (`String` vs. `(String, PartialAppConfig)`).
Parameterizing over both would leave only a ~15-line `match` above the existing
shared primitives (`InlineEditor`, `ComposeInEditor`, `build_editor_backend`,
and `report_editor_failure`).
A shared helper can be extracted if a third composition loop is added later.

Alongside the composed text and config, `compose_query` returns two separate
booleans: whether to echo the request to the terminal (external-editor text was
never printed to the terminal, whereas an inline submission remains visible in
scrollback) and whether to remove the draft file after a successful turn.
These replace the single `query_from_editor` boolean, which conflates both
responsibilities and cannot represent an inline submission seeded from a draft.

### Rename `InlineReply` to `InlineEditor`

The widget is already called the "inline editor" in user-facing config and docs:
the config section is `editor.inline.*`, the config struct is
`InlineEditorConfig`, and doc comments contrast it with the external editor.
`InlineReply` named the widget after its first consumer (interrupt replies),
which does not fit tool-result editing or query composition.

| Current                       | New                                      |
| ----------------------------- | ---------------------------------------- |
| `InlineReply`                 | `InlineEditor`                           |
| `ReplyOutcome`                | `InlineOutcome` (variants unchanged)     |
| `ReplyEditMode`               | `InlineEditMode` (matches the name of    |
|                               | `jp_config::editor::InlineEditMode`; the |
|                               | two types stay separate because          |
|                               | `jp_inquire` does not depend on          |
|                               | `jp_config`)                             |
| `PromptBackend::inline_reply` | `inline_edit`                            |
| prose: "inline reply widget"  | "inline editor"                          |

"Compose" remains the name of the policy axis (where a message is written),
while "inline editor" and "external editor" name the two surfaces.
An **Inline Editor** entry is also added to the ubiquitous-language glossary.

## Drawbacks

- **Default behavior of bare `jp q` changes.** Users who rely on a bare `jp q`
  opening `$EDITOR` must set `query.compose_in_editor = true` or pass `-e`.
  This breaking change is acceptable pre-release, though it would not be
  post-release.
- **~40 lines of duplicated composition policy** between `compose_query` and
  `collect_reply` (see Composition loop).
- **New top-level `[query]` config section with a single key.** While this
  matches `[interrupt]` and gives future query settings a home, it adds config
  surface area.
- **The inline editor is a line editor.** Editing long drafts or `--quote` seeds
  is less comfortable in the inline editor than in `$EDITOR`, though `Ctrl+X`
  switches to the external editor in one keystroke.

## Alternatives

- **`editor.compose_in_editor` instead of a `[query]` section.** Rejected
  because the key name does not indicate that it applies only to queries, and
  placing it alongside `editor.inline.*` (which applies to all composition
  contexts) would be confusing.
- **Global `editor.compose_in_editor` default with per-context overrides.**
  Having `query` and `interrupt` inherit from `editor.compose_in_editor` is more
  orthogonal, but it changes the semantics of two shipped `interrupt` keys and
  makes their resolved configs `Option`-typed without user demand.
  Revisit this if a fourth `compose_in_editor` consumer appears, such as the
  inquiry free-text path (which currently hardcodes its editor escape).
- **Shared composition state machine.** Rejected because the callers differ
  primarily in their failure handling, leaving too little shared logic to
  justify an intermediate layer.
- **Naming the widget `InlineComposer`.** Rejected because it diverges from the
  user-facing `editor.inline.*` config keys, and "compose" is better kept for
  the policy axis.
- **Keeping the external editor as the default and making the inline editor
  opt-in.** Rejected because the inline editor works without configuration, and
  `Ctrl+X` preserves the full-document workflow at one keystroke.

## Non-Goals

- **Editing the TOML preamble in the inline editor.** The inline editor never
  renders or edits the config preamble; editing config requires pressing
  `Ctrl+X` to open the external editor.
  The draft file's preamble is still applied on inline submit (see Query draft
  lifecycle).
- **File-path query arguments.** `jp q ./text.txt` continues to send the literal
  string; use `jp q "$(< text.txt)"` or stdin.
- **Compose policy for inquiry free-text answers.** The tool prompter keeps its
  hardcoded editor escape; adding a `compose_in_editor` key for it is future
  work.
- **Non-interactive and detached behavior.** Flows without a tty are unchanged
  (see [RFD 049]).
- **Per-context external-editor selection.** JP still uses a single configured
  external editor, matching [RFD 088].

## Risks and Open Questions

- **Large seeded buffers in reedline.** Multi-line drafts and `--quote`
  blockquotes can seed the inline editor with large buffers.
  Rendering and cursor behavior at that size need a spike.
- **Echoing after inline submission.** `should_echo_request` re-echoes the
  request when the external editor took over the screen.
  Because a submitted inline buffer stays visible in scrollback, it should not
  be re-echoed; verify that the rendered transcript reads cleanly.
- **Tty detection.** The inline editor requires `/dev/tty`.
  Detection must match how `Printer::prompt_writer` acquires `/dev/tty` so the
  fallback decision and the render target cannot disagree.
- **Draft seeding edge cases.** Whitespace-only drafts and drafts containing
  only a config preamble should seed an empty inline buffer and apply the
  preamble only on a non-empty submit.
  Unparseable preambles and structurally damaged documents follow the escalation
  rules in Query draft lifecycle.

## Implementation Plan

### Phase 1: Rename `InlineReply` to `InlineEditor`

Rename `InlineReply` to `InlineEditor` and apply the associated renames across
`jp_inquire`, `jp_cli`, docs, and doc comments.
Add the glossary entry.
This phase preserves behavior and can merge independently.

### Phase 2: Configuration

Move `ComposeInEditor` to `jp_config::editor` and make its doc comment
context-neutral.
Add `QueryConfig` with `compose_in_editor` and the partial config traits
(`AssignKeyValue`, `PartialConfigDelta`, `FillDefaults`, `ToPartial`).
Keep `-e`, `-E`, and `--quote` out of `apply_cli_config` so `compose_query`
resolves their invocation-local effects after config resolution and conversation
delta recording.
This phase has no dependencies and can merge before the key is wired up.

### Phase 3: Composition loop and draft lifecycle

Add `compose_query` and route bare `jp q` and forced-composition invocations
through it.
Change `edit_query` seeding so the inline buffer overwrites `doc.query`.
Apply the reconciled draft preamble on inline submit (with the delta-gated
notice and parse-error escalation), reusing the reconciliation step that
`edit_query` runs before opening the external editor, and extend draft removal
to inline submissions.
Replace the `query_from_editor` boolean with separate booleans for request
echoing and draft removal.
Update the `--edit`, `--no-edit`, and `--quote` help text to describe
interactive composition under `query.compose_in_editor`.
Depends on Phases 1 and 2.

### Phase 4: Documentation

Update `docs/configuration.md` and the usage docs for the new default, the
`[query]` section, and the updated flag behavior.

## References

- [RFD 088]: Defines the `EditorBackend` trait, the inline editor widget, and
  the `compose_in_editor` modes that this RFD extends to query composition.
- [RFD 080]: Moves editor-provided config into same-turn config resolution.
  This RFD does not require it: until [RFD 080] lands, the draft preamble is
  parsed and recorded as a conversation config delta that applies from the next
  turn.
  [RFD 080] changes when draft config takes effect on both surfaces, not whether
  the preamble survives composition.
- [RFD 049]: Defines non-interactive mode, which this RFD leaves unchanged.

[RFD 049]: 049-non-interactive-mode-and-detached-prompt-policy.md
[RFD 080]: 080-editor-as-a-config-source.md
[RFD 088]: 088-unified-editor-service-and-inline-reply-widget.md
