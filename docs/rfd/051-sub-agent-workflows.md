# RFD 051: Sub-Agent Workflows with Local Tools

- **Status**: Discussion
- **Category**: Guide
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-03-08

## Summary

This guide walks through building sub-agent capabilities in JP using local tool
definitions and JP's configuration system. A main agent delegates scoped tasks —
research, planning, code review — to sub-agents running cheaper models in
separate conversations. The entire workflow is built from existing features:
local tools, `--cfg` overlays ([RFD 038]), `--non-interactive` mode ([RFD 049]),
conversation trees ([RFD 039]), and hidden conversations ([RFD 040]).

No changes to JP's agent loop are required.

## Why sub-agents?

Using a frontier model (e.g. Claude Opus) for an entire conversation is
expensive. A typical agent task — "refactor error handling in jp_llm" — benefits
from a research-plan-implement workflow: research the codebase on a cheaper
model, produce a plan, then execute. Manually orchestrating these phases means
switching models or starting new conversations between steps.

Sub-agents let the main agent orchestrate this itself. The frontier model
delegates research to a cheaper sub-agent, receives a condensed summary,
delegates plan authoring to another sub-agent, then implements from the finished
plan. The main agent's context contains only summaries and artifacts — never the
thousands of tokens of raw file content the research phase consumed.

Sub-agents also handle research needs that arise mid-implementation. Three
cycles into executing the plan, the agent discovers that changing `Error`
variants requires understanding how the retry module pattern-matches on them. It
delegates that investigation to a sub-agent and receives a 500-token summary,
rather than reading the files directly and polluting its own context.

### Design goals

| Goal                  | Description                              |
|-----------------------|------------------------------------------|
| **No special-casing** | Sub-agents are tools, not an agent loop  |
|                       | feature                                  |
| **Composable**        | Sub-agents can themselves spawn          |
|                       | sub-agents                               |
| **Config-driven**     | Model, tools, and behavior controlled    |
|                       | via `--cfg`                              |
| **Observable**        | Sub-conversations are real conversations |
|                       | on disk                                  |
| **Scoped**            | Tools can only access the current        |
|                       | conversation's subtree                   |

## What we're building

Four tools that expose JP's CLI as capabilities the LLM can use:

| Tool                    | Maps to                 | Purpose                                |
|-------------------------|-------------------------|----------------------------------------|
| `jp_query`              | `jp query`              | Delegate a task to a sub-agent         |
| `jp_conversation_list`  | `jp conversation ls`    | Discover existing sub-conversations    |
| `jp_conversation_print` | `jp conversation print` | Read back a sub-conversation's history |
| `jp_conversation_grep`  | `jp conversation grep`  | Search across sub-conversation content |

Each tool enforces a security boundary: it can only operate on conversations
that are descendants of the current conversation. The LLM cannot read, search,
or continue arbitrary user conversations.

## Demo

Here's how a main agent uses these tools in practice. The main agent (running
Opus) is asked to refactor error handling. It delegates research to a cheaper
model:

```js
user: "Refactor error handling in jp_llm to use thiserror"

main_agent: jp_query(
  config: "agent/researcher",
  query: "Find all error types in jp_llm. For each, list its variants, \
          trait impls, and where it's constructed and matched on."
)
```

```xml
<response conversation_id="jp-c19283746501">
jp_llm defines two error types:

`Error` (crates/jp_llm/src/error.rs):
- 8 variants: UnknownModel, Auth, RateLimit, ...
- Implements std::error::Error manually
- Has From impls for reqwest::Error, serde_json::Error, io::Error
- Constructed in provider modules, matched in retry.rs and stream/chain.rs

`StreamError` (crates/jp_llm/src/stream.rs):
- 3 variants for mid-stream failures
- Matched in turn_loop.rs for retry decisions
</response>
```

The sub-agent ran Sonnet, read 5 files, ran 2 greps, and returned a 500-token
summary. The main agent processed only that summary.

Three cycles into implementing, the main agent hits an unexpected pattern in the
retry module. Rather than reading the files itself, it continues the existing
sub-conversation:

```js
main_agent: jp_query(
  id: "jp-c19283746501",
  query: "How does retry.rs decide which Error variants are retryable? \
          Show the match arms."
)
```

```xml
<response conversation_id="jp-c19283746501">
retry.rs matches on Error variants in `is_retryable()`:
- RateLimit => always retry (with backoff from header)
- Timeout => retry up to 3 times
- Connection => retry up to 2 times
- All others => not retryable
</response>
```

The sub-agent had its full prior context. The main agent received only the new
answer. Later, the main agent can discover what sub-conversations exist:

```js
main_agent: jp_conversation_list()
```

```json
[
  {
    "id": "jp-c19283746501",
    "title": "Research error types in jp_llm",
    "events_count": 12
  },
  {
    "id": "jp-c19283746502",
    "title": "Research thiserror migration patterns",
    "events_count": 8
  }
]
```

And search across them:

```js
main_agent: jp_conversation_grep(pattern: "is_retryable")
```

```txt
jp-c19283746501: retry.rs matches on Error variants in `is_retryable()`:
```

Across this entire interaction, the main agent's context accumulated roughly
1,500 tokens of sub-agent summaries. The sub-agents collectively read over 20
files and ran multiple greps — work that would have cost the main agent 10,000+
tokens of raw file content in its context window. The main agent never saw a
line of source code it didn't need for implementation.

Each sub-conversation is a real conversation on disk. The main agent can return
to them hours later, continue a line of research, or search across all of them
for a half-remembered detail. The sub-agents are disposable — the knowledge they
produced is not.

## Building the tools

### `jp_query`

The core delegation tool. Creates a new child conversation (or continues an
existing one) running a sub-agent with a specific configuration profile.

#### Tool definition

```toml
# jp_query.toml
name = "jp_query"
run = "unattended"
source = "local"

description = """
Delegate a task to a sub-agent assistant. The sub-agent runs in a separate
conversation with its own model and tools, and returns a summary.

Use this for tasks that require reading multiple files, multi-step research,
or understanding code structure. For simple lookups (a single file read, a
single grep), use the tool directly instead of delegating.
"""

[parameters.config]
type = "string"
required = true
description = "Agent profile to use."
enum = ["agent/researcher"]

[parameters.query]
type = "string"
required = true
description = "The task or question for the sub-agent."

[parameters.id]
type = "string"
description = "Conversation ID to continue, or 'new' for a fresh sub-conversation."
default = "new"

[parameters.overrides]
type = "array"
items.type = "string"
description = "Config overrides as key=value pairs. Only assistant.model is allowed."
```

#### Command template

The tool runs a shell script that handles both the "new" and "continue" modes:

```sh
#!/usr/bin/env bash
set -euo pipefail

CONVERSATION_ID="{{context.conversation_id}}"
CONFIG="{{tool.arguments.config}}"
QUERY="{{tool.arguments.query}}"
SUB_ID="{{tool.arguments.id}}"

# Validate overrides — only assistant.model is allowed.
OVERRIDES=()
{{#for override in tool.arguments.overrides}}
if [[ "{{override}}" != assistant.model=* ]]; then
  echo "Error: only assistant.model overrides are allowed, got: {{override}}"
  exit 1
fi
OVERRIDES+=(--cfg "{{override}}")
{{/for}}

if [ "$SUB_ID" = "new" ]; then
  # Create a child conversation with an isolated config.
  # --cfg=NONE resets all config to defaults, then the agent profile
  # provides a self-contained configuration.
  #
  # `conversation fork --last=0` creates a blank child of the current
  # conversation and prints its ID to stdout (RFD 050).
  SUB_ID=$(jp conversation fork "$CONVERSATION_ID" \
    --last=0 \
    --cfg=NONE \
    --cfg=".jp/config/${CONFIG}.toml" \
    "${OVERRIDES[@]}" \
    --hidden)

  # Send the query to the new conversation.
  # --no-activate prevents this from becoming the user's active conversation.
  # --root-id constrains the conversation to the current subtree.
  OUTPUT=$(jp query \
    --non-interactive \
    --id="$SUB_ID" \
    --no-activate \
    --root-id="$CONVERSATION_ID" \
    "${OVERRIDES[@]}" \
    "$QUERY" 2>/dev/null)
else
  # Continue the existing sub-conversation.
  # No --cfg=NONE here — the conversation carries its own config.
  # --root-id ensures the LLM-provided ID is a descendant.
  OUTPUT=$(jp query \
    "${OVERRIDES[@]}" \
    --non-interactive \
    --id="$SUB_ID" \
    --no-activate \
    --root-id="$CONVERSATION_ID" \
    "$QUERY" 2>/dev/null)
fi

echo "<response conversation_id=\"${SUB_ID}\">"
echo "$OUTPUT"
echo "</response>"
```

Key design decisions in this script:

- **`--cfg=NONE` on creation only.** This resets all config to defaults before
  applying the agent profile, creating a hard security boundary. The agent
  config file must be fully self-contained — it specifies its own model, system
  prompt, and tool set. Changes to the user's workspace `config.toml` never leak
  into sub-agent behavior.

- **No `--cfg=NONE` on continuation.** Once a conversation is created, its
  config is stored in the stream. Re-applying `NONE` would wipe those settings.
  Continuation operates within the config established at creation time.

- **`conversation fork --last=0` for creation.** The script uses `conversation
  fork` ([RFD 050]) to create a blank child conversation and capture its ID.
  `--last=0` copies zero turns, producing an empty child.

- **`--root-id` for descendant validation.** The "continue" path passes
  `--root-id={{context.conversation_id}}`. JP enforces that the target
  conversation is a descendant of the root — if the LLM provides an ID that
  escapes the subtree, the command fails with a hard error. No shell-level
  validation needed.

- **`--no-activate`.** Sub-agent conversations should not become the user's
  active conversation. The `--no-activate` flag ([RFD 050]) persists the
  conversation without switching to it.

- **Override allowlist.** The `overrides` parameter accepts `key=value` pairs,
  but the script rejects anything that isn't `assistant.model=*`. This lets the
  main agent pick a different model for a sub-agent without opening up arbitrary
  config changes. The allowlist can be extended in the tool definition's shell
  script as needs evolve.

- **Response wrapping.** The script wraps the sub-agent's stdout with a
  `<response>` tag containing the conversation ID, so the main agent can
  reference it in follow-up calls.

- **`2>/dev/null` on `jp query`.** Chrome (progress indicators, tool headers)
  goes to stderr. Discarding it keeps the captured output clean.

### `jp_conversation_list`

Lets the main agent discover existing sub-conversations so it can reuse prior
research instead of repeating it.

```toml
# jp_conversation_list.toml
name = "jp_conversation_list"
run = "unattended"
source = "local"

description = """
List sub-conversations created by this agent. Returns conversation IDs, titles,
and event counts. Use this to find existing research before starting new
sub-agent tasks.
"""
```

No parameters — the tool is always scoped to the current conversation's
children.

#### Command template

```sh
jp conversation ls \
  --root="{{context.conversation_id}}" \
  --hidden \
  --format=json
```

The output is JSON, which the LLM can parse to find conversation IDs and titles.
The `--root` flag (from [RFD 039]) restricts the listing to descendants of the
current conversation. The `--hidden` flag (from [RFD 040]) includes
sub-conversations that were created with `--hidden`.

### `jp_conversation_print`

Lets the main agent read back a sub-conversation's history. Use sparingly — this
can inject many tokens into the main agent's context, which is exactly what
sub-agents are meant to avoid. Prefer `jp_conversation_grep` for targeted
lookups, or `jp_query` to ask the sub-agent a follow-up question.

The primary use case is reviewing what a sub-agent did (debugging), or
extracting a specific artifact from a multi-turn sub-conversation.

```toml
# jp_conversation_print.toml
name = "jp_conversation_print"
run = "unattended"
source = "local"

description = """
Print a sub-conversation's history. This returns the full conversation content,
which can be large. For targeted lookups, prefer jp_conversation_grep. For
asking the sub-agent a follow-up question, use jp_query with the conversation
ID instead.
"""

[parameters.id]
type = "string"
required = true
description = "Conversation ID to print. Valid IDs can be found with `jp_conversation_list`."

[parameters.last]
type = "integer"
description = "Print only the last N turns. Recommended to limit context size."
```

#### Command template

```sh
#!/usr/bin/env bash
set -euo pipefail

{{#if tool.arguments.last}}
jp conversation print "{{tool.arguments.id}}" \
  --root-id="{{context.conversation_id}}" \
  --last={{tool.arguments.last}}
{{else}}
jp conversation print "{{tool.arguments.id}}" \
  --root-id="{{context.conversation_id}}"
{{/if}}
```

### `jp_conversation_grep`

Lets the main agent search across sub-conversation content without printing
entire conversations. This is the preferred way to find specific information in
prior research.

```toml
# jp_conversation_grep.toml
name = "jp_conversation_grep"
run = "unattended"
source = "local"

description = """
Search through sub-conversation history for matching content. Returns matching
lines with conversation IDs. Use this to find specific information across prior
research without printing full conversations.
"""

[parameters.pattern]
type = "string"
required = true
description = "Search pattern (plain text, case-insensitive)."

[parameters.id]
type = "string"
description = "Search within a specific sub-conversation. If omitted, searches all sub-conversations."
```

#### Command template

```sh
#!/usr/bin/env bash
set -euo pipefail

# --root-id scopes the search to descendants of the current conversation.
# JP enforces this natively — no shell-level validation needed.

{{#if tool.arguments.id}}
jp conversation grep \
  --id="{{tool.arguments.id}}" \
  --root-id="{{context.conversation_id}}" \
  -i "{{tool.arguments.pattern}}"
{{else}}
jp conversation grep \
  --root="{{context.conversation_id}}" \
  --hidden \
  -i "{{tool.arguments.pattern}}"
{{/if}}
```

> **Note:** The `--root` flag on `conversation grep` (matching `conversation ls`
> from [RFD 039]) is needed for the "search all descendants" case. See
> [Prerequisites](#prerequisites).

## Configuring sub-agents

A sub-agent's behavior is controlled entirely through a config file. Because the
tool uses `--cfg=NONE` before loading the profile, the config file must be
**fully self-contained** — it cannot rely on inheriting settings from the
workspace's `config.toml`.

This is the primary security property of the system: **the agent config file
_is_ the security policy.** Reviewing what a sub-agent can do means reading one
file.

### Researcher profile

A read-only agent with a cheaper model and no write tools:

```toml
# jp_agent_researcher.toml
[assistant]
model = "anthropic/claude-sonnet-4-20250514"

[[assistant.system_prompt_sections]]
content = """
You are a research assistant. Your job is to investigate codebases, find
relevant code, and produce clear, concise summaries. Focus on facts: what
exists, where it is, how it's structured, and how it's used.

Do not write or modify code. Do not suggest changes. Report what you find.
"""

# Enable only read-only tools.
[conversation.tools.fs_read_file]
source = "local"
run = "unattended"
command = "..."

[conversation.tools.fs_grep_files]
source = "local"
run = "unattended"
command = "..."

[conversation.tools.fs_list_files]
source = "local"
run = "unattended"
command = "..."

# Suppress chrome so stdout is clean for capture.
[style]
reasoning.display = "hidden"
tool_call.show = false
```

This config does not enable `jp_query`, preventing the researcher from spawning
its own sub-agents. It does not enable any write tools (`fs_create_file`,
`fs_modify_file`, `cargo_check`, `git_commit`, etc.).

### Planner profile

An agent that can read code and produce plans, but still cannot modify anything:

```toml
# jp_agent_planner.toml
[assistant]
model = "anthropic/claude-sonnet-4-20250514"

[[assistant.system_prompt_sections]]
content = """
You are a planning assistant. Given research summaries and a goal, produce a
step-by-step implementation plan. For each step, specify:
- Which file(s) to change
- What the change is
- Why the change is needed
- Any ordering dependencies between steps

Be specific enough that another developer could execute the plan without
additional research.
"""

[conversation.tools.fs_read_file]
source = "local"
command = "..."

[conversation.tools.fs_grep_files]
run = "unattended"
source = "local"
command = "..."

[conversation.tools.fs_list_files]
run = "unattended"
source = "local"
command = "..."

[style]
reasoning.display = "hidden"
tool_call.show = false
```

### The `NONE` boundary

The `--cfg=NONE` flag ([RFD 038]) resets all configuration to defaults before
applying the agent profile. This creates a hard isolation boundary:

- The agent config must explicitly set `assistant.model`. If it doesn't, config
  validation fails — a loud, clear error.
- The agent config must explicitly enable each tool it needs. Tools enabled in
  the user's workspace config are not inherited.
- Changes to the workspace `config.toml` never affect sub-agent behavior. The
  user can add new tools, change models, or tweak settings without accidentally
  granting sub-agents new capabilities.

After `NONE`, `config_load_paths` is empty (`[]`), so the agent profile must be
loaded by direct path (`.jp/config/agent/researcher.toml`), not by load-path
name (`agent/researcher`). The command template handles this mapping — the LLM
selects from the `config` enum, and the shell script translates to a file path.

### Restricting capabilities

All capability restrictions are enforced through the agent config file and the
tool's shell script. No special support from JP core is needed.

| Control              | Mechanism                                              |
|----------------------|--------------------------------------------------------|
| No recursion         | Agent config does not enable `jp_query`                |
| Read-only tools      | Agent config only enables read tools                   |
| Model selection      | Agent config sets `assistant.model`                    |
| Allowed profiles     | `config` parameter's `enum` lists valid profiles       |
| Allowed overrides    | Shell script validates override keys against allowlist |
| Conversation scoping | `--root-id` flag enforces descendant constraint in JP  |

## Security: conversation scoping

The tools enforce that the LLM can only interact with conversations in its own
subtree. This is the answer to "what stops the LLM from reading my unrelated
conversations?"

The enforcement works at two levels:

**Command template level.** The shell script controls the actual `jp`
invocation. The LLM provides parameter values (conversation IDs, queries), but
the template structure is fixed. The LLM cannot inject flags or change the
command.

**`--root-id` constraint.** Tools that accept a conversation ID from the LLM
pass `--root-id={{context.conversation_id}}` to the underlying `jp` command. JP
enforces that the target conversation is a descendant of the root — if it isn't,
the command fails with a hard error. This validation happens inside JP, not in
shell script, so it is atomic and cannot be bypassed by malformed input.

| Tool                    | Scoping mechanism                       |
|-------------------------|-----------------------------------------|
| `jp_query` (new)        | `conversation fork`                     |
|                         | `{{context.conversation_id}} --last=0`  |
| `jp_query` (continue)   | `--id=<llm-provided>`                   |
|                         | `--root-id={{context.conversation_id}}` |
| `jp_conversation_list`  | Hardcodes                               |
|                         | `--root={{context.conversation_id}}`    |
| `jp_conversation_print` | `--root-id={{context.conversation_id}}` |
| `jp_conversation_grep`  | `--root-id={{context.conversation_id}}` |
|                         | or `--root=`                            |

The `config` parameter's `enum` is the second security boundary — it restricts
which agent profiles are available. A free-form config path would let the LLM
attempt to load arbitrary config files. The enum prevents this.

The `overrides` parameter's shell-side validation is the third boundary. Only
allowlisted keys (currently `assistant.model=*`) are accepted. The LLM cannot
override tool settings, system prompts, or other sensitive config via overrides.

## Workflows

### Research-plan-implement

The sub-agent pattern supports a phased workflow where the main agent
orchestrates each step:

1. **Research.** Delegate codebase investigation to a researcher sub-agent. The
   sub-agent reads files, runs greps, and returns a structured summary.

2. **Plan.** Feed the research summary to a planner sub-agent (or the main agent
   itself) to produce a step-by-step implementation plan.

3. **Implement.** The main agent executes the plan. When it hits a knowledge
   gap, it delegates another research task rather than reading files directly.

The main agent's context stays lean throughout: it sees summaries and plans,
never raw file contents from the research phase.

### Mid-task research

Research needs often arise during implementation. The main agent can delegate
these to a sub-agent at any point:

```txt
main_agent > [modifying error.rs, realizes it needs to understand retry logic]
           > jp_query(config: "agent/researcher", query: "How does retry.rs pattern-match on Error variants?")
           < [500-token summary]
           > [continues implementation with the new knowledge]
```

The main agent can also check whether a prior sub-conversation already covered
the topic:

```txt
main_agent > jp_conversation_grep(pattern: "retry")
           < jp-c19283746501: retry.rs matches on Error variants in `is_retryable()`:
           > jp_query(config: "agent/researcher", id: "jp-c19283746501", query: "What about the backoff calculation?")
```

### Parallel sub-agents

JP's `ToolCoordinator` runs tool calls in the same batch concurrently. If the
main agent calls `jp_query` multiple times in one response, each creates an
independent sub-conversation running in parallel:

```txt
main_agent > jp_query(config: "agent/researcher", query: "Analyze error handling in jp_llm")
           > jp_query(config: "agent/researcher", query: "Analyze retry logic in jp_llm")
           < (both results returned when both complete)
```

The response wrapping includes the conversation ID, so the main agent can track
and reference each result independently.

#### Future: stateful sub-agents

With [RFD 009] and [RFD 037], `jp_query` could adopt the stateful tool protocol
for asynchronous execution. The main agent would spawn sub-agents, continue with
other work, and await results when needed. This is a future enhancement; the
synchronous model delivers the core value.

## Tradeoffs

**Process overhead.** Each sub-agent spawns a new `jp` process: argument
parsing, config resolution, and MCP server connection setup. For sub-agents
doing multiple LLM turns, this is small relative to API latency. For trivial
lookups, calling a tool directly is faster.

**Conversation storage growth.** Sub-conversations are real conversations on
disk. An aggressive main agent can create many. Mitigations: set `--tmp` to
auto-expire ephemeral sub-conversations, and use `--hidden` ([RFD 040]) to keep
them out of `jp conversation ls`. The tree structure from [RFD 039] keeps them
organized under the parent, and removing the parent with `--cascade` cleans up
the entire subtree.

**Delegation quality.** The main agent must formulate clear, scoped tasks. Vague
delegations ("look at the codebase") waste tokens. Overly narrow tasks ("read
line 42 of error.rs") are cheaper as direct tool calls. The system prompt should
guide when to delegate — the tool description already says "for tasks that
require reading multiple files."

Whether current frontier models reliably follow this guidance needs validation.

**`conversation print` token cost.** Printing a full sub-conversation injects
potentially thousands of tokens into the main agent's context. The tool
description warns against this and recommends `jp_conversation_grep` or
`jp_query` (follow-up) instead. `--last=1` limits the damage for cases where the
main agent only needs the most recent response.

**JSON output key stability.** The current `jp conversation ls --format=json`
output uses display-oriented keys (`"ID"`, `"#"`, `"Activity"`). These are the
table column headers serialized to JSON, not a stable API contract. The shell
scripts in this guide use these keys; they may change to snake_case (`id`,
`events_count`) when a proper structured output format is defined.

## Prerequisites

This guide assumes the following RFDs are implemented:

- [RFD 038] — `--cfg=NONE` for isolated config and config load paths.
- [RFD 039] — conversation trees (`conversation fork` creates children) and
  `--root` flag on `conversation ls`.
- [RFD 040] — hidden conversations and `conversation_id` in tool context.
- [RFD 048] — output channel separation (stdout for assistant output, stderr
  for chrome) so sub-agent output can be cleanly captured.
- [RFD 049] — `--non-interactive` mode for sub-agents running without a
  terminal.
- [RFD 050] — `jp conversation new`, `--no-activate`, and `--root-id` for
  scripting ergonomics.

Additionally:

- `conversation grep` needs a `--root` flag (mirroring `conversation ls`) to
  scope searches to a subtree.
- `conversation print` and `conversation grep` need `--root-id` support for
  descendant validation.
- `conversation fork` needs to print the new conversation ID to stdout and
  accept config options (`--cfg`, `--hidden`) per [RFD 050].

## References

- [RFD 038: Config Inheritance][RFD 038] — `--cfg=NONE` and config keywords.
- [RFD 039: Conversation Trees][RFD 039] — tree-structured conversation
  hierarchy and `--root` scoping.
- [RFD 040: Hidden Conversations and Tool Context][RFD 040] — `hidden` flag and
  `conversation_id` in tool execution context.
- [RFD 048: Four-Channel Output Model][RFD 048] — stdout/stderr separation for
  clean sub-agent output capture.
- [RFD 049: Non-Interactive Mode][RFD 049] — detached prompt policies for
  sub-agents running without a terminal.
- [RFD 050: Scripting Ergonomics][RFD 050] — `conversation new`,
  `--no-activate`, and `--root-id`.
- [RFD 009: Stateful Tool Protocol][RFD 009] — future: handle registry for async
  sub-agents.
- [RFD 037: Await Tool][RFD 037] — future: synchronization for parallel stateful
  sub-agents.

[RFD 009]: 009-stateful-tool-protocol.md
[RFD 037]: 037-await-tool-for-stateful-handle-synchronization.md
[RFD 038]: 038-config-inheritance-for-conversations.md
[RFD 039]: 039-conversation-trees.md
[RFD 040]: 040-hidden-conversations-and-tool-context.md
[RFD 048]: 048-four-channel-output-model.md
[RFD 049]: 049-non-interactive-mode-and-detached-prompt-policy.md
[RFD 050]: 050-scripting-ergonomics-for-conversation-management.md
