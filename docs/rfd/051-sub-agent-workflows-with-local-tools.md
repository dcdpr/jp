# RFD 051: Sub-Agent Workflows with Local Tools

- **Status**: Discussion
- **Category**: Guide
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-03-08
- **Requires**: [RFD 039], [RFD 040], [RFD 049], [RFD 050]
- **Summary**: Shows how to build sub-agent workflows using local tools, `--cfg`
  overlays, hidden conversations, and conversation trees.

## Summary

This guide walks through building sub-agent workflows in JP using local tool
definitions and configuration overlays.
A main agent delegates scoped tasks such as research, planning, and code review
to sub-agents running cheaper models in child conversations.
The workflow composes existing features without changing the agent loop: local
tools, `--cfg` overlays ([RFD 038]), `--non-interactive` mode ([RFD 049]),
conversation trees ([RFD 039]), and hidden conversations ([RFD 040]).

## Why sub-agents?

Running a frontier model (such as Claude Opus) for an entire conversation is
expensive.
A typical agent task, such as "refactor error handling in jp_llm", fits a
research-plan-implement workflow: research the codebase on a cheaper model,
produce a plan, and execute it.
Orchestrating those phases by hand requires switching models or starting new
conversations between steps.

Sub-agents let the main agent orchestrate this workflow itself.
The frontier model delegates research to a cheaper sub-agent, receives a
condensed summary, delegates plan authoring to another sub-agent, and implements
from the finished plan.
The main agent's context holds only summaries and plans, not the thousands of
tokens of raw file content read during research.

Sub-agents also handle questions that come up mid-implementation.
If the agent discovers partway through executing a plan that changing `Error`
variants requires checking how the retry module matches on them, it can delegate
that investigation to a sub-agent and receive a 500-token summary instead of
reading the files into its own context.

### Design goals

| Goal                  | Description                              |
| --------------------- | ---------------------------------------- |
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

## Local tools

Four local tools expose JP CLI commands to the LLM:

| Tool                    | Maps to                 | Purpose                                |
| ----------------------- | ----------------------- | -------------------------------------- |
| `jp_query`              | `jp query`              | Delegate a task to a sub-agent         |
| `jp_conversation_list`  | `jp conversation ls`    | Discover existing sub-conversations    |
| `jp_conversation_print` | `jp conversation print` | Read a sub-conversation's history      |
| `jp_conversation_grep`  | `jp conversation grep`  | Search across sub-conversation content |

Each tool is restricted to descendants of the current conversation, so the LLM
cannot read, search, or continue arbitrary user conversations.

## Demo

Suppose the main agent (running Opus) is asked to refactor error handling.
It delegates the initial codebase research to a cheaper model:

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

The sub-agent ran on Sonnet, read five files, ran two greps, and returned a
500-token summary, which is all the main agent adds to its context.

Three cycles into implementation, the main agent hits an unexpected pattern in
the retry module.
Rather than reading the files itself, it continues the existing
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

Because the sub-conversation retained its prior context, the sub-agent answers
directly and the main agent receives only the new reply.
Later, the main agent can list its sub-conversations:

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

Across this interaction, the main agent's context accumulated roughly 1,500
tokens of sub-agent summaries.
The sub-agents read over 20 files and ran multiple greps, keeping more than
10,000 tokens of raw file content out of the main agent's context window so it
only saw code needed for implementation.

Because each sub-conversation is persisted on disk, the main agent can return to
it hours later, continue a line of research, or search across prior findings for
a specific detail.

## Building the tools

### `jp_query`

`jp_query` is the delegation tool.
It creates a new child conversation or continues an existing one, running a
sub-agent under a specified configuration profile.

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

- **`--cfg=NONE` on creation only.** Resets all configuration to defaults before
  applying the agent profile, isolating the sub-agent from the workspace
  `config.toml`.
  The profile must define its own model, system prompt, and tool set.

- **No `--cfg=NONE` on continuation.** Once created, a conversation stores its
  configuration in its event stream.
  Re-applying `NONE` would wipe those settings, so continuation runs with the
  configuration established at creation time.

- **`conversation fork --last=0` for creation.** `conversation fork` ([RFD 050])
  creates a child conversation and prints its ID to stdout.
  `--last=0` copies zero turns, producing an empty child.

- **`--root-id` for descendant validation.** The continuation path passes
  `--root-id={{context.conversation_id}}`.
  JP verifies that the target conversation is a descendant of that root and
  fails with an error if the ID escapes the subtree, without needing shell-level
  validation.

- **`--no-activate`.** Persists the sub-conversation without switching the
  user's active conversation to it ([RFD 050]).

- **Override allowlist.** The `overrides` parameter accepts `key=value` pairs,
  and the script rejects anything other than `assistant.model=*`.
  This lets the main agent choose a different model for a sub-agent without
  permitting arbitrary configuration changes.
  Extend the allowlist in the script if other overrides are needed.

- **Response wrapping.** Wraps the sub-agent's stdout in a `<response>` tag
  containing the conversation ID so the main agent can reference it in follow-up
  calls.

- **`2>/dev/null` on `jp query`.** Chrome (progress indicators and tool headers)
  is written to stderr; discarding it keeps the captured output clean.

### `jp_conversation_list`

`jp_conversation_list` lets the main agent discover existing sub-conversations
so it can reuse prior research instead of repeating it.

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

The tool takes no parameters; it is always scoped to the current conversation's
subtree.

#### Command template

```sh
jp conversation ls \
  --root="{{context.conversation_id}}" \
  --hidden \
  --format=json
```

The command outputs JSON containing conversation IDs and titles.
`--root` ([RFD 039]) restricts the listing to descendants of the current
conversation, and `--hidden` ([RFD 040]) includes sub-conversations created with
`--hidden`.

### `jp_conversation_print`

`jp_conversation_print` reads back a sub-conversation's history.
Because printing a full transcript can inject many tokens into the main agent's
context, reserve this tool for debugging what a sub-agent did or extracting a
specific artifact from a multi-turn exchange.
Prefer `jp_conversation_grep` for targeted lookups and `jp_query` for follow-up
questions.

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

`jp_conversation_grep` searches across sub-conversation transcripts without
printing full histories, making it the preferred way to look up specific
findings from prior research.

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
> from [RFD 039]) is needed for the "search all descendants" case.
> See [Prerequisites](#prerequisites).

## Configuring sub-agents

A sub-agent's behavior is defined by its configuration profile.
Because `jp_query` passes `--cfg=NONE` before loading the profile, the file must
be **self-contained**: it cannot inherit settings from the workspace's
`config.toml`.

This makes the profile file the sub-agent's complete security policy.
Auditing what a sub-agent can do requires reading only that file.

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

This profile omits `jp_query`, preventing the researcher from spawning its own
sub-agents, and omits write tools (`fs_create_file`, `fs_modify_file`,
`cargo_check`, `git_commit`, etc.).

### Planner profile

An agent that can read code and produce plans, but cannot modify files:

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
applying the agent profile, creating an isolation boundary:

- The agent config must set `assistant.model` explicitly, or configuration
  validation fails with an error.
- The agent config must enable each tool it needs; tools enabled in the
  workspace config are not inherited.
- Changes to the workspace `config.toml` never affect sub-agent behavior.
  Adding tools, changing models, or adjusting workspace settings does not grant
  sub-agents new capabilities.

After `NONE`, `config_load_paths` is empty (`[]`), so the agent profile must be
loaded by file path (`.jp/config/agent/researcher.toml`) rather than by
load-path name (`agent/researcher`).
The LLM selects a profile from the `config` enum, and the shell script maps it
to the corresponding file path.

### Restricting capabilities

Capability restrictions are enforced through the agent config file and the
tool's shell script, without special support in JP core.

| Control              | Mechanism                                              |
| -------------------- | ------------------------------------------------------ |
| No recursion         | Agent config does not enable `jp_query`                |
| Read-only tools      | Agent config only enables read tools                   |
| Model selection      | Agent config sets `assistant.model`                    |
| Allowed profiles     | `config` parameter's `enum` lists valid profiles       |
| Allowed overrides    | Shell script validates override keys against allowlist |
| Conversation scoping | `--root-id` flag enforces descendant constraint in JP  |

## Security: conversation scoping

The tools restrict the LLM to conversations in its own subtree, preventing it
from reading, searching, or continuing unrelated user conversations.

Subtree scoping is enforced at two levels:

**Command template level.** The shell script controls the `jp` invocation.
The LLM supplies parameter values (conversation IDs and queries), while the
command structure is fixed so the LLM cannot inject flags or alter the command.

**`--root-id` constraint.** Tools that accept a conversation ID from the LLM
pass `--root-id={{context.conversation_id}}` to `jp`.
JP verifies that the target conversation is a descendant of that root and fails
with an error otherwise.
Because this check runs inside JP rather than in the shell script, it is atomic
and cannot be bypassed by malformed input.

| Tool                    | Scoping mechanism                       |
| ----------------------- | --------------------------------------- |
| `jp_query` (new)        | `conversation fork`                     |
|                         | `{{context.conversation_id}} --last=0`  |
| `jp_query` (continue)   | `--id=<llm-provided>`                   |
|                         | `--root-id={{context.conversation_id}}` |
| `jp_conversation_list`  | Hardcodes                               |
|                         | `--root={{context.conversation_id}}`    |
| `jp_conversation_print` | `--root-id={{context.conversation_id}}` |
| `jp_conversation_grep`  | `--root-id={{context.conversation_id}}` |
|                         | or `--root=`                            |

Two additional boundaries restrict configuration:

- **Profile allowlist (`config` enum).** The `config` parameter's `enum` limits
  which agent profiles the LLM can select, preventing it from loading arbitrary
  config files via a free-form path.

- **Override allowlist (`overrides` validation).** The shell script accepts only
  allowlisted override keys (`assistant.model=*`), so the LLM cannot override
  tool settings, system prompts, or other configuration.

## Workflows

### Research-plan-implement

The sub-agent pattern supports a phased workflow orchestrated by the main agent:

1. **Research.** Delegate codebase investigation to a researcher sub-agent.
   The sub-agent reads files, runs greps, and returns a structured summary.

2. **Plan.** Pass the research summary to a planner sub-agent (or plan in the
   main agent) to produce a step-by-step implementation plan.

3. **Implement.** Execute the plan in the main agent, delegating follow-up
   research tasks whenever a knowledge gap comes up rather than reading files
   directly.

The main agent's context stays lean throughout: it sees summaries and plans, not
raw file contents from the research phase.

### Mid-task research

When questions arise during implementation, the main agent can delegate them to
a sub-agent at any point:

```txt
main_agent > [modifying error.rs, realizes it needs to understand retry logic]
           > jp_query(config: "agent/researcher", query: "How does retry.rs pattern-match on Error variants?")
           < [500-token summary]
           > [continues implementation with the new knowledge]
```

The main agent can also check whether an earlier sub-conversation already
covered the topic:

```txt
main_agent > jp_conversation_grep(pattern: "retry")
           < jp-c19283746501: retry.rs matches on Error variants in `is_retryable()`:
           > jp_query(config: "agent/researcher", id: "jp-c19283746501", query: "What about the backoff calculation?")
```

### Parallel sub-agents

JP's `ToolCoordinator` runs tool calls in the same batch concurrently.
When the main agent calls `jp_query` multiple times in a single turn, each call
runs in parallel in its own sub-conversation:

```txt
main_agent > jp_query(config: "agent/researcher", query: "Analyze error handling in jp_llm")
           > jp_query(config: "agent/researcher", query: "Analyze retry logic in jp_llm")
           < (both results returned when both complete)
```

Because each `<response>` tag includes its `conversation_id`, the main agent can
track and continue each sub-conversation independently.

#### Future: stateful sub-agents

With [RFD 009] and [RFD 037], `jp_query` could adopt the stateful tool protocol
for asynchronous execution, allowing the main agent to spawn sub-agents,
continue working, and await their results later.

## Tradeoffs

**Process overhead.** Each sub-agent call spawns a new `jp` process, including
argument parsing, config resolution, and MCP server connection setup.
Across multiple LLM turns, this startup cost is small compared to API latency,
but for single lookups calling a tool directly is faster.

**Conversation storage growth.** Sub-conversations are stored on disk, and an
aggressive main agent can create many of them.
Passing `--tmp` auto-expires ephemeral sub-conversations, and `--hidden` ([RFD
040]) keeps them out of default `jp conversation ls` output.
Conversation trees ([RFD 039]) group them under the parent conversation, and
removing the parent with `--cascade` cleans up the entire subtree.

**Delegation quality.** The main agent must formulate clear, scoped tasks.
Vague delegations ("look at the codebase") waste tokens, while narrow tasks
("read line 42 of error.rs") are cheaper as direct tool calls.
The `jp_query` description instructs the model to delegate tasks that require
reading multiple files, and the main agent's system prompt should reinforce when
to delegate.
Whether frontier models follow this guidance consistently still needs
validation.

**`conversation print` token cost.** Printing a full sub-conversation can inject
thousands of tokens into the main agent's context.
The tool description warns against this and recommends `jp_conversation_grep` or
follow-up `jp_query` calls instead; when only the latest response is needed,
`--last=1` caps the output.

**JSON output key stability.** `jp conversation ls --format=json` serializes
display-oriented table column headers (`"ID"`, `"#"`, `"Activity"`) rather than
a stable schema.
The shell scripts in this guide use those keys, which may change to `snake_case`
(`id`, `events_count`) once a structured output format is defined.

## Prerequisites

This guide assumes the following RFDs are implemented:

- [RFD 038]: `--cfg=NONE` for isolated config and config load paths.
- [RFD 039]: conversation trees (`conversation fork` creates children) and
  `--root` flag on `conversation ls`.
- [RFD 040]: hidden conversations and `conversation_id` in tool context.
- [RFD 048]: output channel separation (stdout for assistant output, stderr for
  chrome) so sub-agent output can be cleanly captured.
- [RFD 049]: `--non-interactive` mode for sub-agents running without a terminal.
- [RFD 050]: `jp conversation new`, `--no-activate`, and `--root-id` for
  scripting ergonomics.

Additionally:

- `conversation grep` needs a `--root` flag (matching `conversation ls`) to
  scope searches to a subtree.
- `conversation print` and `conversation grep` need `--root-id` support for
  descendant validation.
- `conversation fork` needs to print the new conversation ID to stdout and
  accept config options (`--cfg`, `--hidden`) per [RFD 050].

## References

- [RFD 038: Config Reset Keywords][RFD 038]: `--cfg=NONE` and config keywords.
- [RFD 039: Conversation Trees][RFD 039]: tree-structured conversation hierarchy
  and `--root` scoping.
- [RFD 040: Hidden Conversations and Tool Context][RFD 040]: `hidden` flag and
  `conversation_id` in tool execution context.
- [RFD 048: Four-Channel Output Model][RFD 048]: stdout/stderr separation for
  clean sub-agent output capture.
- [RFD 049: Non-Interactive Mode][RFD 049]: detached prompt policies for
  sub-agents running without a terminal.
- [RFD 050: Scripting Ergonomics][RFD 050]: `conversation new`, `--no-activate`,
  and `--root-id`.
- [RFD 009: Stateful Tool Protocol][RFD 009]: future handle registry for async
  sub-agents.
- [RFD 037: Await Tool][RFD 037]: future synchronization for parallel stateful
  sub-agents.

[RFD 009]: 009-stateful-tool-protocol.md
[RFD 037]: 037-await-tool-for-stateful-handle-synchronization.md
[RFD 038]: 038-config-reset-keywords.md
[RFD 039]: 039-conversation-trees.md
[RFD 040]: 040-hidden-conversations-and-tool-context.md
[RFD 048]: 048-four-channel-output-model.md
[RFD 049]: 049-non-interactive-mode-and-detached-prompt-policy.md
[RFD 050]: 050-scripting-ergonomics-for-conversation-management.md
