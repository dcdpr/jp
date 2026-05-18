# RFD 085: Query Explain

- **Status**: Discussion
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-05-12

## Summary

Add an `--explain` flag to `jp query` that prints the rendered system prompt,
the resolved tools, the resolved attachments, and the new user query that would
be sent to the LLM, then exits without calling the provider. Output is
structured and JSON-first via `--format json`.

## Motivation

JP composes the next request to the LLM from many sources: workspace config,
conversation deltas, CLI flags, tool directives, attachments, and the user's
query. By the time a turn is sent, the user often can't easily tell *what* the
assistant is seeing. Common questions:

- Which tools is the assistant getting this turn, and with what parameter
  schemas?
- What does the fully rendered system prompt look like after persona,
  instructions, and overrides have merged?
- Which attachments are attached, and where are they coming from?
- Is the query template rendering the way I think?

Today, answering these requires reading provider logs, instrumenting the code,
or guessing. `--explain` makes the assembled request inspectable in one command.
Because the output is structured, it composes with `jq` and scripts.

## Design

### User experience

A single boolean flag, no companion flags:

```sh
jp q -c architect --explain "my query"
jp q -c architect --explain --format json "my query"
```

Behavior:

- Assembles the full request the same way `jp q` would — including MCP
  server startup, tool schema resolution, and attachment fetching — then
  exits before calling the provider.
- `--explain` implies `--no-persist`. No conversation events are persisted,
  no title is generated in the background, and locking is delegated to
  `NullLockBackend`.
- Echoes the rendered system prompt and its sections, the enabled tools
  (name, description, JSON Schema), the attachment list (source + title),
  and the new user request.
- **Not a dry-run.** Request assembly performs its usual side effects:
  MCP server startup, MCP tool/resource calls, HTTP fetches, file reads,
  and command-attachment execution. These are real, and exactly what the
  executed path performs. Only the conversation state and the LLM
  provider call are skipped. Resolution failures abort the preview, same
  as a normal query.

The flag composes naturally with every existing `jp q` flag (`-m`, `-r`,
`--tool`, `-a`, `--cfg`, `-%`, etc.). The mental model is "set up the query
you'd run, then flip a flag to see what it would be."

### Output schema (v1)

```json
{
  "schema_version": 1,
  "system": {
    "prompt": "...",
    "sections": [
      :
        "title": "...",
        "tag": "...",
        "content": "..."
      }
    ]
  },
  "tools": [
    {
      "name": "fs_grep_files",
      "description": "Search through the project's files.",
      "parameters": {
        "/* JSON Schema */": ""
      },
      "source": "local"
    }
  ],
  "attachments": [
    {
      "source": "file:///path/to/file.md",
      "title": "file.md"
    }
  ],
  "request": {
    "content": "my query",
    "schema": null
  }
}
```

`title` on an attachment is a best-effort display label: `Attachment.description`
if set, otherwise derived from the URL's last path segment, otherwise the raw
source.

`description` on a tool is `ToolDocs::schema_description()` — the same string
providers send in the tool schema (the tool's summary if set, otherwise its
description). The preview shows what the assistant sees, not a richer
internal docs field.

`source` on a tool follows `ToolSource`'s serialization: `builtin`, `local`,
or `mcp` for the common cases; dotted forms (`local.<tool>`, `mcp.<server>`,
`mcp.<server>.<tool>`) when the source overrides the name in config.

Text rendering walks the same struct: a header per section, the system prompt
rendered as markdown, tools listed with their descriptions and parameter
schemas as fenced JSON blocks, attachments as a labeled list, and the user
query in a quoted block. The text renderer reuses existing components where
possible: `SectionConfig::render()` for system sections,
`ToolDefinition::to_parameters_schema()` for schemas, the markdown renderer
in `jp_md`, and the format-aware printer.

### Architecture

`--explain` implies `--no-persist`. The implication is wired in startup,
not inside `Query::run` — by the time `Query::run` is reached,
`load_workspace` has already chosen the persist and lock backends. The
mechanism is a `Commands::implies_no_persist() -> bool` method (default
`false`) consulted by `run_inner` before `load_workspace`; `Query` returns
`self.explain`. This matches the existing
`Commands::conversation_load_request()` pattern for declarative
per-command information needed at startup.

The effective persist value (`cli.globals.persist &&
!cli.command.implies_no_persist()`) is computed once and written back to
`cli.globals.persist` before `Ctx::new` runs. From that point on, both
`load_workspace` and every downstream consumer of `ctx.term.args.persist`
(notably the title-generation branch in `Query::run`) see the same value,
so persist-gated logic doesn't need separate `!self.explain` guards. The
longer-term overlap between the persist flag and the `NullPersistBackend`
type is a known redundancy worth consolidating in a separate refactor; 085
does not depend on that consolidation.

The branch itself short-circuits in `Query::run` after `chat_request`
stamping but **before** the editor echo block — the block that renders
the user's freshly edited request via `TurnView::render_user_request`.
Skipping the echo prevents it from emitting non-JSON text to stdout under
`--format json`, and is sound because the preview already contains the
request body. By that point in the flow, everything the preview needs is
already available locally:

- `Vec<ToolDefinition>` from `tool_definitions(...)`, including fully
  resolved MCP tool schemas. MCP servers are started by
  `configure_active_mcp_servers` earlier in `Query::run`, independently of
  persistence.
- `Vec<Attachment>` resolved by the attachment handlers.
- The final `ChatRequest` after stdin / query / template / editor / schema
  / author processing.
- The system prompt (`cfg.assistant.system_prompt`) and rendered sections
  (`build_sections(&cfg.assistant, !tools.is_empty())`, the same call
  `build_thread` makes).
- The per-tool config (`cfg.conversation.tools`), which lets the preview
  recover each tool's `source` — `tool_definitions` erases that field by
  the time it returns `Vec<ToolDefinition>`.

The explain branch reads these values, builds a `QueryPreview`, prints it
via the format-aware printer, and returns. On success, it also removes
the editor query file (`QUERY_MESSAGE.md`) if one was created by the
editor path — matching the cleanup the executed path performs after
`handle_turn`. Both paths use the same `tool_definitions`, attachment
resolution, and `build_sections` helpers with the same upstream inputs.
The executed path rebuilds the thread inside `run_turn_loop` once the new
request is added to the stream; the explain path renders its preview
from the same inputs. The preview omits conversation history, which is
the only place the two snapshots differ.

As an incidental cleanup, the pre-turn `build_thread(...)` call currently
in `Query::run` is removed. Today it produces a `Thread` only consumed
for its `attachments` field — `handle_turn` only reads
`&thread.attachments`, and `run_turn_loop` rebuilds the thread itself.
After this RFD, `attachments` flows directly into `handle_turn`, and the
explain branch constructs its preview inputs locally.

```rust
pub(crate) async fn run(self, ctx: &mut Ctx, handle: Option<ConversationHandle>) -> Output {
    // ... existing setup: lock, MCP, build_conversation, tools, attachments,
    //     sanitize, chat_request stamping ...

    if self.explain {
        let preview = QueryPreview::from_parts(
            &cfg, &tools, &attachments, &chat_request,
        );
        let result = print_preview(&ctx.printer, &preview);
        if let Some(path) = query_file
            && result.is_ok()
        {
            fs::remove_file(path)?;
        }
        return result;
    }

    // ... editor echo (renders the user's freshly edited request) ...

    let turn_result = self.handle_turn(/* ..., &attachments, ... */).await;
    // ...
}
```

`QueryPreview` derives `Serialize`. The shape is versioned via
`schema_version` so future changes can evolve without breaking existing
consumers.

JSON output passes the preview to `print_json`. The current helper accepts
`&serde_json::Value`; this RFD widens it to accept `T: Serialize`. Text
output is a single render function over the struct.

## Drawbacks

- **Adds a flag to a command that already has many.** `jp q --help` grows.
  Mitigated by clear short-help text.
- **Maintenance contract on the JSON shape.** Every future change to what
  gets sent must consider whether to surface the change in the preview and
  how to evolve the schema. This is the cost of making the request
  inspectable; the benefit is debuggability for users and AI agents alike.

## Alternatives

**`jp query show` subcommand.** Ruled out by the existing CLI shape. `jp q`'s
trailing positional `query: Option<Vec<String>>` consumes any non-flag tokens
as the query body, so `jp q show ...` parses `show` as the query text, not as
a subcommand. Reshaping `jp q` to free up subcommand space is a much larger
change than this feature warrants.

**`jp prompt print` (or `jp request print`) as a peer command.** Cleanly
separates concerns, but the user must rewrite their `jp q` invocation in a
different form to inspect it. The whole point of the feature is "I have this
command, what would it actually do?" — making the user re-type it elsewhere
is friction with no architectural payoff. Discoverability is also worse: a
user inside `jp q --help` looking for inspection finds a flag, not a sibling
command.

**Multiple scope flags (`--show-tools`, `--show-system`, ...).** Adds CLI
surface area for filtering that JSON consumers can do trivially with `jq`.
If filtering proves useful, add it later as a value on the existing flag
(`--explain=tools,system`) without breaking the boolean form.

**Showing history contents.** Considered and rejected. `jp c print` already
renders conversation history with rich style and turn-selection flags;
duplicating that surface would drag scope-modifier flags (`--last N`,
`--turn N`) onto `jp q` for a job that is already done well elsewhere. Use
`jp c print` directly when prior turns matter.

**Extracting a `build_query` function as the single source of truth.**
Earlier drafts proposed pulling the request-building logic out of `Query::run`
into a pure function called by both the executed and explained paths to
prevent drift. Once history contents were dropped from the preview, the only
work left to share was the system prompt + tools + attachments + new
request — all of which `Query::run` already produces in locals before
`handle_turn`. Short-circuiting there reuses the same locals directly, so
the extra function buys nothing.

**Wire-format preview.** Each provider serializes the request differently
(Anthropic blocks, OpenAI messages, Gemini parts). `--explain` operates on
JP's abstract representation, which is stable and provider-agnostic.
Wire-level preview is a separate concern with a different audience and can
be added later as e.g. `--wire-explain` without conflict.

## Non-Goals

- **History contents.** `jp c print` already shows conversation history with
  rich style flags. Use it directly when prior turns matter.
- **Attachment content.** The preview lists attachments by source and title.
  To inspect content, use `jp a print <url>`.
- **Wire-format preview.** Out of scope — see Alternatives.

## Risks and Open Questions

- **JSON schema stability.** Once shipped, downstream scripts will depend on
  the field names (Hyrum's Law). The `schema_version` field is the safety
  valve. Document the schema in the user-facing docs alongside `--explain`
  so the contract is explicit from day one.
- **System prompt rendering parity.** Parity is structural: both paths
  use the same `cfg.assistant.system_prompt` value and the same
  `build_sections(&cfg.assistant, !tools.is_empty())` call. The executed
  path rebuilds the thread inside `run_turn_loop` once the new request is
  added to the stream; the explain path renders its preview from the same
  upstream inputs. The preview omits history, which is the only place the
  two snapshots differ.
- **`--explain` is not side-effect-free.** Attachment and tool resolution
  run exactly as on the executed path, including command-attachment
  execution, HTTP fetches, and MCP server startup. Neither path is
  expected to write to caches or workspace state during resolution; any
  handler that does is a separate bug, not unique to this feature.
- **Session activation under `--no-persist`.** Today, `--no-persist` (and
  therefore `--explain`) still writes the terminal session's
  active-conversation mapping — `load_workspace` swaps the persist and
  lock backends to null variants, but not the session backend. Users
  running `jp q --explain --id X` will observe their session's active
  conversation change to X. This is a pre-existing gap in `--no-persist`,
  tracked separately; This RFD inherits the behaviour rather than fixing it
  inline.

## Implementation Plan

1. **Widen `print_json` to accept `T: Serialize`.** One-line trait bound
   change plus call-site updates. Independent of this feature and reviewable
   on its own.
2. **Add `Commands::implies_no_persist()`.** A method on `Commands`
   (default `false`) consulted by `run_inner` to compute the effective
   persist value, which is then written back to `cli.globals.persist`
   before `Ctx::new` so `ctx.term.args.persist` mirrors it. `load_workspace`
   and every downstream consumer of the flag see the same value. Matches
   the existing `Commands::conversation_load_request()` pattern.
   Independent of this feature.
3. **Add the `--explain` flag on `Query` and the short-circuit branch.**
   `Query::implies_no_persist()` returns `self.explain`, so persistence is
   forced off before workspace setup. The branch sits in `Query::run`
   after `chat_request` stamping but before the editor echo block, and
   removes the editor query file on success (matching the executed path's
   post-`handle_turn` cleanup). As an incidental cleanup, the pre-turn
   `build_thread(...)` call (only used today to pass `thread.attachments`
   into `handle_turn`) is removed; `attachments` flows directly into
   `handle_turn`.
4. **Define `QueryPreview` and supporting types.** Derive `Serialize`.
   Version via `schema_version`. Build it from `&cfg`,
   `Vec<ToolDefinition>`, `Vec<Attachment>`, and `ChatRequest` — `&cfg`
   carries the system prompt input and the per-tool `source` lookup.
5. **Add the text and JSON renderers.** Text path reuses
   `SectionConfig::render()`, `jp_md`,
   `ToolDefinition::to_parameters_schema`, and the format-aware printer.
   JSON path passes `QueryPreview` to the widened `print_json`.
6. **Tests.** Snapshot tests for both text and JSON output across fixture
   configurations.
7. **Documentation.** A feature page under `docs/features/` describing the
   flag and the schema (with `schema_version: 1` called out).

Phases 1 and 2 are independent and can land first. Phases 3–6 depend on
them. Phase 7 follows merge.
