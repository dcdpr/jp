# RFD 085: Query Explain

- **Status**: Discussion
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-05-12
- **Summary**: Adds an `--explain` flag to `jp query` that prints the assembled
  LLM request and exits without calling the provider.

## Summary

This RFD adds an `--explain` flag to `jp query` that prints the rendered system
prompt, resolved tools, resolved attachments, and outgoing user message, then
exits without calling the LLM provider.
Output supports both terminal rendering and structured JSON via `--format json`.

## Motivation

JP composes each LLM request from workspace config, conversation deltas, CLI
flags, tool directives, attachments, and the user's query.
By the time a turn is sent, it is hard to tell what the assistant actually
receives:

- Which tools are enabled this turn, and with what parameter schemas?
- What does the rendered system prompt look like after persona, instructions,
  and overrides merge?
- Which attachments are included, and where do they come from?
- Did the query template render as expected?

Answering these questions otherwise requires reading provider logs,
instrumenting the code, or guessing.
`--explain` makes the assembled request inspectable in one command, with
structured output that composes with `jq` and scripts.

## Design

### User Experience

`--explain` is a standalone boolean flag with no companion flags:

```sh
jp q -c architect --explain "my query"
jp q -c architect --explain --format json "my query"
```

Behavior:

- **Assembles the full request, then exits.** Runs the same setup as `jp q`
  (including MCP server startup, tool schema resolution, and attachment
  fetching) and exits before calling the provider.
- **Implies `--no-persist`.** Persists no conversation events, skips background
  title generation, and delegates locking to `NullLockBackend`.
- **Prints the assembled components.** Echoes the rendered system prompt and its
  sections, the enabled tools (name, description, and JSON Schema), the
  attachment list (source and title), and the new user request.
- **Not a dry-run.** Request assembly performs its usual side effects: MCP
  server startup, MCP tool and resource calls, HTTP fetches, file reads, and
  command-attachment execution.
  Only conversation persistence and the LLM provider call are skipped.
  Resolution failures abort the preview, just like a normal query.

The flag composes with every existing `jp q` flag (`-m`, `-r`, `--tool`, `-a`,
`--cfg`, `-%`, etc.): set up a query as usual and add `--explain` to inspect the
request.

### Output Schema (v1)

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

`title` on an attachment is a best-effort display label:
`Attachment.description` if set, otherwise the URL's last path segment, or the
raw source as a fallback.

`description` on a tool is `ToolDocs::schema_description()`, the same string
sent to providers in the tool schema (the tool's summary if set, otherwise its
description).
The preview shows what the assistant receives rather than internal documentation
fields.

`source` on a tool follows `ToolSource` serialization: `builtin`, `local`, or
`mcp` in the common case, and dotted forms (`local.<tool>`, `mcp.<server>`,
`mcp.<server>.<tool>`) when the config overrides the tool name.

Text rendering walks the same struct: a header per section, the system prompt
rendered as Markdown, tools listed with their descriptions and parameter schemas
as fenced JSON blocks, attachments as a labeled list, and the user query in a
quoted block.
It reuses `SectionConfig::render()` for system sections,
`ToolDefinition::to_parameters_schema()` for schemas, `jp_md` for Markdown
rendering, and the format-aware printer.

### Architecture

`--explain` implies `--no-persist` at startup rather than inside `Query::run`,
because `load_workspace` selects the persist and lock backends before
`Query::run` is called.
`Commands::implies_no_persist() -> bool` (default `false`, returning
`self.explain` on `Query`) follows the `Commands::conversation_load_request()`
pattern for declarative per-command startup metadata.

`run_inner` computes the effective persist value (`cli.globals.persist &&
!cli.command.implies_no_persist()`) before `load_workspace` and writes it back
to `cli.globals.persist` before `Ctx::new` runs.
Both `load_workspace` and every downstream consumer of `ctx.term.args.persist`
(notably background title generation in `Query::run`) see the updated value, so
persist-gated logic needs no separate `!self.explain` guards.
Consolidating the overlap between the persist flag and `NullPersistBackend` is
left to a separate refactor.

In `Query::run`, the explain branch short-circuits after `chat_request` stamping
and **before** the editor echo block (`TurnView::render_user_request`).
Skipping the echo prevents non-JSON stdout output under `--format json`, and the
preview already includes the request body.
At that point, all preview inputs are available in local bindings:

- `Vec<ToolDefinition>` from `tool_definitions(...)`, including resolved MCP
  tool schemas (`configure_active_mcp_servers` starts MCP servers earlier in
  `Query::run`, independent of persistence).
- `Vec<Attachment>` resolved by the attachment handlers.
- The final `ChatRequest` after stdin, query, template, editor, schema, and
  author processing.
- The system prompt (`cfg.assistant.system_prompt`) and rendered sections from
  `build_sections(&cfg.assistant, !tools.is_empty())`, the same call
  `build_thread` makes.
- The per-tool config (`cfg.conversation.tools`), used to recover each tool's
  `source` after `tool_definitions` strips that field from
  `Vec<ToolDefinition>`.

The explain branch builds a `QueryPreview` from these bindings, prints it via
the format-aware printer, removes the editor query file (`QUERY_MESSAGE.md`) on
success to match post-`handle_turn` cleanup, and returns.
Whereas the executed path rebuilds the thread inside `run_turn_loop` after
appending the new request to the stream, the explain path renders its preview
directly from the same upstream inputs and omits only conversation history.

This change also removes the pre-turn `build_thread(...)` call in `Query::run`.
That call built a `Thread` only for `handle_turn` to read `&thread.attachments`,
while `run_turn_loop` rebuilt the thread itself.
Passing `attachments` directly into `handle_turn` eliminates the redundant
build.

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

`QueryPreview` derives `Serialize` and carries a `schema_version` field so the
JSON shape can evolve without breaking existing consumers.
JSON output passes `QueryPreview` to `print_json`, widening that helper from
`&serde_json::Value` to `T: Serialize`.
Text output uses a single render function over `QueryPreview`.

## Drawbacks

- **Adds a flag to a command that already has many.** `jp q --help` grows,
  mitigated by concise short-help text.
- **Maintenance contract on the JSON shape.** Every change to request assembly
  must decide whether to surface the change in the preview and how to evolve the
  schema.
  That maintenance cost is the trade-off for making requests inspectable by
  users and AI agents.

## Alternatives

**`jp query show` subcommand.** `jp q`'s trailing positional `query:
Option<Vec<String>>` consumes any non-flag tokens as the query body, so `jp q
show ...` parses `show` as query text rather than a subcommand.
Reshaping `jp q` to support subcommands is a larger change than this feature
warrants.

**`jp prompt print` (or `jp request print`) as a peer command.** A separate
command isolates inspection from execution, but forces users to rewrite a `jp q`
invocation in a different form to inspect it.
Discoverability also suffers: a user checking `jp q --help` for an inspection
option looks for a flag on `jp q`, not a sibling command.

**Multiple scope flags (`--show-tools`, `--show-system`, ...).** Dedicated flags
add CLI surface area for filtering that JSON consumers can already do with `jq`.
If filtering proves useful, `--explain` can accept an optional filter list
(`--explain=tools,system`) without breaking the boolean form.

**Showing history contents.** `jp c print` already renders conversation history
with style and turn-selection flags (`--last N`, `--turn N`).
Duplicating that output on `jp q` would pull those scope-modifier flags onto `jp
q` as well.
Use `jp c print` directly when prior turns matter.

**Extracting a `build_query` function as the single source of truth.** Moving
request assembly out of `Query::run` into a shared helper would only help if
both paths built a full `Thread`.
Without conversation history in the preview, the shared inputs are the system
prompt, tools, attachments, and new request, all of which `Query::run` already
binds locally before `handle_turn`.
Short-circuiting in `Query::run` reuses those bindings directly.

**Wire-format preview.** Each provider serializes requests into its own wire
format (Anthropic blocks, OpenAI messages, Gemini parts).
`--explain` operates on JP's provider-agnostic request representation instead.
A provider-specific wire preview can be added separately (for example, as
`--wire-explain`) without conflict.

## Non-Goals

- **History contents.** Use `jp c print` to inspect conversation history with
  style and turn-selection flags.
- **Attachment content.** The preview lists attachments by source and title; use
  `jp a print <url>` to inspect attachment contents.
- **Wire-format preview.** Out of scope (see Alternatives).

## Risks and Open Questions

- **JSON schema stability.** Downstream scripts will depend on the JSON field
  names (Hyrum's Law).
  Versioning the payload with `schema_version` and documenting the schema in the
  user-facing docs alongside `--explain` makes the contract explicit from day
  one.
- **System prompt rendering parity.** Parity is structural: both paths read
  `cfg.assistant.system_prompt` and call `build_sections(&cfg.assistant,
  !tools.is_empty())` with the same upstream inputs.
  Although the executed path rebuilds the thread inside `run_turn_loop` after
  appending the new request to the stream, only conversation history differs
  between the two snapshots.
- **`--explain` is not side-effect-free.** Attachment and tool resolution run
  the same code as the executed path, including command-attachment execution,
  HTTP fetches, and MCP server startup.
  Resolution handlers are not expected to write to caches or workspace state;
  any handler that does has a bug independent of `--explain`.
- **Session activation under `--no-persist`.** `load_workspace` swaps the
  persist and lock backends to null variants under `--no-persist`, but leaves
  the session backend active.
  Running `jp q --explain --id X` therefore updates the terminal session's
  active conversation to `X`.
  This is a pre-existing gap in `--no-persist` tracked separately; `--explain`
  inherits the behavior rather than fixing it inline.

## Implementation Plan

1. **Widen `print_json` to accept `T: Serialize`.** Relax the parameter bound
   from `&serde_json::Value` and update call sites.
2. **Add `Commands::implies_no_persist()`.** Default to `false` on `Commands`
   (matching `Commands::conversation_load_request()`).
   In `run_inner`, fold the result into `cli.globals.persist` before `Ctx::new`
   and `load_workspace` so `ctx.term.args.persist` and all downstream consumers
   see the effective value.
3. **Add the `--explain` flag on `Query` and the short-circuit branch.** Return
   `self.explain` from `Query::implies_no_persist()`.
   In `Query::run`, branch after `chat_request` stamping and before the editor
   echo block, and remove the editor query file on success.
   Remove the pre-turn `build_thread(...)` call and pass `attachments` directly
   into `handle_turn`.
4. **Define `QueryPreview` and supporting types.** Derive `Serialize`, version
   via `schema_version`, and build `QueryPreview` from `&cfg` (for the system
   prompt and per-tool `source` lookup), `Vec<ToolDefinition>`,
   `Vec<Attachment>`, and `ChatRequest`.
5. **Add the text and JSON renderers.** Reuse `SectionConfig::render()`,
   `jp_md`, `ToolDefinition::to_parameters_schema`, and the format-aware printer
   for text output, and pass `QueryPreview` to `print_json` for JSON output.
6. **Tests.** Add snapshot tests for text and JSON output across fixture
   configurations.
7. **Documentation.** Add a feature page under `docs/features/` describing
   `--explain` and the `schema_version: 1` JSON schema.

Phases 1 and 2 are independent and can land first.
Phases 3-6 depend on them.
Phase 7 follows merge.
