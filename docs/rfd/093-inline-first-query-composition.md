# RFD 093: Inline-First Query Composition

- **Status**: Discussion
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-07-02
- **Extends**: [RFD 088]

## Summary

A bare `jp query` composes the query in the inline editor (the reedline-based
widget from [RFD 088]) instead of opening the external editor.
`Ctrl+X` escapes into the full external-editor query document, and a leftover
query draft pre-seeds whichever surface opens.
A new `query.compose_in_editor` setting selects the surface using the existing
`ComposeInEditor` spectrum, and the widget is renamed from `InlineReply` to
`InlineEditor` to match the user-facing vocabulary.

## Motivation

`jp query` has five entry paths today, and none of them use the inline editor:

| Invocation          | Behavior                                                  |
| ------------------- | --------------------------------------------------------- |
| `jp q` (bare, tty)  | Opens the external editor with the full query document.   |
|                     | Errors with `MissingEditor` if no editor resolves.        |
| `jp q <query>`      | Sends directly.                                           |
| `echo x \| jp q`    | Sends directly.                                           |
| `jp q [<query>] -e` | External editor, seeded with `<query>`.                   |
| `jp q -E`           | No editing; replays the last request or sends `continue`. |

This has three problems:

1. **A bare `jp q` hard-requires a configured external editor.** The inline
   editor works on any tty with zero configuration; requiring `$EDITOR` for the
   flagship command's zero-argument form is an unnecessary setup hurdle.
2. **Short queries pay for a full editor round-trip.** Spawning an editor,
   writing one line, and saving is heavy for the most common interaction.
3. **The compose vocabulary stops short of the query.** [RFD 088] gave interrupt
   replies and tool responses the `compose_in_editor` spectrum (inline-first,
   editor-first, editor-only, inline-only); the initial query, the most-composed
   message of all, has no equivalent.

Doing nothing leaves the initial query as the last high-frequency composition
path that hard-requires the external editor.

## Design

### Behavior

| Invocation                          | New behavior                                              |
| ----------------------------------- | --------------------------------------------------------- |
| `jp q` (tty)                        | Inline editor. `Ctrl+X` escapes to the full query         |
|                                     | document in the external editor, seeded with the buffer.  |
|                                     | A cancelled or empty external editor returns to the       |
|                                     | inline editor with the buffer intact. `Ctrl+C` aborts.    |
| `jp q` + `compose_in_editor = true` | External editor first (today's behavior); a spawn         |
|                                     | failure falls back to the inline editor.                  |
| `... = "always"`                    | External editor or error; never the inline editor.        |
| `... = "never"`                     | Inline editor only; `Ctrl+X` is unbound.                  |
| `jp q [<query>] -e`                 | Forces composition and prefers the external editor,       |
|                                     | seeded with `<query>` when given. A configured `"always"` |
|                                     | keeps its no-inline-fallback semantics.                   |
| `jp q` (tty), empty submit          | An empty (or whitespace-only) Enter in the inline editor  |
|                                     | aborts composition with the existing "Query is empty,     |
|                                     | ignoring." notice; the query draft is not consumed.       |
| `jp q --quote`                      | Forces composition (as today); the surface follows        |
|                                     | `query.compose_in_editor`.                                |
| `jp q <query>`, piped stdin, `-E`   | Unchanged: send directly / skip composition.              |
| No tty                              | Unchanged: `query.compose_in_editor` has no effect        |
|                                     | without `/dev/tty` (external editor or `MissingEditor`).  |

Two consequences worth naming: a bare `jp q` no longer requires any editor
configuration, and quick one-line queries become cheaper than an editor
round-trip while `Ctrl+X` keeps the full document one keystroke away.

The empty-submit rule is deliberately asymmetric with the escape path: an empty
*external editor* result returns to the inline editor because it may be
discarding a buffer the user already typed, while an empty inline Enter has
nothing to preserve and ends the invocation, matching today's empty-query
behavior.

### Configuration

```toml
[query]
compose_in_editor = false # false | true | "always" | "never"
```

A new top-level `[query]` section holds query-composition behavior, mirroring
how `[interrupt]` holds query-interrupt behavior.
The value type is the existing `ComposeInEditor` enum, which moves from
`jp_config::interrupt` to `jp_config::editor` — it is editor vocabulary, not
interrupt vocabulary, and it now has consumers in two sections.
The move changes no serialized format (the enum is a value type, not a key).

The enum's fallback semantics are per-context.
For interrupts the fallback target is the interrupt menu; for the query there is
no menu, so:

- `false` (default): inline editor first; `Ctrl+X` escapes to the external
  editor.
- `true`: external editor first; if it cannot open, fall back to the inline
  editor.
- `"always"`: external editor only; if it cannot open, the query aborts with an
  error.
- `"never"`: inline editor only; the `Ctrl+X` escape is disabled.

The enum's doc comment becomes context-neutral about the fallback target; each
context's field documents its own.

When `/dev/tty` is unavailable, `query.compose_in_editor` has no effect:
composition follows the existing external-editor behavior, including
`MissingEditor` when no editor resolves.
This matches the shipped `interrupt.*.compose_in_editor` keys, which document
themselves as inert in non-interactive (no-tty) mode.

`-e`/`--edit`, `-E`/`--no-edit`, and `--quote` stay outside `AppConfig`, as they
are today: `apply_cli_config` ignores them, so they are never recorded as a
conversation config delta the way `--cfg query.compose_in_editor=...` is.
Instead, `compose_query` resolves the effective policy after config resolution
and delta recording.
The flags control *whether* composition happens (`-e` and `--quote` force it;
`-E` skips it), and `-e` additionally prefers the external editor for this
invocation: a configured `false` or `"never"` behaves as `true`, while `true`
and `"always"` are unchanged — `-e` never re-enables the inline fallback that
`"always"` opts out of.

### Query draft lifecycle

A leftover `QUERY_MESSAGE.md` draft takes part in composition on both surfaces;
item 1 defines what seeds the query text when the draft and the invocation both
provide some.
Both surfaces run the same query-document config step: parse the `QueryDocument`
structure, parse the TOML preamble, resolve model aliases, compute the notice
delta against the conversation's current config, and return the parsed partial
for recording.
The surfaces differ only in where the query *text* comes from; what gets
recorded is identical on both — today the parsed partial, wholesale, and
uniformly whatever [RFD 080] changes that to when it lands.

1. Query text is seeded by precedence: the invocation-built request (`--quote`,
   piped stdin, `<query>`, replay seed — as built by the query command) when
   non-empty; otherwise the draft's `doc.query`; otherwise the buffer starts
   empty.
   The draft's config preamble and history metadata are preserved regardless of
   which source wins; they stay in the file and never enter the widget.
   When invocation text displaces a non-empty `doc.query`, a chrome notice says
   so; a cancelled composition reverts the draft file, so the displaced text is
   lost only on a successful send.
   A direct send (`jp q <query>` or piped stdin without `-e`/`--quote`) bypasses
   composition entirely: the draft is neither seeded, nor modified, nor
   consumed.
2. On a non-empty inline submit, the draft's config preamble is applied exactly
   as the external-editor path applies it: the parsed partial is recorded on the
   conversation as a config delta.
   An empty (or whitespace-only) submit follows the empty-query rule instead: no
   config delta is recorded and the draft is kept.
   The preamble's effect — its delta against the conversation's current config
   — gates only a chrome notice; it does not change what is recorded.
   When the effect is non-empty, the notice `Applying config from the query
   draft.` tells the user that config came along with the text, suffixed with
   `(Ctrl+X to review)` only when the editor escape is wired and an external
   editor resolves.
   A preamble that is textually different but semantically a no-op stays silent
   — what matters is whether the draft changes the conversation config, not how
   the file is worded.
3. A draft that fails to parse escalates to the external editor instead of
   composing inline.
   A TOML preamble error re-opens the full document with the error annotated
   (the existing re-open mechanism), the inline buffer winning for the query
   text.
   A structural error (`QUERY_MESSAGE.md` cannot be parsed as a `QueryDocument`
   at all) is detected when the draft is first read: the editor opens on the
   file exactly as it is, with the error reported as a chrome notice.
   This replaces today's silent fallback, where a structurally damaged draft is
   swallowed by `unwrap_or_default()` and the force-write rebuilds the file from
   an empty document, destroying the draft's query text.
   When the editor cannot be opened — `compose_in_editor = "never"`, no editor
   configured, or a spawn failure — composition aborts, the parse error is
   printed, and the draft is kept.
   The abort message names the recovery options: configure `editor.cmd` and run
   `jp q -e`, or edit or delete `QUERY_MESSAGE.md` directly.
4. On `Ctrl+X`, the buffer *overwrites* `doc.query` in the document handed to
   the external editor.
   Today `edit_query` uses the passed query only when `doc.query` is empty; this
   flips to buffer-wins: the buffer is the user's most recent statement of the
   query text, whatever seeded it.
   The config preamble persists untouched and keeps today's behavior: parsed and
   recorded as a conversation config delta, applied from the next turn until
   [RFD 080] moves it into same-turn config resolution.
5. The draft is consumed (removed) after a successful turn regardless of which
   surface composed the message.
   Today only the external-editor path triggers removal; without extending it,
   the next bare `jp q` would re-seed stale text after an inline submission.
   Removal loses no config: the preamble was applied on submit (item 2), on
   escape it round-tripped through the editor (item 4).
   Displaced query text is covered by the notice rule in item 1.

### Composition loop

A `compose_query` function in `jp_cli` (next to the existing `edit_query`)
implements the policy loop: inline editor via `jp_inquire`, external editor via
`edit_query`, failure reporting via `report_editor_failure`.

The loop is deliberately *not* shared with `InterruptHandler::collect_reply`.
The two differ in fallback target (menu vs. abort) and return type (`String` vs.
`(String, PartialAppConfig)`); after parameterizing over both, the shared core
is a ~15-line `match` — the wrong abstraction.
The shared primitives (`InlineEditor`, `ComposeInEditor`,
`build_editor_backend`, `report_editor_failure`) are already shared.
If a third full compose loop appears, extract then.

Alongside the composed text and config, `compose_query` reports two independent
facts: whether the request still needs echoing (only editor-composed text was
never rendered on the terminal; an inline submission remains visible in
scrollback) and whether a draft file needs consuming after a successful turn.
The existing `query_from_editor` boolean conflates the two and cannot express an
inline submission seeded from a draft; it is replaced.

### Rename: `InlineReply` becomes `InlineEditor`

The widget's user-facing name is already "inline editor": the config section is
`editor.inline.*`, the config struct is `InlineEditorConfig`, and doc comments
contrast it with "the external editor".
`InlineReply` names the widget after its first use (interrupt replies) and is
already wrong for tool-result editing, let alone query composition.

| Current                       | New                                      |
| ----------------------------- | ---------------------------------------- |
| `InlineReply`                 | `InlineEditor`                           |
| `ReplyOutcome`                | `InlineOutcome` (variants unchanged)     |
| `ReplyEditMode`               | `InlineEditMode` (name-matching          |
|                               | `jp_config::editor::InlineEditMode`; the |
|                               | two types stay separate — `jp_inquire`   |
|                               | does not depend on `jp_config`)          |
| `PromptBackend::inline_reply` | `inline_edit`                            |
| prose: "inline reply widget"  | "inline editor"                          |

"Compose" remains the name of the policy axis (*where* a message is composed);
"inline editor" and "external editor" are the two surfaces.
An **Inline Editor** entry is added to the ubiquitous-language glossary.

## Drawbacks

- **The flagship command's default changes.** Users who rely on bare `jp q`
  opening `$EDITOR` must set one config key or pass `-e`.
  Pre-release, this is acceptable; post-release it would not be.
- **~40 lines of duplicated compose policy** between `compose_query` and
  `collect_reply`, accepted deliberately (see Composition loop).
- **A new top-level config section with a single key.** Justified by the
  `[interrupt]` precedent and by giving future query-scoped settings a home, but
  it is surface area.
- **The inline editor is a line editor.** Long drafts and `--quote` seeds are
  less comfortable there than in `$EDITOR`; `Ctrl+X` is the mitigation.

## Alternatives

- **`editor.compose_in_editor` instead of a `[query]` section.** Rejected: the
  name doesn't say "query", and it would sit next to `editor.inline.*`, which
  applies to *all* composition contexts — a confusion trap.
- **A global compose default with per-context overrides**
  (`editor.compose_in_editor` inherited by query and interrupt contexts).
  The most orthogonal shape, but it changes the semantics of two shipped
  interrupt keys and makes their resolved configs `Option`-typed for a need
  nobody has expressed.
  Flip condition: a fourth `compose_in_editor` consumer — the likely candidate
  is the inquiry free-text path, which today hardcodes its editor escape.
- **Extracting a shared compose state machine.** Rejected as a midlayer: the
  contexts' failure semantics are the part that differs.
- **Naming the widget `InlineComposer`.** Rejected: it moves the code *away*
  from the user-facing `editor.inline.*` keys, and "compose" is better kept for
  the policy axis.
- **Keeping the external editor as the default and adding inline as opt-in.**
  Rejected: the inline editor is the better zero-config default, and the escape
  hatch preserves the full-document flow at one keystroke.

## Non-Goals

- **TOML editing in the widget.** The inline editor never renders or edits the
  config preamble; *editing* config requires the `Ctrl+X` escape.
  The draft file's preamble is still applied on inline submit (see the draft
  lifecycle) — out of scope is only growing TOML display or editing into the
  widget itself.
- **File-path query arguments.** `jp q ./text.txt` continues to send the literal
  string; use `jp q "$(< text.txt)"` or stdin.
- **Compose policy for inquiry free-text answers.** The tool prompter keeps its
  hardcoded editor escape; giving it a `compose_in_editor` key is future work.
- **Non-interactive and detached behavior.** No-tty flows are unchanged (see
  [RFD 049]).
- **Per-context external-editor selection.** Still one configured external
  editor, as in [RFD 088]'s non-goals.

## Risks and Open Questions

- **Large seeded buffers in reedline.** Multi-line drafts and `--quote`
  blockquotes seed the widget with substantial text; rendering and cursor
  behavior at that size need a spike.
- **Echo after inline submission.** `should_echo_request` re-echoes the request
  when the external editor took over the screen.
  A submitted inline buffer stays visible in scrollback, so no re-echo should be
  needed — validate that the rendered transcript reads correctly.
- **Tty detection.** The widget requires `/dev/tty`; detection must be
  consistent with how `Printer::prompt_writer` acquires it, so the fallback
  decision and the render target cannot disagree.
- **Draft seeding edge cases.** Whitespace-only drafts and drafts containing
  only a config preamble need defined behavior (proposed: seed an empty widget;
  the preamble applies only on a non-empty submit, per the lifecycle rules).
  Unparseable preambles and structurally damaged documents are covered by the
  escalation rules in the draft lifecycle.

## Implementation Plan

### Phase 1: rename (independent, behavior-preserving)

`InlineReply` → `InlineEditor` and the associated renames across `jp_inquire`,
`jp_cli`, docs, and doc comments.
Add the glossary entry.
Mergeable on its own.

### Phase 2: configuration

Move `ComposeInEditor` to `jp_config::editor`; make its doc comment
context-neutral.
Add `QueryConfig` with `compose_in_editor` and the full partial plumbing
(`AssignKeyValue`, `PartialConfigDelta`, `FillDefaults`, `ToPartial`).
Leave `-e`, `-E`, and `--quote` out of `apply_cli_config` (as today); their
invocation-local effects are resolved in `compose_query` after config resolution
and conversation delta recording.
Depends on nothing; mergeable with the key unused.

### Phase 3: compose loop and draft lifecycle

Add `compose_query`; wire bare `jp q` (and forced composition) through it.
Flip `edit_query` seeding to buffer-wins; apply the draft preamble on inline
submit (with the delta-gated notice and the parse-error escalation); extend
draft consumption to inline submissions.
Replace the `query_from_editor` boolean with the two independent facts from the
Composition loop section.
Update the `--edit`, `--no-edit`, and `--quote` help text to the new semantics
(compose interactively; the surface follows `query.compose_in_editor`), so
`--help` and behavior change together.
Depends on phases 1–2.

### Phase 4: documentation

Update `docs/configuration.md` and the usage docs for the new default, the
`[query]` section, and the flag semantics.

## References

- [RFD 088] — the `EditorBackend` trait, the inline editor widget, and the
  `compose_in_editor` spectrum this RFD extends to the query.
- [RFD 080] — moves editor-provided config into same-turn config resolution.
  This RFD does not require it: until it lands, the draft preamble keeps today's
  behavior (parsed and recorded as a conversation config delta, applied from the
  next turn).
  RFD 080 changes *when* draft config applies — on both surfaces uniformly —
  not whether the preamble survives composition.
- [RFD 049] — non-interactive mode; untouched by this RFD.

[RFD 049]: 049-non-interactive-mode-and-detached-prompt-policy.md
[RFD 080]: 080-editor-as-a-config-source.md
[RFD 088]: 088-unified-editor-service-and-inline-reply-widget.md
