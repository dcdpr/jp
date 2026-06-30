# RFD 078: Tool Config Mutation

- **Status**: Accepted
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-04-17
- **Requires**: [RFD 076], [RFD 070]

## Summary

This RFD introduces scoped config access for tools via `access.config` — a new
resource type in [RFD 076]'s access model.
Workspace owners grant tools read and/or write access to specific config paths.
Tools receive granted config values in `context.config` on invocation and return
updated values in `outcome.config`.
Approved changes land in a per-cycle commit buffer and are emitted at cycle end
as a single merged `ConfigDelta` event on the conversation's event stream.

Grants apply to any `AppConfig` path — `assistant.model`,
`conversation.attachments`, `conversation.tools.*`, and so on.
This enables tools that adapt the assistant's behavior during a conversation.

## Motivation

Tools today cannot interact with JP's configuration.
A tool cannot read what model is active, cannot adjust tool availability for the
next turn, and cannot modify attachments.
The only way to change config mid-conversation is through the user's CLI flags
or config files.

This limits what tools can do:

- A coding tool cannot switch to a stronger model when it encounters a complex
  problem.
- A workflow orchestration tool cannot disable tools that are no longer relevant
  for the current phase.
- A tool cannot read the current model or attachment configuration to adapt its
  behavior.
- A config file loaded via `config_load_paths` (e.g., a phase-specific override)
  cannot adjust the assistant's configuration between phases without user
  intervention.

The config system already has persistence via `ConfigDelta`, fork propagation,
CLI seeding, and file-based defaults.
Giving tools scoped access to this infrastructure is a natural extension.

## Design

### `access.config`

[RFD 076] defines a per-tool `access` field with resource types for filesystem
(`fs`), network (`net`), and environment variables (`env`).
This RFD adds `config` as a fourth resource type using the same rule-based
model.

Each config rule grants capabilities at a config path.
Like [RFD 076]'s filesystem rules, capabilities default to `false` and each rule
is self-contained:

```toml
# Grant read + write to assistant.model, user approves each write
[[conversation.tools.change_model.access.config]]
path = "assistant.model"
read = true
write = true
apply = "ask"

# Grant read-only to attachments
[[conversation.tools.summarize_attachment.access.config]]
path = "conversation.attachments"
read = true

# Grant read access to attachments, and allow adding (write) but not
# removing (delete) entries. Tool can only augment the attachment list.
[[conversation.tools.augment_attachments.access.config]]
path = "conversation.attachments"
read = true
write = true
# delete defaults to false — tool cannot unset entries

# Grant sensitive-path write access with explicit acknowledgment
[[conversation.tools.toggle_tools.access.config]]
path = "conversation.tools"
read = true
write = "insecure_allow"
delete = true
apply = "ask"
```

When no `access.config` rules are present, the tool has no config access —
consistent with [RFD 076]'s default-deny model.

#### Capabilities

| Field    | Description                                                          |
| -------- | -------------------------------------------------------------------- |
| `read`   | Read the current value of the config path                            |
| `write`  | Return updated values for the config path (via `outcome.config`)     |
| `delete` | Unset the config path or remove vec elements (via `outcome.unset`)   |
| `apply`  | Delta application mode: `"ask"` or `"unattended"` (default: `"ask"`) |

`read`, `write`, and `delete` are independent capabilities — none implies the
others.
A tool granted `write = true` without `read = true` writes blind, receiving no
current value in `context.config`.
A tool granted `write` but not `delete` can set or replace values but cannot
remove them via `outcome.unset`.

`apply` controls whether the user is prompted before a config delta is applied.
It governs both `outcome.config` writes and `outcome.unset` removals.
Default is `"ask"`.
This is separate from the tool's `run` mode, which controls whether the tool
runs at all.

`apply` only applies to writes and deletes.
Reads always go through without prompting.

#### Sensitive paths and `write = "insecure_allow"`

JP maintains a small list of hardcoded sensitive config paths.
These are paths where unintended writes could compromise safety or security:

- `conversation.tools.*.access` — writing to a tool's own access policy allows
  self-escalation.

(The list will grow over time as other sensitive paths are identified.)

**Wildcard semantics.** `*` is a single-segment wildcard that matches any key at
that position.
For example, `conversation.tools.*.access` matches
`conversation.tools.fs_modify_file.access`, `conversation.tools.foo.access`, and
so on — one `*` consumes exactly one path segment.

Wildcards are only valid in positions where the underlying config type is a
`Map<String, ...>` (e.g., `conversation.tools` is `IndexMap<String,
ToolConfig>`).
This is a deliberate constraint: wildcards expand the set of paths the rule
covers, and that only makes sense when the set is genuinely unbounded (as with
user-keyed maps).
Hardcoded struct fields have a fixed set of names and should be enumerated
individually if multiple are sensitive.

JP validates this at config-load time — attempting to use `*` where the schema
type is not a map produces a config error.

For sensitive paths, `write = true` is rejected at config validation time with
an explicit error.
The workspace owner must use `write = "insecure_allow"` to acknowledge the risk:

```
Config error: tool 'X' grants write = true to path 'conversation.tools.*.access'
which is a sensitive path. Use write = "insecure_allow" to explicitly
acknowledge this grant.
```

The `insecure_allow` value is only meaningful on sensitive paths.
On non-sensitive paths, `write = true` and `write = "insecure_allow"` behave
identically.

**Orthogonality from `apply`.** The sensitivity acknowledgment (`write` value)
and confirmation mode (`apply` value) are independent.
A workspace owner can:

- `write = "insecure_allow"` + `apply = "ask"` — acknowledge sensitivity and
  still confirm each write interactively.
- `write = "insecure_allow"` + `apply = "unattended"` — acknowledge sensitivity
  and skip prompts for non-interactive workflows.

Default for `apply` remains `"ask"` regardless of the `write` value.
To set `apply = "unattended"` on an insecure path, the workspace owner must
write it explicitly on that rule.
Self-contained rule semantics mean no inheritance from other rules and no
implicit "insecure paths default to ask" treatment — the default simply applies
as it would for any rule.

#### Evaluation: longest prefix match

When multiple rules match a config path, the most specific rule wins.
Specificity is determined by path component count (dot-separated segments).
The winning rule applies in full; capabilities are not inherited from less
specific rules.

```toml
# Rule A: broad read over all conversation config
[[access.config]]
path = "conversation"
read = true

# Rule B: write access to tools (sensitive path)
[[access.config]]
path = "conversation.tools"
read = true
write = "insecure_allow"

# Rule C: deny access to own access policy
[[access.config]]
path = "conversation.tools.toggle_tools.access"
# defaults to false — denied
```

- `conversation.attachments` → matches Rule A → read only
- `conversation.tools.fs_read_file` → matches Rule B → read + write
- `conversation.tools.toggle_tools.access.config` → matches Rule C → denied

If no rule matches a config path, all capabilities are denied.

Rules are self-contained — each rule is readable in isolation.
This is the same design as [RFD 076]'s filesystem rules, for the same reasons:
clarity over brevity, no subtle inheritance bugs.

### Structured Outcome Transport

The current execution pipeline flattens a successful tool outcome to a bare
`String` at the first hop: `parse_command_output` converts
`jp_tool::Outcome::Success { content }` into `CommandResult::Success(content)`,
which becomes `ExecutionOutcome::Completed { result: Ok(String) }`, which is
finally stored as `ToolCallResponse { result: Result<String, String> }`.
By the time the tool coordinator runs `handle_tool_result`, any structured
payload has been discarded.

This RFD requires a structured success payload to survive end-to-end so the tool
coordinator can validate and apply `config`/`unset` before the tool's content is
delivered to the LLM.
The plumbing change spans three crates.

**`jp_tool::Outcome::Success` grows structured fields.**

```rust
#[derive(Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Outcome {
    Success {
        content: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        config: Option<serde_json::Value>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        unset: Vec<String>,
    },
    // Error and NeedsInput unchanged.
}
```

`config` and `unset` are only valid on `Success`.
`Error` and `NeedsInput` cannot carry config mutations (see [Output:
`outcome.config` and `outcome.unset`](#output-outcomeconfig-and-outcomeunset)).

**`jp_llm::CommandResult::Success` and `ExecutionOutcome::Completed` carry the
structured payload.**

The variants grow to hold a `ToolSuccess { content, config, unset }` record (or
equivalent), replacing the bare `String`.
All existing call sites that project back to a string continue to work via a
`.content` accessor; the new fields are observed only by the coordinator path
that needs them.

**`ToolCallResponse` is unchanged.**

The event stored on the conversation event stream still carries `Result<String,
String>`.
Config mutations leave the pipeline as a separate `ConfigDelta` event, not as
metadata on the tool response.
This keeps the stream schema honest: `ToolCallResponse` records the LLM-facing
content; `ConfigDelta` records the config change.

**Builtin tools share the wire type but not the semantics.**

`jp_tool::Outcome` is the shared return type for all tool execution paths,
including builtins.
Config mutations from builtins are out of scope for this RFD — their execution
path has no `Context` and no `access.config` plumbing.
If a builtin returns `Outcome::Success` with `config` or `unset` populated, the
host execution path — specifically `jp_llm::tool::execute_builtin`, where
builtin outcomes are interpreted and mapped onto `ExecutionOutcome::Completed`
— **drops those fields and logs a warning** before the outcome reaches the
coordinator.
This keeps the wire type shared without silently honoring mutations from an
unprivileged path.
Adding config access to builtins is deferred to the follow-up RFD noted in
[Non-Goals](#non-goals).

**Coordinator integration.**

`ExecutorResult::Completed` delivers the structured outcome to the tool
coordinator's `handle_tool_result` path.
The coordinator runs the config delta pipeline (validation, authorization, apply
chrome, buffer, optional re-invocation) before the tool's `content` enters the
existing result-mode dispatch.
See [Interaction with Result Mode](#interaction-with-result-mode) for the full
ordering.

### Tool Protocol

This section describes the protocol for **local tools** (tools that communicate
via JP's stdin/stdout JSON protocol).
Builtin tools and MCP tools are out of scope; see [Non-Goals](#non-goals) for
the rationale.

**Action scope.** `access.config` applies only to `Action::Run` invocations.
`Action::FormatArguments` invocations never carry `context.config` or
`context.delta_rejection`.
If a tool under `FormatArguments` returns `outcome.config` or `outcome.unset`,
JP ignores those fields — formatter output is display-only and cannot mutate
config.
This matches [RFD 076]'s treatment of both actions as separate enforcement
surfaces.

**Enforcement is host-side.** Unlike [RFD 076]'s `fs`, `net`, and `env` rules
(which are enforced by tools self-checking via `Context`), config access is
enforced entirely by JP.
JP filters `context.config` before sending it to the tool, validates
`outcome.config` and `outcome.unset` against grants before persisting, and
constructs the `ConfigDelta`.
The tool sees its grants in `context` for introspection (e.g., "can I write this
path?") but cannot bypass the host enforcement.
[RFD 076]'s planned OS-level enforcement extension eventually unifies fs/net/env
enforcement under JP as well; this RFD follows that direction from the start.

#### Input: `context.config`

On invocation, the tool receives the current values of its granted read paths in
`context.config`, alongside the existing `context.root` and `context.action`
fields:

```json
{
  "tool": {
    "name": "change_model",
    "arguments": {
      "target": "sonnet"
    },
    "answers": {},
    "options": {}
  },
  "context": {
    "root": "/path/to/workspace",
    "action": "run",
    "config": {
      "assistant": {
        "model": {
          "id": {
            "provider": "anthropic",
            "name": "opus"
          }
        }
      }
    }
  }
}
```

Only paths matched by `access.config` rules with `read = true` are included.
The tool sees a partial config tree scoped to its grants.
`write = true` or `delete = true` without `read = true` does not grant read
access — the tool writes or removes blind.

Tools that do not have `access.config` receive no `config` field in their
context — backward compatible with existing tools.

#### Output: `outcome.config` and `outcome.unset`

Local tool output is parsed as `jp_tool::Outcome`, a tagged enum (`#[serde(tag =
"type", rename_all = "snake_case")]`) with three variants: `success`, `error`,
and `needs_input`.
This RFD adds two optional fields to the `success` variant only: `config` for
sets and updates, `unset` for removals.

```json
{
  "type": "success",
  "content": "Model changed to sonnet, cleared stale parameters.",
  "config": {
    "assistant": {
      "model": {
        "id": "sonnet"
      }
    }
  },
  "unset": [
    "assistant.model.parameters.temperature",
    "conversation.attachments[\"stale-file.md\"]"
  ]
}
```

The `"id": "sonnet"` in this example is the alias form of
`ModelIdOrAliasConfig`; JP resolves it to the expanded `{ provider, name }` form
before authorization.
Tools may use either the alias form or the expanded form; authorization and
claims always operate on the resolved leaf set (see [Alias resolution for union
fields](#delta-application)).

`config` and `unset` are not permitted on the `error` or `needs_input` variants.
A tool that errored has not completed successfully and should not produce config
mutations; a tool requesting more input has not yet finished and should defer
mutations until the final success outcome.

JP processes a successful outcome by:

1. Deserializing `config` as `PartialAppConfig` — the same type used by `--cfg`
   flags and config files.
   Deserialization performs structural validation for free (type matching, enum
   variants, required fields).
2. Parsing `unset` as a list of dotted paths per [RFD 070]'s `unsets` format,
   including vec-element syntax (`path["serialized-json-value"]`).
3. Building a `ConfigDelta` with `delta = partial`, `unsets = paths`, and
   `claims = { path → None for each written or unset path }` (see [Claims on
   tool-generated deltas](#claims-on-tool-generated-deltas)).
4. Adding the delta to the per-cycle commit buffer.
   A single merged `ConfigDelta` is emitted to the event stream at cycle end
   (see [Per-Cycle Commit Buffer](#per-cycle-commit-buffer)).

The tool never sees or constructs a `ConfigDelta` directly — it works with
plain config values and unset paths.
Tools that omit `config` and `unset` produce no delta — backward compatible
with existing tools.

#### Merge semantics

`outcome.config` is deserialized as `PartialAppConfig` and merged using the
**existing config merge machinery** — the same pipeline used by `--cfg` flags
and config files.

This means:

- **Fields present** in `outcome.config` apply per their field-level merge
  strategy.
  Scalar fields are replaced.
  `Option<T>` fields are set.
  Vec fields follow their declared merge strategy (`MergeableVec` with append,
  replace, or other modes as configured on the field).
- **Fields absent** in `outcome.config` are preserved — `None` in the partial
  means "no change," not "clear."
- **Nested objects** merge recursively.
  Setting `{ "assistant": { "model": { "id": "sonnet" } } }` changes only
  `assistant.model.id`; other fields under `assistant.model` (e.g.,
  `parameters`) are untouched.

These are the same semantics that govern `jp q --cfg
'assistant.model.id:="sonnet"'`.
Tool authors write partial configs; they do not need to know how merging works
for each field type — the field's declared merge strategy is authoritative.

`outcome.unset` is the escape hatch for **removals**.
Because partial-merge semantics cannot express `Some → None` or vec-element
removal, explicit unset paths are needed.
This parallels [RFD 070]'s `unsets` field on `ConfigDelta`.

#### Claims on tool-generated deltas

Per [RFD 070], `ConfigDelta` events carry a `claims` map that records which
config source last set each field, enabling `-C flag` (file-based revert) to
find the right values to walk back to.

Tool-generated deltas populate `claims[path] = None` for every modified and
unset path.
The `None` value is the explicit-unclaim marker (same as the mechanism used by
CLI shortcut flags like `--model`).
It signals "this field is deliberately set to its current value; do not walk
back through file-based claims to revert it."

Consequence: `-C dev` does not revert tool-made changes.
If a user wants to undo a tool's config mutation, they use the same mechanisms
available for any other persisted `ConfigDelta` — e.g., issuing a
counter-change via `--cfg` or a subsequent tool call, or editing the
conversation's event stream directly.

This matches how key-value `--cfg` assignments work: they also do not produce
file-attributed claims, and `-C file` cannot reach past them.

### Delta Application

After a tool produces `outcome.config` and/or `outcome.unset`, JP validates and
applies the resulting delta.

**Alias resolution for union fields.** A handful of config fields are untagged
unions — most notably `assistant.model.id`, which is a `ModelIdOrAliasConfig`
that accepts either a structured object (`{ provider, name }`) or an alias
string (`"sonnet"`).
Before authorization runs, JP normalizes the deserialized `PartialAppConfig` by
resolving every alias variant to its expanded `{ provider, name }` form using
the workspace's alias map.
This reuses the existing `PartialModelIdOrAliasConfig::resolve_in_place` pass
that JP already runs on `PartialAppConfig` values before they are persisted as
`ConfigDelta`s in the event stream.

The resolved tree is the authoritative view for every downstream stage: leaf
enumeration, authorization, apply chrome display, claims generation, and buffer
folding all operate on the same expanded leaf set.
A rule granting only `assistant.model.id.name` does **not** implicitly allow a
provider switch via alias expansion — the alias resolves first, and the
`provider` leaf must be independently covered.
If the alias is unknown and cannot be parsed as `provider/name`, the delta is
rejected as `invalid_config` with the alias name in `detail`.

The full pipeline:

1. **Structural validation.** JP deserializes `outcome.config` as
   `PartialAppConfig`.
   Serde catches:

   - Unknown field paths that don't exist in the schema.
   - Type mismatches (e.g., string where number expected).
   - Invalid enum variants (e.g., `"provider": "bogus"` when `ProviderId` only
     accepts `anthropic | openai | ollama | ...`).

   Partial types are intentionally permissive: missing fields on populated
   subtrees are NOT flagged here.
   `required` constraints apply to the fully-resolved config, not to partials.
   Required-field violations surface later, at re-resolve time.

   For `outcome.unset`, JP parses each path per RFD 070's syntax and verifies
   the base path refers to a field that exists in the schema and has a type that
   supports unsetting (optional scalar, or vec for element-level unsets).
   Specific vec-element existence is NOT validated — element removal is
   idempotent per RFD 070 (filtering produces the same array if the element
   wasn't there).

   Validation is limited to structural checks.
   JP does **not** validate semantic correctness (e.g., whether a specific model
   name is available from the provider, or whether an attachment file exists on
   disk).
   Semantic failures surface later, at re-resolve time (see [Re-Resolve
   Failures](#re-resolve-failures)).

2. **Authorization check.** Authorization runs over the set of **concrete leaf
   paths** touched by the outcome, not over the top-level keys present in the
   JSON tree.
   JP walks the deserialized `PartialAppConfig` and enumerates every leaf
   assignment.
   For example, the JSON tree:

   ```json
   {
     "assistant": {
       "model": {
         "id": {
           "provider": "anthropic",
           "name": "sonnet"
         },
         "parameters": {
           "temperature": 1.0
         }
       }
     }
   }
   ```

   produces the leaf set `{ assistant.model.id.provider,
   assistant.model.id.name, assistant.model.parameters.temperature }`.
   Each leaf must independently be covered by a rule with `write = true` or
   `write = "insecure_allow"`.
   A rule granting `write` at a parent path (e.g., `assistant.model.id`) covers
   every leaf beneath it via the longest-prefix-match evaluation in [Evaluation:
   longest prefix match](#evaluation-longest-prefix-match).

   `outcome.unset` paths are authorized directly (they are already concrete
   paths) and each must be covered by a rule with `delete = true`.

   If any leaf write or unset path falls outside granted access, the entire
   delta is rejected with reason `unauthorized_paths`.

   Apply chrome (step 3) and claim generation (step 4) operate on the same
   leaf-path set.
   The three layers — authorization, confirmation, claims — cannot diverge.

3. **Apply chrome.** If any leaf path matched by the delta falls under a rule
   with `apply = "ask"`, JP displays a single prompt listing all proposed
   changes (both writes and removals):

   ```
   ⟳ Configuration delta proposed by tool 'change_model':
   
     assistant.model.id: "anthropic/opus" → "anthropic/sonnet"
     assistant.model.parameters.temperature: (unset)
   
   Apply delta? [Y/n/?]
   ```

   Multi-field deltas are shown as a single batched prompt.
   Approve once, all changes (both writes and unsets) land atomically.
   Reject once, no changes land.

   **Non-interactive behavior.** When the session has no interactive prompt path
   — no TTY, `--non-interactive`, or a detached query — and any matched leaf
   would require `apply = "ask"`, JP rejects the delta with reason
   `confirmation_unavailable` (see [Delta Rejection and
   Re-Invocation](#delta-rejection-and-re-invocation)).
   This is distinct from `user_rejected`: the former signals an environment
   constraint (no prompt path), the latter signals an explicit human decline.
   Tools may branch on the reason — for example, offering a fallback path for
   `confirmation_unavailable` but not for `user_rejected`.
   JP never auto-approves a write that required confirmation.
   Tools that must mutate config in unattended environments require the
   workspace owner to set `apply = "unattended"` explicitly on the relevant
   rule.

4. **Buffer.** If approved (or if all affected rules are `unattended`), JP adds
   the delta to the **per-cycle commit buffer** rather than appending to the
   event stream immediately.
   See [Per-Cycle Commit Buffer](#per-cycle-commit-buffer) for how buffered
   deltas merge and commit at cycle end.

5. **Deliver content.** The tool's `content` from the outcome flows into the
   existing result-mode pipeline (see [Interaction with Result
   Mode](#interaction-with-result-mode)).

If any step (validation, authorization, approval) fails, the flow branches to
delta rejection (see below).

### Delta Rejection and Re-Invocation

When delta application fails, JP re-invokes the tool with a
`context.delta_rejection` field describing what went wrong.
This gives the tool a chance to adapt its response or retry with corrected
values.

**Rejection reasons:**

| Reason                     | Trigger                                                  |
| -------------------------- | -------------------------------------------------------- |
| `invalid_config`           | Structural validation failed (in `config` or `unset`),   |
|                            | or an alias in a union field could not be resolved       |
| `unauthorized_paths`       | Paths outside granted `write` or `delete` rules          |
| `user_rejected`            | User explicitly declined at the apply chrome prompt      |
| `confirmation_unavailable` | Apply chrome required but no interactive prompt path was |
|                            | available (no TTY, `--non-interactive`, detached query)  |

**Re-invocation protocol:**

The tool is re-invoked with the same `arguments`, `answers`, and `options`, plus
a new `context.delta_rejection` field.
Accumulated `answers` from any prior `needs_input` rounds are preserved so the
re-invoked run has the same state as the initial call:

```json
{
  "tool": {
    "name": "change_model",
    "arguments": {
      "provider": "bogus"
    },
    "answers": {},
    "options": {}
  },
  "context": {
    "root": "/path/to/workspace",
    "action": "run",
    "config": {
      "assistant": {
        "model": {
          "id": {
            "provider": "anthropic",
            "name": "opus"
          }
        }
      }
    },
    "delta_rejection": {
      "reason": "invalid_config",
      "fields": [
        "assistant.model.id.provider"
      ],
      "detail": "unknown variant 'bogus': expected one of anthropic, cerebras, deepseek, google, llamacpp, ollama, openai, openrouter"
    }
  }
}
```

The tool inspects `context.delta_rejection` and branches its response.
It may:

- Return new `content` without any `config` or `unset`, reflecting that the
  change didn't land.
- Return new `content` with corrected `config` or `unset` (retry with different
  values).
- Return identical output (will be rejected again).

A re-invocation follows the same pipeline: validation, authorization, approval,
application.
The retry counter increments on each rejection.

**Retry limit:**

JP enforces a maximum of 3 delta rejections per tool call.
On the 4th failure, the tool call aborts and JP **synthesizes a
`ToolCallResponse`** with `result = Err(fallback_message)` so the conversation
event stream and the LLM's view of the tool call remain well-formed.
The synthesized response is an ordinary tool-result event routed through the
existing `commit_tool_responses()` path — it is not a new event type and not a
separate host-to-LLM message channel.
The content looks like:

```
Tool 'change_model' failed to produce a valid config delta after 3 attempts.
Last error: unknown variant 'bogus' at assistant.model.id.provider.
```

**Side effects on re-invocation:**

A re-invoked tool runs twice (or more).
External side effects outside of `outcome.config` and `outcome.unset` (file
writes, network calls, subprocess launches) happen multiple times.
Tool authors using `access.config` must design their tools to be safe under
re-invocation — either idempotent or guarded against re-entry.
This is part of the contract for tools that mutate config.

### Interaction with Result Mode

The existing tool pipeline in `crates/jp_cli/src/cmd/query/tool/coordinator.rs`
handles tool completion via a result-mode dispatch that lets the user approve,
edit, or skip the tool's content before it reaches the LLM.
The modes are `Unattended`, `Ask`, `Edit`, and `Skip`, configured per tool.

Config delta processing inserts **before** the result-mode dispatch inside
`handle_tool_result` — the post-execution content-delivery stage that runs on
`ExecutorResult::Completed` events.
The full ordering when a tool call completes:

1. **Tool execution completes** (existing behavior) with an outcome that may
   include `config` and `unset`.

2. **Config delta processing** (new): a.
   Structural validation (see [Delta Application](#delta-application)). b.
   Authorization check against `access.config` rules. c.
   Apply chrome if any matching rule has `apply = "ask"`. d.
   On rejection (invalid, unauthorized, or user-rejected) — re-invoke the tool
   with `context.delta_rejection` and restart this step.
   Retry counter increments; exhaustion aborts the tool call. e.
   On approval — add the delta to the per-cycle commit buffer (see [Per-Cycle
   Commit Buffer](#per-cycle-commit-buffer)).
   The delta is not yet in the event stream.

3. **Result mode processing** (existing behavior): the tool's `content` flows
   through `handle_tool_result` per the tool's `ResultMode` (`Unattended` /
   `Ask` / `Edit` / `Skip`).

**Config and content are independent.** The approval gate for config is the
apply chrome in step 2c; the approval gate for content is result mode in step 3.
`Skip` at result mode hides content from the LLM but does not revoke a
provisional config approval.
Whether a provisionally-approved delta eventually lands in the event stream is
decided at cycle end by the commit buffer (specifically, by which path closes
the cycle — see [Cycle-termination semantics](#cycle-termination-semantics)).

**Re-invocation avoids result-mode friction.** Because re-invocation happens in
step 2d (before result mode), a user does not edit content that is subsequently
thrown away by a delta rejection.
By the time content reaches result mode, the delta has been resolved (either
added to the buffer or abandoned).

#### Prompt ordering

The coordinator already runs a single FIFO queue of `PendingPrompt` entries with
two variants: `Question` (tool asking for input) and `ResultMode` (user
approving tool content).
This RFD adds a third variant:

```rust
enum PendingPrompt {
    Question { index: usize, question: Question },
    ResultMode { index: usize, tool_id: String, ... },
    ApplyDelta { index: usize, tool_id: String, delta: ProposedDelta },
}
```

`ApplyDelta` is enqueued when a tool completes with an `outcome.config` or
`outcome.unset` whose authorization check passed and whose matched rules include
`apply = "ask"`.
Ordering rules:

- The queue stays FIFO.
  Apply chrome does not preempt a queued prompt from another tool.
  A question prompt from tool B that was already queued when tool A finished is
  served before tool A's apply chrome.
- `ApplyDelta` for tool A is enqueued before `ResultMode` for tool A. Within a
  single tool, config is resolved before content (matching the per-tool ordering
  in [Interaction with Result Mode](#interaction-with-result-mode)).
- Only one prompt is active at a time (`prompt_active` guards the queue), same
  as today.

The consequence: a user may see prompts interleaved across tools (question for
B, apply chrome for A, result mode for A, result mode for B) depending on
completion order.
This is the same interleaving behavior that already applies to
question/result-mode prompts; the RFD does not introduce a priority system.

### Per-Cycle Commit Buffer

Approved config deltas are held in a per-cycle commit buffer during the
executing phase.
The buffer is a list of `(tool_call_index, PartialAppConfig, Vec<UnsetPath>)`
entries, one per successfully-approved tool invocation.
Entries append as tools complete.

**The live event stream sees nothing until the buffer commits.** Config deltas
approved in apply chrome are provisional — they're in memory, not yet events.

#### Within-cycle visibility

Tools within the same cycle do not see each other's provisional deltas.
Each tool's `context.config` reflects the config state as of cycle start.
Re-invocation of a rejected tool also sees cycle-start state, not the buffer's
accumulating contents.
This keeps re-invocation deterministic: the same input produces the same
expected behavior regardless of what other tools in the cycle are doing.

If a tool needs to see another tool's config change, the second tool must run in
a later cycle (a subsequent LLM round-trip).
This is the same constraint that already applies to tool *content* — a tool
cannot observe another tool's output within the same cycle.

#### Commit at cycle end

At cycle end (all tools completed, all result-mode processing finished), the
buffer is folded into a single `ConfigDelta` event:

1. **Sort** buffer entries by `tool_call_index` (the order the LLM emitted the
   tool calls).
   This is more deterministic than completion order and matches the ordering the
   LLM observes in its conversation history.

2. **Fold per-path** in call order.
   For each config path touched by any buffered entry:

   - The last operation in call order determines the final state.
   - If the last op was a set (via `outcome.config`), the path ends up in the
     merged `PartialAppConfig` with that value.
   - If the last op was an unset (via `outcome.unset`), the path ends up in the
     merged `unsets` list.

   This ensures `ConfigDelta.delta` and `ConfigDelta.unsets` never refer to the
   same path — a clean invariant.

3. **Merge claims.** Union all modified and unset paths, each mapped to `None`
   (the explicit-unclaim marker, see [Claims on tool-generated
   deltas](#claims-on-tool-generated-deltas)).

4. **Emit** one `ConfigDelta { delta, unsets, claims }` event.
   The cycle then triggers re-resolve and proceeds to the next stream.

Only one `ConfigDelta` event is emitted per cycle, regardless of how many tools
contributed to it.
Empty buffers produce no event; the cycle completes normally without triggering
re-resolve.

#### Cycle-termination semantics

The per-cycle commit buffer is in-memory state owned by the cycle coordinator.
Its fate is governed by how the executing phase ends.
JP's tool-execution interrupt menu offers three choices:

- **Continue**: no change to the buffer.
  Tools keep running; new approvals append as they complete.
  The cycle commits normally when all tools finish.

- **Stop & respond**: cancel in-flight tools, then run the normal cycle-end commit
  path.
  Any deltas already approved at apply chrome commit to the event stream as a
  single merged `ConfigDelta`.
  The user's reply text is attached to each cancelled tool as its response
  content (wrapped in JP's cancellation message), matching current Stop & respond
  behavior — the LLM sees the cancelled tool responses, not a new user request.
  User-explicit approvals from apply chrome are not revoked by cutting the cycle
  short.

- **Restart**: cancel in-flight tools, discard the buffer entirely.
  The tool batch replays from the LLM's prior response; new tool invocations
  will re-propose their deltas and re-prompt the user for approval.
  This avoids double-counting approvals from a partially-completed run.

**Streaming interrupts** (the separate menu with `Continue` / `Reply` / `Stop` /
`Abort` options) fire during LLM streaming, not during tool execution.
The buffer is always empty at those points because no tools have run yet in the
current cycle — no prior cycle's buffer survives into a new streaming phase.
These interrupts do not interact with the buffer.
Deltas committed in prior cycles of the same turn already crossed their boundary
and are unaffected.

**Process-level termination** (SIGKILL, crash, OS-level kill) destroys the
buffer along with the process.
Because the buffer is memory-only and not tracked by `ConversationMut`'s
dirty-state persistence, no provisional delta can leak to the event stream
through a termination path.

**User implication.** Approvals via apply chrome are provisional until the cycle
closes via Continue or Stop & respond.
A user who clicks "apply" at apply chrome and then chooses Restart does not get
their config change — the Restart explicitly discards the batch to avoid
double-counting on the replayed run.

Implementation note: the buffer must live outside `ConversationMut`'s
dirty-state tracking so that drop-persistence cannot leak provisional deltas.
A plain `Vec` owned by the cycle coordinator, cleared on Restart and flushed to
the stream on Continue / Stop & respond success.

### Re-Resolve Failures

Structural validation at delta-application time does not cover semantic
validity.
A delta like `assistant.model.id.name = "nonexistent"` passes structural checks
(it's a valid string in a valid place) but may fail when the turn loop
re-resolves config and tries to fetch model details from the provider.

If re-resolve fails, the turn aborts with a clear error naming the offending
config path.
The `ConfigDelta` remains in the event stream.
The user recovers by:

- Issuing another config change (via `--cfg`, a tool, or editing the
  conversation), or
- Manually correcting the config and re-running the conversation.

Automatic rollback of re-resolve failures is not attempted.
Distinguishing "transient" failures (provider API down, retry might succeed)
from "persistent" failures (model name genuinely invalid) requires semantics JP
doesn't currently have.

Cross-field and cross-reference validation could catch many of these cases at
delta-application time (e.g., validating that a model name exists for the chosen
provider).
That work is orthogonal to this RFD — it would benefit `--cfg` users and config
files as well — and is noted in [Risks and Open
Questions](#risks-and-open-questions).

### JP-to-LLM Communication

This RFD introduces **no new host-to-LLM channel**.
Tool content still flows through the existing result-mode pipeline, which may
already replace or edit the tool's `content` before it reaches the LLM: `Skip`
substitutes `"Result delivery skipped by configuration."` or `"Result delivery
skipped by user."`, `Edit` allows the user to rewrite the content, and `Ask` can
reject delivery entirely.
That machinery is unchanged by this RFD.

The retry-exhausted fallback is the only case this RFD adds where JP authors the
tool's response content directly — and even there, the mechanism is a
**synthesized `ToolCallResponse`** carried on the normal event stream, not a
distinct host-to-LLM channel.
No new event type; no side-channel.

### User-Facing Chrome for Delta Events

The user sees chrome for:

- **Approval prompt.** Shown when a delta affects any rule with `apply = "ask"`.
  Batched across multi-field deltas.

- **Invalid delta.** Brief informational chrome — no prompt, just notice:

  ```
  ⚠ Tool 'change_model' proposed an invalid config change:
    assistant.model.id.provider = "bogus" (not a valid provider)
  Change not applied, tool re-invoked.
  ```

- **User rejection.** The user already acted on the prompt; JP confirms:

  ```
  Config delta rejected. Tool re-invoked.
  ```

- **Retry exhaustion.** When the 3-retry limit is hit:

  ```
  ⚠ Tool 'change_model' exceeded delta retry limit. Aborting tool call.
  ```

### Config Mutation Lifecycle

Config deltas applied during a turn take effect **between cycles** of the turn
loop.

A turn consists of one or more cycles, where each cycle is: stream an LLM
response → execute tool calls → stream the next response.
When a tool produces a config delta during the executing phase, the cycle
completes, JP detects the delta, and the agentic loop is restarted with
freshly-resolved config before the next stream begins.

The restart mechanism:

1. The executing phase completes.
   All approved deltas are in the per-cycle commit buffer (see [Per-Cycle Commit
   Buffer](#per-cycle-commit-buffer)).
2. If the buffer is non-empty, JP folds the entries into a single merged
   `ConfigDelta`, emits it to the event stream, and returns
   `CycleResult::ConfigChanged` from the inner loop.
3. The outer turn loop reloads `cfg` from the event stream (re-projecting from
   all events including the new `ConfigDelta`) and re-resolves the derived
   values: provider, model, tool definitions, tool choice, inquiry backend.
4. The inner loop re-enters at `TurnPhase::Streaming` (not `Idle`, which would
   re-emit a `TurnStart` event).
   The LLM receives the accumulated thread, which includes the tool's content
   response, and continues.

**Mid-stream reload is not supported.** Once a stream starts, the LLM is
committed to producing a response with the model and tools it started with.
Config changes do not affect a stream in progress — they take effect at the
next stream boundary.

**Re-resolve cost.** Each restart re-runs:

- `provider.model_details()` — typically cached, may hit the provider API
- `tool_definitions()` — may roundtrip to MCP servers if MCP tools changed
- `build_inquiry_backend()` — cheap local construction

For most config mutations (e.g., switching between two models from the same
provider), re-resolve is sub-100ms.
For changes that introduce new providers or MCP servers, it may be several
hundred ms.
This is acceptable because config mutations are rare compared to normal tool
calls.

Re-resolve failures are handled per [Re-Resolve Failures](#re-resolve-failures).

## Drawbacks

- **Prefix-based access control requires repetition.** Like [RFD 076]'s
  filesystem rules, each config rule is self-contained.
  A more specific deny rule must be added explicitly to restrict a sub-path that
  a broader rule grants.
  This is clear but verbose for complex policies.

- **Partial-merge semantics cannot express removals.** Setting a field to `null`
  in `outcome.config` is ambiguous (the `PartialAppConfig` model treats absent
  fields as "preserve").
  Removals go through the separate `outcome.unset` field.
  Tool authors must know about both channels.

- **Cache invalidation on cacheable config mutations.** Tools that modify
  cacheable config (system prompt, instructions, persistent attachments)
  invalidate provider-side token caches on the next request.
  Workspace owners can restrict writes to these paths via `access.config` rules
  if cache preservation matters more than mutability.

- **Side effects on re-invocation.** Tools that mutate config and have external
  side effects (file I/O, network calls) may execute those side effects multiple
  times if their delta is rejected.
  Tool authors must design for this.

- **Re-resolve latency between cycles.** Each config change triggers a
  between-cycle re-resolve of provider/model/tools.
  For most changes this is fast, but provider-switching or MCP tool refresh can
  add hundreds of ms to the turn.

## Alternatives

### Dedicated store system

Build a separate persistence layer for tool state, independent of `AppConfig`,
with custom events, rollback, and fork logic.

Rejected because it duplicates the config system's existing capabilities.
Two persistence systems for related problems is worse than one.

### Store-only scope, defer general config mutation

Limit `access.config` to a designated data namespace only.
Tools cannot read or write general config paths like `assistant.model`.

Rejected because the mechanism is identical regardless of which paths are
granted.
The access control model (path-based grants) already scopes what a tool can
touch.
Restricting to a single namespace would require lifting the restriction later
using the exact same infrastructure.

### Config mutation via dedicated API, not tool outcome

Instead of `outcome.config`, provide a separate `config_set()` function or
protocol message that tools call explicitly.

Rejected because it adds a second communication channel between the tool and JP.
The outcome-based approach is simpler: the tool returns all its results (content

- config changes) in a single response.
  JP processes both atomically.

## Non-Goals

- **Mid-stream config reload.** Once an LLM stream starts, the model and tool
  set are fixed.
  Config changes take effect at the next cycle boundary, not mid-response.
- **Cross-conversation config access.** Reading or writing another
  conversation's config is deferred to a follow-up RFD.
- **Builtin tool config access.** Builtin tools use a different execution path
  (`BuiltinTool::execute(arguments, answers)`) with no `Context` parameter at
  all.
  Giving builtins access to config requires a trait redesign that is out of
  scope here.
  The follow-up RFD introducing general-purpose `config_get` / `config_set`
  builtins is the natural place for that work.
- **Built-in `config_get` / `config_set` tools.** General-purpose config tools
  for the LLM are deferred to the same follow-up RFD.
  Domain-specific tools that use `access.config` internally (via the local tool
  protocol) are in scope.
- **Expanding the hardcoded sensitive path list via config.** The list is fixed
  in JP's source.
  Users opt into sensitive writes via `write = "insecure_allow"` per rule, but
  cannot add to or remove from the sensitivity list.
- **MCP tool config access.** MCP tools use a separate protocol that does not
  carry JP config.
  Extending MCP with config access is future work.
- **Schema enforcement on config values.** Validating config values returned by
  tools against user-defined schemas is future work.
  JP's own structural validation against the `AppConfig` schema is in scope.
- **Finer-grained capability split.** `write` covers all `outcome.config`
  operations; `delete` covers all `outcome.unset` operations.
  Finer distinctions (e.g., separating create from update within `write`) are
  not in scope — the current split matches the two distinct outcome channels.

## Risks and Open Questions

- **Dependency on RFD 070.** This RFD builds on multiple parts of RFD 070:

  - The `unsets` field on `ConfigDelta` and its path syntax (including
    vec-element `path["json"]` format) for `outcome.unset` support.
  - The `claims` field on `ConfigDelta` for storing `claims[path] = None` on
    tool-generated deltas.
  - The `None` explicit-unclaim semantics in claim walk-back, which protects
    tool mutations from `-C`-driven reversion.

  RFD 070 is Accepted but not yet implemented; this RFD's Phase 2 depends on RFD
  070's Phase 1 landing first.
  The concrete code touchpoints in RFD 070 that this RFD depends on are:

  - The `ConfigDelta` struct in `crates/jp_conversation/src/stream.rs` —
    currently `{ timestamp, delta }`, needs `unsets` and `claims` fields.
  - `ConversationStream::add_config_delta()` — must accept and store the new
    fields.
  - `ConversationStream::config()` replay — must apply unsets and carry claims
    forward through projection.
  - Persistence/deserialization paths for the new fields.

- **Merge semantics inheritance from partial config system.** By reusing the
  existing `PartialAppConfig` merge pipeline, tools inherit whatever field-level
  merge strategies the config author chose.
  If a tool author expects "replace" behavior but the field is `MergeableVec`
  with append semantics, their write appends instead.
  This is the same pitfall that `--cfg` users face, and the solution is the
  same: document field-level semantics and use `outcome.unset` for removals.

- **Tool access to sensitive config.** Allowing tools to write to paths like
  `assistant.model` is powerful but risky — a misbehaving tool could change the
  model mid-conversation.
  The per-rule `apply = "ask"` mode mitigates this for interactive use by
  surfacing every write to the user, but non-interactive mode needs careful
  defaults.
  For truly sensitive paths (`conversation.tools.*.access`, etc.), the `write =
  "insecure_allow"` requirement forces explicit acknowledgment in config.

- **Retry limit tuning.** The 3-rejection cap is arbitrary.
  Too low and legitimate self-correction loops fail; too high and broken tools
  waste cycles.
  Three feels right for typical cases (initial attempt, one correction, one
  fallback) but may need adjustment based on real usage.

- **Re-resolve failure recovery.** Structural validation cannot catch all
  semantic failures (see [Re-Resolve Failures](#re-resolve-failures)).
  When re-resolve fails, the delta is already persisted and the user recovers
  manually.
  Automatic rollback would require distinguishing transient failures (provider
  API down, retry might succeed) from persistent ones (model name genuinely
  invalid), which is beyond what JP can reliably infer.

- **Cross-field and cross-reference validation.** Structural validation via
  `PartialAppConfig` deserialization catches type and enum errors but not
  cross-field consistency (e.g., model-parameter compatibility) or external
  references (e.g., attachment file existence, model name availability from the
  provider).
  Tightening the config layer to catch these earlier would reduce re-resolve
  failures and improve error messages for `--cfg` users and tools alike.
  This is orthogonal to this RFD but worth scoping in a follow-up.

## Implementation Plan

### Phase 1: `access.config` grants

Add `ConfigRule` to [RFD 076]'s `AccessPolicy` alongside `FsRule`, `NetRule`,
and `EnvRule`.
Fields: `path`, `read`, `write` (`bool | "insecure_allow"`), `delete`, `apply`
(`"ask" | "unattended"`).
Implement longest prefix match evaluation for dot-separated config paths,
mirroring [RFD 076]'s filesystem path evaluation.
Implement single-segment `*` wildcard matching, validated against the schema to
reject wildcards on non-map paths.
Add the hardcoded sensitive path list and config-time validation that rejects
`write = true` on sensitive paths.
No tool protocol changes yet — this phase defines the data model and evaluation
logic.

Depends on: [RFD 076] access policy types.

### Phase 2: Outcome plumbing + apply pipeline + re-invocation

This phase lands the full end-to-end behavior as a single mergeable unit.
Splitting re-invocation into a follow-up phase is not safe: a silent-drop
intermediate state would let a tool's `content` ("Model changed to sonnet")
reach the LLM while JP had rejected the delta.

**Plumbing.** Restructure the structured outcome path across crates per
[Structured Outcome Transport](#structured-outcome-transport):

- Grow `jp_tool::Outcome::Success` with optional `config` and `unset` fields.
- Replace the bare `String` in `CommandResult::Success` and
  `ExecutionOutcome::Completed` with a structured payload that carries
  `content`, `config`, and `unset`.
- Surface the structured payload to the tool coordinator's `handle_tool_result`
  path via `ExecutorResult::Completed`.
- Keep `ToolCallResponse` unchanged — config mutations leave the pipeline as a
  separate `ConfigDelta` event, not as metadata on the response.

**Apply pipeline.** In the coordinator, before the existing result-mode
dispatch:

- Filter `context.config` to paths matched by rules with `read = true`.
- Deserialize `outcome.config` as `PartialAppConfig`.
- Parse `outcome.unset` as RFD 070 unset paths; validate each base path against
  the schema.
- Enumerate the concrete leaf paths touched by the deserialized partial and
  authorize each leaf against `write` rules; authorize each unset against
  `delete` rules.
- Enqueue apply chrome (`PendingPrompt::ApplyDelta`) if any matched leaf has
  `apply = "ask"`, batched across writes and unsets.
  Non-interactive sessions reject `apply = "ask"` deltas without prompting.
- On approval, add entries to the per-cycle commit buffer (living outside
  `ConversationMut`'s dirty-state tracking).

**Re-invocation.** On any delta failure (invalid, unauthorized, or
user-rejected), re-invoke the tool with `context.delta_rejection`, preserving
the original `arguments` and `answers`.
Enforce the 3-rejection retry limit; on exhaustion, abort the tool call with the
fallback LLM message.
Tool `content` is held until the delta is resolved (approved, rejected after
retries, or retry-exhausted) — it does not leak to the LLM during
re-invocation.

**Buffer commit.** At cycle end, fold the buffer by tool-call index into a
single merged `ConfigDelta` with `delta`, `unsets`, and `claims[path] = None`
for each modified or unset path.
Append one event to the stream.
On Restart (tool-execution interrupt), discard the buffer; on Continue / Stop &
Reply, commit normally.

**Chrome.** Approval prompts and informational notices (invalid delta, user
rejection, retry exhaustion).

Depends on: Phase 1, [RFD 070]'s `ConfigDelta` fields (`unsets`, `claims`),
`add_config_delta`, and replay changes landing first.

### Phase 3: Agentic loop restart

Restructure the turn loop to support between-cycle config reload.
After each executing phase, detect applied deltas and, if any, signal a new
between-cycle restart path to the outer loop.
`CycleResult::ConfigChanged` in this RFD is a **new** control-flow concept; the
current code has no `CycleResult` type and routes tool continuation through
`ExecutionResult { responses, restart_requested }` and `Action::SendFollowUp`.
This phase either extends that enum (or parallels it with a new signal) so the
turn loop can distinguish "continue with follow-up" from "reload config and
re-resolve".
The outer loop reloads `cfg` from the event stream and re-resolves derived
values (provider, model, tool definitions, tool choice, inquiry backend) before
re-entering the inner loop at `TurnPhase::Streaming`.

Concrete code touchpoints:

- `crates/jp_cli/src/cmd/query/turn_loop.rs::commit_tool_responses()` — must
  flush the per-cycle commit buffer alongside the existing tool response commit,
  emitting the merged `ConfigDelta` event to the stream.
- `crates/jp_cli/src/cmd/query/turn/coordinator.rs::TurnCoordinator::handle_tool_responses()`
  — must signal `CycleResult::ConfigChanged` to the outer loop when the buffer
  produced a delta.
- The current `ExecutionResult { responses, restart_requested }` flow —
  extended (or paralleled) to carry the merged delta from the coordinator to the
  turn loop without bypassing existing response routing.
- Outer-loop restart path — re-project config from the stream, re-resolve
  provider/model/tools/inquiry backend, re-enter at `TurnPhase::Streaming` (not
  `Idle`, which would re-emit `TurnStart`).

Handle re-resolve failures per [Re-Resolve Failures](#re-resolve-failures) —
turn aborts with a clear error, delta stays persisted, user recovers manually.

Depends on: Phase 2.

## References

- [RFD 076: Tool Access Grants][RFD 076] — the access model that
  `access.config` extends with a fourth resource type.
- [RFD 038: Config Reset Keywords][RFD 038] — how config propagates through
  conversation creation.
- [RFD 042: Tool Options][RFD 042] — `options` on tool configuration;
  `access.config` is part of the `access` model, not inside `options`.
- [RFD 070: Negative Config Deltas][RFD 070] — dependency for key removal
  semantics in `outcome.config`.

[RFD 038]: 038-config-reset-keywords.md
[RFD 042]: 042-tool-options.md
[RFD 070]: 070-negative-config-deltas.md
[RFD 076]: 076-tool-access-grants.md
