# RFD D39: Conversation Access Tools for the Assistant

- **Status**: Draft
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-05-21
- **Summary**: Adds `conversation_list`, `conversation_grep`, and
  `conversation_read` tools so the assistant can search and read past workspace
  conversations.

## Summary

This RFD adds three tools for the assistant to access past conversations in the
current workspace: `conversation_list` (paginated metadata), `conversation_grep`
(content search), and `conversation_read` (full content of a specific
conversation).
The tools are Rust wrappers under `.config/jp/tools/src/conversation/` that
shell out to `jp conversation` and present a stable JSON contract to the LLM.
They are scoped to the current workspace and this repository's maintenance
tooling; if the pattern proves useful, they can graduate to built-in tools or a
shipped MCP server.

## Motivation

Every session starts without recall of past conversations.
A user working on a long-running project frequently refers to earlier
discussions ("we discussed retry semantics last week", "what model did we settle
on?") and has to copy-paste or re-explain the context.
Giving the assistant tools to list, search, and read its own history removes
that manual step:

- **List**: Discover existing conversations, sorted by recency, with titles and
  basic metadata.
- **Grep**: Search content across one, several, or all conversations to find
  where a topic was discussed.
- **Read**: Fetch the content of a specific conversation, optionally narrowed to
  specific turns or event kinds.

The CLI already provides `jp conversation ls`, `jp conversation grep`, and `jp
conversation print` with filtering, and the workspace's `jp-tools` binary
(`.config/jp/tools/`) already wraps CLI commands (`git_log`, `fs_grep_files`,
`cargo_check`) as assistant tools.
Wrapping `jp conversation` the same way reuses this pattern without new
architectural commitments.

## Design

### Overview

Three `local` tools defined in TOML invoke the `jp-tools` binary.
`jp-tools` runs `jp conversation` with `--format=json`, parses and filters the
JSON output, and formats an XML response matching the other tools in `jp-tools`.

```
  LLM
   │   tool call (conversation_grep, ...)
   ▼
  jp-tools (Rust wrapper)
   │   shells out, with stable args
   ▼
  jp conversation grep --format=json  ─►  events.json / metadata
   │   stable JSON to stdout
   ▼
  jp-tools parses + filters + formats
   │   XML envelope to stdout
   ▼
  LLM (tool result)
```

The Rust wrapper defines the entire LLM-facing contract (tool name, parameters,
and output shape).
The CLI's `--format=json` output is a private interface between two binaries
owned by this repository.

### Scope and Trust

The tools operate on the entire workspace.
JP treats the workspace as a single trust domain: any tool with filesystem
access can already read the conversation directory on disk.
Structured tools expose that data directly so the LLM does not have to grep raw
JSON files.

This differs from the sub-agent tools in [RFD 051], which are scoped to the
current conversation's subtree.
The two designs are complementary: [RFD 051] isolates ephemeral sub-agents,
while this RFD recalls user-driven workspace history.

### Tool APIs

All three tools are read-only and configured with `run = "unattended"` (no user
prompt) and `enable = "explicit"` (opt-in per persona).

#### `conversation_list`

Parameters:

| Name             | Type   | Default    | Description                                   |
| ---------------- | ------ | ---------- | --------------------------------------------- |
| `limit`          | int    | `20`       | Maximum number of conversations to return.    |
| `offset`         | int    | `0`        | Skip the first N conversations after sorting. |
| `sort`           | enum   | `activity` | `created`, `activity`, or `updated`.          |
| `descending`     | bool   | `true`     | Sort most recent first by default.            |
| `archived`       | bool   | `false`    | Return archived conversations instead.        |
| `title_contains` | string | `null`     | Case-insensitive substring filter on title.   |

Returns an XML envelope listing matching conversations with `snake_case` keys
(`id`, `title`, `events_count`, `created_at`, `last_event_at`, `archived_at`,
and `expires_at`), along with the total count and active offset for pagination.

#### `conversation_grep`

Parameters:

| Name          | Type            | Default | Description                                       |
| ------------- | --------------- | ------- | ------------------------------------------------- |
| `pattern`     | string          | -       | Required. Substring, case-insensitive by default. |
| `ignore_case` | bool            | `true`  | Toggle case sensitivity.                          |
| `ids`         | array of string | `null`  | Restrict to specific conversations.               |
| `scopes`      | array of string | all     | `chat`, `tool`, `title`, or any concrete scope.   |
| `context`     | int             | `0`     | Context lines around each match.                  |
| `limit`       | int             | `50`    | Maximum number of hits returned.                  |

Returns an XML envelope of hits.
Each hit carries `id`, `scope`, `text`, `is_match`, and the conversation's title
for context.
The wrapper truncates long lines to a cap suited to LLM context budgets,
separate from the CLI's human-output truncation.

#### `conversation_read`

Parameters:

| Name      | Type            | Default                                               | Description                                   |
| --------- | --------------- | ----------------------------------------------------- | --------------------------------------------- |
| `id`      | string          | -                                                     | Required. Conversation to read.               |
| `turn`    | int             | `null`                                                | 1-based turn index.                           |
| `last`    | int             | `null`                                                | Last N turns. Mutually exclusive with `turn`. |
| `include` | array of string | `["chat", "reasoning", "tool_calls", "tool_results"]` | Event kinds to include.                       |

If neither `turn` nor `last` is set, all turns are returned.
If the unfiltered output exceeds a constant size cap in the wrapper, the tool
rejects the request with an error suggesting `last` or `turn`.

Returns an XML envelope of turns, each containing the included events with typed
fields (`event_kind`, `timestamp`, `content`, and tool call name and arguments
where applicable).

### CLI Changes

Two CLI changes are part of this work and stand alone as bug fixes:

1. **`jp conversation ls --format=json` emits `snake_case` keys** (`id`,
   `events_count`, `last_event_at`, etc.) instead of display-derived table
   headers (`"ID"`, `"#"`, `"Activity"`).
   Serializing the table-header row verbatim was never an intentional contract,
   and [RFD 051] already calls this out as a prerequisite for its tools.

2. **`jp conversation print --format=json` emits the underlying event stream**
   for the selected turn window.
   This serializes the `ConversationEvent`s within the window (`events.json`
   already uses this shape on disk), excluding internal events the LLM does not
   need.
   The CLI emits events verbatim; per-event-type filtering lives in the wrapper.

Both changes are additive.
The `--format=json` output keys on `conversation ls` are not documented as a
contract, though renaming them is an observable change; the wrapper is the only
known consumer.

### Wrapper Layout

The wrapper modules follow the layout of `.config/jp/tools/src/git/`, `fs/`, and
`cargo/`:

```
.config/jp/tools/src/conversation.rs
.config/jp/tools/src/conversation/list.rs
.config/jp/tools/src/conversation/grep.rs
.config/jp/tools/src/conversation/read.rs
```

The top-level `lib.rs` routes tool names by prefix to module runners and adds a
branch for `conversation_`:

```rust
s if s.starts_with("conversation_") => conversation::run(ctx, t).await,
```

Each module owns its parameter parsing, child-process invocation, JSON
deserialization, post-processing (filtering and truncation), and XML formatting.

Tool TOMLs live under `.jp/mcp/tools/conversation/{list,grep,read}.toml`,
mirroring `.jp/mcp/tools/git/` and `.jp/mcp/tools/fs/`.
Each uses `source = "local"` and `command = "just serve-tools {{context}}
{{tool}}"` like every other local tool.

### Persona Enablement

Personas opt into the tools by extending a skill file (such as
`.jp/config/skill/conversation-access.toml`) that enables the relevant tool
TOMLs.
The default `dev` persona leaves them disabled; personas that benefit from
history recall, such as `architect` for long-running research or `committer` for
looking up prior decisions, opt in explicitly.

## Drawbacks

**Double process startup per call.** Each tool call spawns `jp-tools`, which in
turn spawns `jp conversation`.
Those two process startups cost a few hundred milliseconds outside any hot loop,
where LLM latency dominates.

**Token budget on read.** Calling `conversation_read` on a thousand-turn
conversation can inject thousands of tokens into the context window.
The wrapper enforces a size cap and refuses oversized requests with a hint to
narrow the range, which bounds single calls but does not prevent repeated reads.

**Maintenance-only scope.** `.config/jp/tools/` is workspace-local tooling that
other JP installations do not receive.
Users outside this workspace must copy the pattern, build an MCP server, or wait
for the tools to graduate to built-ins.
This tradeoff lets us validate the tool interface before committing to a shipped
feature.

**Hyrum's Law on the wrapper's output shape.** Once personas and prompts consume
the XML envelope, they will depend on its exact field names, so the output shape
must be stable from day one.

## Alternatives

**Built-in tools (`BuiltinTool` impls in `jp_llm`).** Moving conversation-query
logic into `jp_workspace` and running the tools in-process is the cleaner
long-term architecture, but it requires widening the `BuiltinTool` trait to
accept a workspace handle (or wrapping `Workspace` in `Arc`) and lifting the
list and grep cores out of `jp_cli`.
That refactoring is worth doing once the tool shape is validated.

**Direct shell-out, no Rust wrapper.** Having tool TOMLs invoke `jp
conversation` directly is cheaper to build, but it couples the LLM-facing
contract to whatever `jp conversation` happens to emit.
Rejected to avoid that Hyrum's Law trap.

**Loopback MCP server.** Running a long-lived MCP server inside `jp` to expose
conversation queries is unnecessary for a single-user CLI when the existing
local tool infrastructure suffices.

**Pure-core extraction without wrappers.** Refactoring
`cmd/conversation/{ls,grep,print}.rs` to expose pure library functions before
wrapping them is the right boundary, but a much larger change.
This RFD ships the wrappers first; pure-core extraction follows if the tools
graduate.

## Non-Goals

**Conversation summaries.** Adding a one-line summary per entry in
`conversation_list` requires a separate generation, storage, and invalidation
pipeline, and is deferred to a separate RFD.

**Subtree scoping.** [RFD 051] handles the sub-agent case where the LLM only
sees descendants of its current conversation.
The tools here cover workspace-wide recall; the two sets can coexist under
different tool names, and this RFD does not unify them.

**Mutating past conversations.** These tools do not edit, archive, or delete
conversations.
The assistant can read history, while modifications remain user-driven via the
CLI.

**Cross-workspace access.** The tools only access the current workspace, not
other workspaces on the machine.

**Shipping to other JP users.** The tools are workspace-only at first.
Graduating them to an MCP server or built-in tools will be decided once the
interface has proven useful.

## Risks and Open Questions

**Active-conversation handling.** Running `conversation_grep` across all
conversations matches content in the active conversation, which the LLM already
sees in its thread.
The options are to (a) include it with a flag in the conversation context, (b)
exclude it by default with an opt-in parameter, or (c) pass it through
unmodified and let the LLM ignore duplicates.
This design **excludes the active conversation by default** and adds an
`include_current` boolean parameter to both `grep` and `read`.
The wrapper reads `{{context.conversation_id}}` to identify the current
conversation, which depends on [RFD 040].
Until [RFD 040] is implemented, excluding the active conversation is not
possible, which does not block initial implementation.

**Output shape stability.** The XML envelope's field names form a contract.
The wrapper's module documentation will specify the output shape, and changes to
field names will be treated as breaking changes requiring coordinated tool
definition updates.

**Per-event-kind serialization.** The wrapper translates raw `ConversationEvent`
JSON into the LLM-facing XML shape, and tool call arguments are themselves
user-defined JSON.
To avoid raw JSON nested in XML, the wrapper emits tool call arguments as a
fenced JSON code block inside the XML element, matching how `git_log` and other
tools embed structured fields.

**Sanitization races.** Because the wrapper spawns a child `jp` process, two
concurrent `jp` processes load the same workspace, and workspace loading runs
sanitization that may rewrite on-disk state.
Concurrent loads are safe in practice, and this assumption is recorded here so
it can be re-checked if storage behavior changes.

## Implementation Plan

**Phase 1: CLI JSON contracts.** Standalone bug fixes, reviewable independently.

- Convert `jp conversation ls --format=json` to `snake_case` keys.
- Add `--format=json` to `jp conversation print`, emitting filtered events.
- Add tests covering both shapes.

**Phase 2: Wrappers and tool TOMLs.** Depends on Phase 1.

- Add `.config/jp/tools/src/conversation/{list,grep,read}.rs`.
- Add `.jp/mcp/tools/conversation/{list,grep,read}.toml`.
- Add unit tests using the `ProcessRunner` mock pattern from `git_log_tests.rs`.
- Pick defaults for size caps suited to LLM context budgets and document them in
  the wrapper.

**Phase 3: Persona enablement.** Depends on Phase 2.

- Add a skill file (`.jp/config/skill/conversation-access.toml`) that enables
  the three tools.
- Extend the relevant personas (`architect`, `committer`, and others) by adding
  the skill to their `extends`.
- Leave `dev` opt-in for now.

No phase changes JP's shipped surface; everything is workspace-local.

## References

- [RFD 040]: Hidden conversations and `conversation_id` in tool context, needed
  for active-conversation exclusion.
- [RFD 051]: Sub-agent workflows, which defines tools with the same names but a
  subtree-scoped contract; this RFD is the whole-workspace variant.
- [RFD 074]: Eager loading with command-declared data requirements, relevant for
  any future pure-core extraction of conversation queries.

[RFD 040]: ../040-hidden-conversations-and-tool-context.md
[RFD 051]: ../051-sub-agent-workflows-with-local-tools.md
[RFD 074]: ../074-eager-loading-with-command-declared-data-requirements.md
