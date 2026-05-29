# RFD D39: Conversation Access Tools for the Assistant

- **Status**: Draft
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-05-21

## Summary

Add three tools the assistant can call to access historical conversations in the
current workspace: `conversation_list` (paginated metadata), `conversation_grep`
(content search), and `conversation_read` (full content of a specific
conversation).
The tools are Rust wrappers under `.config/jp/tools/src/conversation/` that
shell out to `jp conversation` and present a stable JSON contract to the LLM.
Scope is workspace-only and maintenance-tooling-only; if the pattern proves
useful, it can graduate to built-in tools or a shipped MCP server later.

## Motivation

The assistant has no recall of past conversations.
Every session starts blank.
A user working on a long-running project frequently refers back to earlier
conversations — "we discussed retry semantics last week", "what model did we
settle on?" — and has to copy-paste or re-explain.
The assistant can help itself if it can list, search, and read its own history.

Three primitives cover the realistic use cases:

- **List.** Discover what conversations exist, sorted by recency, with titles
  and basic metadata.
- **Grep.** Search content across one, many, or all conversations to find where
  a topic was discussed.
- **Read.** Pull the full content of a specific conversation, optionally
  narrowed to specific turns or event kinds.

The infrastructure to deliver this is already in place.
The CLI exposes `jp conversation ls`, `jp conversation grep`, and `jp
conversation print` with rich filtering.
The workspace's `jp-tools` binary (`.config/jp/tools/`) is the established
pattern for exposing CLI capabilities to the assistant — `git_log`,
`fs_grep_files`, `cargo_check`, and others all use it.
Wrapping `jp conversation` the same way fits the existing shape and avoids new
architectural commitments.

## Design

### Overview

Three tools, defined as `local` tool sources in TOML, calling the project's
`jp-tools` binary, which shells out to `jp conversation` with `--format=json`,
parses the result, applies LLM-facing filtering, and emits an XML-style response
consistent with the other tools in `jp-tools`.

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

The LLM-facing contract — the tool name, its parameters, its output shape — is
defined entirely in the Rust wrapper.
The CLI's JSON output is a private interface between two binaries owned by this
repository.

### Scope and trust

The tools operate on the entire workspace.
JP treats the workspace as a single trust domain — any tool with filesystem
access can already read the conversation directory on disk.
Exposing the same data via structured tools surfaces it clearly rather than
leaving the LLM to grep the raw JSON files.

This differs from [RFD 051]'s sub-agent tools, which scope to the current
conversation's subtree.
The two are complementary: 051 is about isolating ephemeral sub-agents; this RFD
is about recalling user-driven history.

### Tool APIs

All three tools are configured with `run = "unattended"` (read-only operations,
no user prompt needed) and `enable = "explicit"` (opt-in per persona).

#### `conversation_list`

Parameters:

| Name             | Type   | Default    | Description                                   |
| ---------------- | ------ | ---------- | --------------------------------------------- |
| `limit`          | int    | `20`       | Max number of conversations to return.        |
| `offset`         | int    | `0`        | Skip the first N (after sorting).             |
| `sort`           | enum   | `activity` | `created`, `activity`, or `updated`.          |
| `descending`     | bool   | `true`     | Most recent first by default.                 |
| `archived`       | bool   | `false`    | Return archived conversations instead.        |
| `title_contains` | string | `null`     | Substring filter on title (case-insensitive). |

Output: an XML envelope listing matching conversations with snake\_case keys —
`id`, `title`, `events_count`, `created_at`, `last_event_at`, `archived_at`,
`expires_at`.
The total count and the active offset are included so the LLM can paginate.

#### `conversation_grep`

Parameters:

| Name          | Type            | Default | Description                                       |
| ------------- | --------------- | ------- | ------------------------------------------------- |
| `pattern`     | string          | —       | Required. Substring, case-insensitive by default. |
| `ignore_case` | bool            | `true`  | Toggle case sensitivity.                          |
| `ids`         | array of string | `null`  | Restrict to specific conversations.               |
| `scopes`      | array of string | all     | `chat`, `tool`, `title`, or any concrete scope.   |
| `context`     | int             | `0`     | Context lines around each match.                  |
| `limit`       | int             | `50`    | Max number of hits returned.                      |

Output: an XML envelope of hits.
Each hit carries `id`, `scope`, `text`, `is_match`, and the conversation's title
for context.
The wrapper truncates long lines (the CLI already does this for human output;
the wrapper applies its own cap suited to LLM context budgets).

#### `conversation_read`

Parameters:

| Name      | Type            | Default                                               | Description                                   |
| --------- | --------------- | ----------------------------------------------------- | --------------------------------------------- |
| `id`      | string          | —                                                     | Required. Conversation to read.               |
| `turn`    | int             | `null`                                                | 1-based turn index.                           |
| `last`    | int             | `null`                                                | Last N turns. Mutually exclusive with `turn`. |
| `include` | array of string | `["chat", "reasoning", "tool_calls", "tool_results"]` | Which event kinds to include.                 |

If neither `turn` nor `last` is set, all turns are returned.
The wrapper refuses requests whose unfiltered output would exceed a configured
size cap (initially: the wrapper's own constant, no config knob), and returns a
clear-error suggesting `last` or `turn`.

Output: an XML envelope of turns, each containing the included events with typed
fields (`event_kind`, `timestamp`, `content`, tool call name and arguments where
applicable).

### CLI changes

Two small changes to the CLI are part of this work and stand alone as bug fixes:

1. **`jp conversation ls --format=json` emits snake\_case keys** (`id`,
   `events_count`, `last_event_at`, …) instead of display-derived keys (`"ID"`,
   `"#"`, `"Activity"`).
   The current JSON shape is the table-header row serialized verbatim; that was
   never an intentional contract.
   [RFD 051] already calls this out as a prerequisite for its tools.

2. **`jp conversation print --format=json` emits the underlying event stream**
   for the selected turn window.
   Implementation is largely "serialize `ConversationEvent`s within the window"
   — `events.json` is already this shape on disk, minus internal events the LLM
   doesn't need.
   The default mode emits verbatim events; per-event-type filtering lives in the
   wrapper.

Both changes are additive.
The current `--format=json` output keys on `conversation ls` are not documented
as a contract, but renaming them is strictly an observable change — the wrapper
is the only known consumer.

### Wrapper layout

Following the existing `.config/jp/tools/src/git/`, `fs/`, `cargo/` modules:

```
.config/jp/tools/src/conversation.rs
.config/jp/tools/src/conversation/list.rs
.config/jp/tools/src/conversation/grep.rs
.config/jp/tools/src/conversation/read.rs
```

The top-level `lib.rs` already routes `s.starts_with("...")` patterns to module
runners.
A new branch is added:

```rust
s if s.starts_with("conversation_") => conversation::run(ctx, t).await,
```

Each module owns its parameter parsing, child-process invocation, JSON
deserialization, post-processing (filtering, truncation), and XML formatting.

Tool TOMLs live under `.jp/mcp/tools/conversation/{list,grep,read}.toml`,
mirroring `.jp/mcp/tools/git/`, `.jp/mcp/tools/fs/`, etc. They follow the same
`source = "local"` + `command = "just serve-tools {{context}} {{tool}}"` shape
as every other local tool.

### Persona enablement

New personas opt into the tools by adding a skill file (e.g.
`.jp/config/skill/conversation-access.toml`) that extends the relevant tool
TOMLs.
The default `dev` persona does not enable them — recall is a deliberate
capability, not a default.
The personas that benefit most (e.g. an `architect` doing long-running research,
a `committer` looking up prior decisions) opt in explicitly.

## Drawbacks

**Double process startup per call.** The wrapper spawns `jp-tools`, which then
spawns `jp conversation`.
Each tool call costs two process startups.
For a user-triggered LLM tool call, this is a few hundred milliseconds and not
in a hot loop; the LLM-side latency dominates.
Not worth optimizing.

**Token budget on read.** A naive call to `conversation_read` against a
thousand-turn conversation injects thousands of tokens.
The wrapper enforces a size cap and refuses with a hint; this is a defense, not
a guarantee.

**Maintenance-only scope.** `.config/jp/tools/` is this workspace's tooling, not
something other JP installations get.
Anyone wanting these capabilities outside this workspace has to either copy the
pattern, build their own MCP server, or wait for graduation to built-ins.
This is acceptable as a starting point — the goal is to validate the shape
before committing to a shipped feature.

**Hyrum's Law on the wrapper's output shape.** Once the LLM is consuming the XML
envelope, downstream personas and prompts will depend on its exact field names.
The shape needs to be stable from day one.

## Alternatives

**Built-in tools (`BuiltinTool` impls in `jp_llm`).** The clean architectural
choice — moves conversation-query logic into `jp_workspace`, makes the tools
in-process.
Costs: widening the `BuiltinTool` trait to accept a workspace handle (or moving
`Workspace` into `Arc`), and lifting grep/list cores out of `jp_cli` where they
currently live.
Worth doing eventually; not worth doing before the shape is validated.

**Direct shell-out, no Rust wrapper.** Tool TOMLs invoke `jp conversation`
directly.
Cheapest to deliver, but the LLM-facing contract becomes "whatever `jp
conversation` happens to emit" — which is exactly the Hyrum's Law trap the
wrapper exists to prevent.
Rejected.

**Loopback MCP server.** A long-lived MCP server inside `jp` exposing
conversation queries.
Overkill for a single-user CLI; the existing tool infrastructure is sufficient.

**Pure-core extraction without wrappers.** Refactor `cmd/conversation/{ls,
grep,print}.rs` to expose pure functions, then expose those via either built-ins
or wrappers.
Right boundary, much larger change.
Sequenced: this RFD ships first; pure-core extraction follows if the tools
graduate.

## Non-Goals

**Conversation summaries.** `conversation_list` would benefit from a one-line
summary per entry.
That requires a separate generation pipeline (storage, invalidation,
regeneration) and is deferred to a separate RFD.

**Subtree scoping.** [RFD 051] handles the sub-agent case where the LLM should
only see descendants of its current conversation.
The tools here are the broader "recall everything" variant.
The two can coexist with different tool names; this RFD does not unify them.

**Mutating past conversations.** No edit, archive, or delete via these tools.
The assistant can read history; modifications stay on the user's side via the
CLI.

**Cross-workspace access.** The tools only see the current workspace.
A user with multiple workspaces gets recall within each, not across them.

**Shipping to other JP users.** This is intentionally workspace-only at first.
Graduation requires either an MCP server or built-in tools, decided once we have
evidence the shape is right.

## Risks and Open Questions

**Active-conversation handling.** `conversation_grep` against "all
conversations" will match content in the active conversation — content the LLM
is already seeing in its thread.
Options: (a) include it, with a flag the LLM can set in the conversation
context; (b) exclude it by default, with an opt-in parameter; (c) pass it
through unmodified and let the LLM ignore duplicates.
Proposed: **exclude by default**, with an `include_current` boolean parameter on
both `grep` and `read`.
The wrapper reads `{{context.conversation_id}}` to identify the current
conversation, which depends on [RFD 040] being implemented.
Until then, no exclusion is possible — that's worth flagging but not blocking.

**Output shape stability.** The XML envelope's field names become a contract.
Proposal: document the shape in the wrapper's module docs and treat changes to
field names as breaking (require coordinated tool definition updates).

**Per-event-kind serialization.** The wrapper translates raw `ConversationEvent`
JSON into the LLM-facing shape.
Tool call arguments are themselves user-defined JSON; nested-JSON-in-XML is
ugly.
Proposed: emit tool call arguments as a JSON code fence inside the XML, matching
how `git_log` and others embed structured fields.

**Sanitization races.** Two concurrent `jp` processes load the same workspace.
Workspace load runs sanitization which may rewrite on-disk state.
The user has confirmed this is not an issue in practice, but the assumption is
documented here so it can be re-checked if storage behavior changes.

## Implementation Plan

**Phase 1: CLI JSON contracts.** Standalone bug fixes, reviewable independently.

- Convert `jp conversation ls --format=json` to snake\_case keys.
- Add `--format=json` to `jp conversation print`, emitting filtered events.
- Add tests covering both shapes.

**Phase 2: Wrappers and tool TOMLs.** Depends on Phase 1.

- Add `.config/jp/tools/src/conversation/{list,grep,read}.rs`.
- Add `.jp/mcp/tools/conversation/{list,grep,read}.toml`.
- Add unit tests using the existing `ProcessRunner` mock pattern from
  `git_log_tests.rs`.
- Pick LLM-friendly defaults for size caps; document them in the wrapper.

**Phase 3: Persona enablement.** Depends on Phase 2.

- Add a skill file (`.jp/config/skill/conversation-access.toml`) that enables
  the three tools.
- Extend the relevant personas (architect, committer, others) by adding the
  skill to their `extends`.
- Leave `dev` opt-in for now.

No phase changes JP's shipped surface; everything is workspace-local.

## References

- [RFD 040] — Hidden conversations and `conversation_id` in tool context.
  Needed for the active-conversation exclusion.
- [RFD 051] — Sub-agent workflows.
  Defines tools with the same names but a subtree-scoped contract; this RFD is
  the whole-workspace variant.
- [RFD 074] — Eager loading with command-declared data requirements.
  Relevant for any future pure-core extraction of conversation queries.

[RFD 040]: 040-hidden-conversations-and-tool-context.md
[RFD 051]: 051-sub-agent-workflows.md
[RFD 074]: 074-eager-loading-with-command-declared-data-requirements.md
