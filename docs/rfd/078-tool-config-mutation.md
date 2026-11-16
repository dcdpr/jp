# RFD 078: Tool Config Mutation

- **Status**: Accepted
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-04-17
- **Requires**: [RFD 076], [RFD 070]
- **Extended by**: [RFD 108]
- **Summary**: Grants tools scoped read and write access to `AppConfig` paths
  via `access.config` rules and per-cycle `ConfigDelta` commits.

## Summary

This RFD extends [RFD 076]'s access model with `access.config` rules that grant
tools scoped read and write access to `AppConfig` paths such as
`assistant.model`, `conversation.attachments`, and `conversation.tools.*`.
Tools receive granted values in `context.config` and return proposed changes in
`outcome.config`.
Approved mutations accumulate in a per-cycle commit buffer and persist as a
single merged `ConfigDelta` event on the conversation stream.

## Motivation

Tools cannot read or modify JP's configuration.
A tool cannot inspect the active model, adjust tool availability for the next
turn, or modify attachments.
Mid-conversation config changes are only possible through CLI flags or config
files.

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

The config system already supports persistence via `ConfigDelta`, fork
propagation, CLI seeding, and file-based defaults.
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

When no `access.config` rules are present, the tool has no config access,
matching [RFD 076]'s default-deny model.

#### Capabilities

| Field    | Description                                                          |
| -------- | -------------------------------------------------------------------- |
| `read`   | Read the current value of the config path                            |
| `write`  | Return updated values for the config path (via `outcome.config`)     |
| `delete` | Unset the config path or remove vec elements (via `outcome.unset`)   |
| `apply`  | Delta application mode: `"ask"` or `"unattended"` (default: `"ask"`) |

`read`, `write`, and `delete` are independent capabilities; none implies the
others.
A tool granted `write = true` without `read = true` writes blind and receives no
current value in `context.config`.
A tool granted `write` without `delete` can set or replace values, but cannot
remove them via `outcome.unset`.

`apply` controls whether JP prompts the user before applying a config delta.
It governs both `outcome.config` writes and `outcome.unset` removals, and
defaults to `"ask"`.
It is independent of the tool's `run` mode, which controls whether the tool
executes at all.
`apply` only governs writes and deletes; reads never prompt.

#### Sensitive paths and `write = "insecure_allow"`

JP maintains a hardcoded list of sensitive config paths where unintended writes
could compromise safety or security:

- `conversation.tools.*.access`: writing to a tool's own access policy allows
  self-escalation.

Additional paths will be added as they are identified.

**Wildcard semantics.** `*` is a single-segment wildcard that matches any key at
that position.
For example, `conversation.tools.*.access` matches
`conversation.tools.fs_modify_file.access` and `conversation.tools.foo.access`.

Wildcards are valid only where the underlying config type is a `Map<String,
...>` (for example, `conversation.tools` is `IndexMap<String, ToolConfig>`).
Wildcards expand the set of paths a rule covers, which only makes sense when the
key set is unbounded.
Struct fields have a fixed set of names and must be listed individually when
sensitive.
JP validates this at config load time; using `*` on a non-map schema type
produces a config error.

On sensitive paths, config validation rejects `write = true`.
The workspace owner must set `write = "insecure_allow"` to acknowledge the risk:

```
Config error: tool 'X' grants write = true to path 'conversation.tools.*.access'
which is a sensitive path. Use write = "insecure_allow" to explicitly
acknowledge this grant.
```

`insecure_allow` is only required on sensitive paths.
On non-sensitive paths, `write = true` and `write = "insecure_allow"` behave
identically.

**Orthogonality from `apply`.** Sensitivity acknowledgment (`write`) and
confirmation mode (`apply`) are independent:

- `write = "insecure_allow"` with `apply = "ask"` acknowledges sensitivity and
  still confirms each write interactively.
- `write = "insecure_allow"` with `apply = "unattended"` acknowledges
  sensitivity and skips prompts for non-interactive workflows.

`apply` defaults to `"ask"` regardless of the `write` value.
Because rules are self-contained and do not inherit from other rules, setting
`apply = "unattended"` on a sensitive path requires writing it explicitly on
that rule.

#### Evaluation: longest prefix match

When multiple rules match a config path, the most specific rule wins, measured
by dot-separated segment count.
The winning rule applies in full without inheriting capabilities from less
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
Because each rule is self-contained, it can be read in isolation without subtle
inheritance bugs, matching [RFD 076]'s filesystem rules.

### Structured Outcome Transport

The execution pipeline flattens a successful tool outcome to a bare `String` at
the first hop: `parse_command_output` converts `jp_tool::Outcome::Success {
content }` into `CommandResult::Success(content)`, which becomes
`ExecutionOutcome::Completed { result: Ok(String) }` and is stored as
`ToolCallResponse { result: Result<String, String> }`.
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
`outcome.config` and
`outcome.unset`](#output-outcome-config-and-outcome-unset)).

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
metadata on the tool response: `ToolCallResponse` records the LLM-facing
content, and `ConfigDelta` records the config change.

**Builtin tools share the wire type but not the semantics.**

`jp_tool::Outcome` is the shared return type for all tool execution paths,
including builtins.
Config mutations from builtins are out of scope for this RFD; their execution
path has no `Context` and no `access.config` plumbing.
If a builtin returns `Outcome::Success` with `config` or `unset` populated,
`jp_llm::tool::execute_builtin` (where builtin outcomes are mapped onto
`ExecutionOutcome::Completed`) **drops those fields and logs a warning** before
the outcome reaches the coordinator.
This keeps the wire type shared without honoring mutations from an unprivileged
path.
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
JP ignores those fields because formatter output is display-only and cannot
mutate config.
This matches [RFD 076]'s treatment of both actions as separate enforcement
surfaces.

**Enforcement is host-side.** Unlike [RFD 076]'s `fs`, `net`, and `env` rules,
which tools self-check via `Context`, JP enforces config access on the host.
JP filters `context.config` before sending it to the tool, validates
`outcome.config` and `outcome.unset` against grants before persisting, and
constructs the `ConfigDelta`.
The tool sees its grants in `context` for introspection (for example, to check
whether it can write a path), but cannot bypass host enforcement.
[RFD 076]'s planned OS-level enforcement extension unifies `fs`, `net`, and
`env` enforcement under JP as well; this RFD follows that direction from the
start.

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

Only paths matched by `access.config` rules with `read = true` are included, so
the tool sees a partial config tree scoped to its grants.
`write = true` or `delete = true` without `read = true` does not grant read
access; the tool writes or removes blind.
Tools without `access.config` rules receive no `config` field in `context`,
preserving backward compatibility with existing tools.

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

1. Deserializing `config` as `PartialAppConfig`, the same type used by `--cfg`
   flags and config files.
   Deserialization validates field types, enum variants, and schema structure.
2. Parsing `unset` as a list of dotted paths in [RFD 070]'s `unsets` format,
   including vec-element syntax (`path["serialized-json-value"]`).
3. Building a `ConfigDelta` with `delta = partial`, `unsets = paths`, and
   `claims = { path → None for each written or unset path }` (see [Claims on
   tool-generated deltas](#claims-on-tool-generated-deltas)).
4. Adding the delta to the per-cycle commit buffer, which emits a single merged
   `ConfigDelta` to the event stream at cycle end (see [Per-Cycle Commit
   Buffer](#per-cycle-commit-buffer)).

The tool works with plain config values and unset paths rather than constructing
a `ConfigDelta` directly.
Tools that omit `config` and `unset` produce no delta, preserving backward
compatibility with existing tools.

#### Merge semantics

`outcome.config` is deserialized as `PartialAppConfig` and merged through the
same config merge pipeline used by `--cfg` flags and config files:

- **Fields present** in `outcome.config` apply according to their field-level
  merge strategy.
  Scalar fields are replaced, `Option<T>` fields are set, and vec fields follow
  their declared `MergeableVec` strategy (append, replace, or another configured
  mode).
- **Fields absent** in `outcome.config` are preserved (`None` in the partial
  means "no change," not "clear").
- **Nested objects** merge recursively.
  Setting `{ "assistant": { "model": { "id": "sonnet" } } }` changes only
  `assistant.model.id` and leaves sibling fields under `assistant.model` (such
  as `parameters`) untouched.

These are the same semantics that govern `jp q --cfg
'assistant.model.id:="sonnet"'`.
Tool authors write partial configs without needing to know how each field type
merges; the field's declared merge strategy is authoritative.

Because partial-merge semantics cannot express `Some → None` or vec-element
removal, removals use `outcome.unset`, which parallels [RFD 070]'s `unsets`
field on `ConfigDelta`.

#### Claims on tool-generated deltas

Per [RFD 070], `ConfigDelta` events carry a `claims` map that records which
config source last set each field, enabling `-C flag` (file-based revert) to
find the values to walk back to.

Tool-generated deltas set `claims[path] = None` for every modified and unset
path.
`None` is the explicit-unclaim marker (also used by CLI shortcut flags such as
`--model`), signaling that the field is deliberately set to its current value
and should not be reverted by walking back through file-based claims.

As a result, `-C dev` does not revert tool-made changes.
To undo a tool's config mutation, the user uses the same mechanisms available
for any other persisted `ConfigDelta`: issuing a counter-change via `--cfg` or a
subsequent tool call, or editing the conversation's event stream directly.
This matches key-value `--cfg` assignments, which also omit file-attributed
claims so `-C file` cannot reach past them.

### Delta Application

After a tool returns `outcome.config` or `outcome.unset`, JP validates and
applies the resulting delta.

**Alias resolution for union fields.** Several config fields are untagged
unions, most notably `assistant.model.id` (`ModelIdOrAliasConfig`), which
accepts either a structured object (`{ provider, name }`) or an alias string
(`"sonnet"`).
Before authorization runs, JP normalizes the deserialized `PartialAppConfig` by
resolving every alias variant to its expanded `{ provider, name }` form using
the workspace's alias map.
This reuses `PartialModelIdOrAliasConfig::resolve_in_place`, which JP already
runs on `PartialAppConfig` values before persisting them as `ConfigDelta` events
in the stream.

Every downstream stage (leaf enumeration, authorization, apply chrome, claims
generation, and buffer folding) operates on this resolved tree.
A rule granting only `assistant.model.id.name` does **not** allow a provider
switch via alias expansion: the alias resolves first, and the `provider` leaf
must be covered independently.
If the alias is unknown and cannot be parsed as `provider/name`, JP rejects the
delta as `invalid_config` with the alias name in `detail`.

The application pipeline runs in five steps:

1. **Structural validation.** JP deserializes `outcome.config` as
   `PartialAppConfig`.
   Serde rejects:

   - Unknown field paths that do not exist in the schema.
   - Type mismatches (such as a string where a number is expected).
   - Invalid enum variants (such as `"provider": "bogus"` when `ProviderId` only
     accepts `anthropic | openai | ollama | ...`).

   Partial types are permissive: missing fields on populated subtrees are not
   flagged here because `required` constraints apply to the resolved config, not
   to partials.
   Required-field violations surface at re-resolve time.

   For `outcome.unset`, JP parses each path using [RFD 070]'s syntax and
   verifies that the base path exists in the schema and supports unsetting (an
   optional scalar, or a vec for element-level unsets).
   JP does not check whether a targeted vec element is present; element removal
   is idempotent per [RFD 070].

   Validation checks structure only, not semantic validity (such as whether a
   model name exists on the provider or an attachment file exists on disk).
   Semantic failures surface at re-resolve time (see [Re-Resolve
   Failures](#re-resolve-failures)).

2. **Authorization check.** Authorization checks the **concrete leaf paths**
   touched by the outcome rather than the top-level keys in the JSON tree.
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
   Each leaf must be covered by a rule with `write = true` or `write =
   "insecure_allow"`.
   A rule granting `write` at a parent path (such as `assistant.model.id`)
   covers every leaf beneath it via longest-prefix-match evaluation (see
   [Evaluation: longest prefix match](#evaluation-longest-prefix-match)).

   `outcome.unset` paths are already concrete paths; each must be covered by a
   rule with `delete = true`.

   If any leaf write or unset path falls outside granted access, JP rejects the
   entire delta with reason `unauthorized_paths`.

   Apply chrome (step 3) and claim generation (step 4) use the same leaf-path
   set, so authorization, confirmation, and claims cannot diverge.

3. **Apply chrome.** If any leaf path matched by the delta falls under a rule
   with `apply = "ask"`, JP displays a single prompt listing all proposed writes
   and removals:

   ```
   ⟳ Configuration delta proposed by tool 'change_model':
   
     assistant.model.id: "anthropic/opus" → "anthropic/sonnet"
     assistant.model.parameters.temperature: (unset)
   
   Apply delta? [Y/n/?]
   ```

   Multi-field deltas are batched into one prompt: approving applies all writes
   and unsets atomically, and rejecting discards the entire delta.

   **Non-interactive behavior.** When the session has no interactive prompt path
   (no TTY, `--non-interactive`, or a detached query) and any matched leaf
   requires `apply = "ask"`, JP rejects the delta with reason
   `confirmation_unavailable` (see [Delta Rejection and
   Re-Invocation](#delta-rejection-and-re-invocation)).
   Whereas `user_rejected` indicates an explicit decline by the user,
   `confirmation_unavailable` indicates that no prompt path exists.
   Tools can branch on this distinction (for example, falling back when
   confirmation is unavailable while respecting an explicit `user_rejected`).
   JP never auto-approves a write that requires confirmation.
   To mutate config in unattended environments, the workspace owner must set
   `apply = "unattended"` on the rule.

4. **Buffer.** Once approved (or when all matched rules set `apply =
   "unattended"`), JP appends the delta to the **per-cycle commit buffer**
   rather than writing to the event stream immediately (see [Per-Cycle Commit
   Buffer](#per-cycle-commit-buffer)).

5. **Deliver content.** The tool's `content` enters the result-mode pipeline
   (see [Interaction with Result Mode](#interaction-with-result-mode)).

If validation, authorization, or approval fails, JP rejects the delta and
re-invokes the tool.

### Delta Rejection and Re-Invocation

When delta application fails, JP re-invokes the tool with
`context.delta_rejection` describing the failure so the tool can adapt its
response or retry with corrected values.

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

JP re-invokes the tool with the original `arguments`, `answers`, and `options`,
plus `context.delta_rejection`.
Accumulated `answers` from prior `needs_input` rounds are preserved so the
re-invoked run sees the same input state as the initial call:

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

The tool inspects `context.delta_rejection` and may:

- Return new `content` without `config` or `unset`, reporting that the change
  was not applied.
- Return new `content` with corrected `config` or `unset` values.
- Return identical output, which will be rejected again.

Each re-invocation runs through the same validation, authorization, approval,
and buffering pipeline, incrementing the retry counter on each rejection.

**Retry limit:**

JP allows at most 3 delta rejections per tool call.
On the 4th failure, the tool call aborts and JP **synthesizes a
`ToolCallResponse`** with `result = Err(fallback_message)` so the conversation
event stream and the LLM's view of the tool call remain well-formed.
The synthesized response is an ordinary tool-result event routed through
`commit_tool_responses()`, not a new event type or separate host-to-LLM channel:

```
Tool 'change_model' failed to produce a valid config delta after 3 attempts.
Last error: unknown variant 'bogus' at assistant.model.id.provider.
```

**Side effects on re-invocation:**

Because a re-invoked tool executes multiple times, any external side effects
outside `outcome.config` and `outcome.unset` (file writes, network calls,
subprocess launches) also repeat.
Tools that use `access.config` must be idempotent or guard against re-entry.

### Interaction with Result Mode

The tool pipeline in `crates/jp_cli/src/cmd/query/tool/coordinator.rs` handles
completed tools through a per-tool `ResultMode` dispatch (`Unattended`, `Ask`,
`Edit`, or `Skip`) that lets the user approve, edit, or skip the tool's
`content` before it reaches the LLM.

Config delta processing runs **before** result-mode dispatch inside
`handle_tool_result` when `ExecutorResult::Completed` arrives:

1. **Tool execution completes** with an outcome that may include `config` and
   `unset`.

2. **Config delta processing** runs:

   - Structural validation (see [Delta Application](#delta-application)).
   - Authorization check against `access.config` rules.
   - Apply chrome if any matching rule has `apply = "ask"`.
   - On rejection (invalid, unauthorized, or user-rejected), re-invoke the tool
     with `context.delta_rejection` and restart step 2.
     Increment the retry counter, and abort the tool call when retries are
     exhausted.
   - On approval, append the delta to the per-cycle commit buffer (see
     [Per-Cycle Commit Buffer](#per-cycle-commit-buffer)).
     The delta is not yet in the event stream.

3. **Result mode processing** routes the tool's `content` through
   `handle_tool_result` according to the tool's `ResultMode` (`Unattended`,
   `Ask`, `Edit`, or `Skip`).

**Config and content are independent.** Apply chrome in step 2 gates config
mutations, while result mode in step 3 gates content delivery.
Choosing `Skip` in result mode hides `content` from the LLM without revoking a
provisional config approval.
Whether a provisionally approved delta is committed to the event stream is
decided at cycle end by how the cycle closes (see [Cycle-termination
semantics](#cycle-termination-semantics)).

**Re-invocation avoids result-mode friction.** Because re-invocation happens in
step 2 before result mode, the user never edits `content` that a delta rejection
would subsequently discard.
By the time `content` reaches result mode, the delta has already been buffered
or abandoned.

#### Prompt ordering

The coordinator runs a single FIFO queue of `PendingPrompt` entries with
`Question` (tool asking for input) and `ResultMode` (user approving tool
content).
This RFD adds `ApplyDelta`:

```rust
enum PendingPrompt {
    Question { index: usize, question: Question },
    ResultMode { index: usize, tool_id: String, ... },
    ApplyDelta { index: usize, tool_id: String, delta: ProposedDelta },
}
```

`ApplyDelta` is enqueued when a tool completes with an authorized
`outcome.config` or `outcome.unset` whose matched rules include `apply = "ask"`:

- The queue remains FIFO.
  Apply chrome does not preempt a queued prompt from another tool; a `Question`
  prompt from tool B that was already queued when tool A finished is served
  before tool A's apply chrome.
- For a given tool, `ApplyDelta` is enqueued before `ResultMode`, resolving
  config before content.
- Only one prompt is active at a time (`prompt_active` guards the queue).

Prompts may therefore interleave across tools (for example: question for B,
apply chrome for A, result mode for A, result mode for B) depending on
completion order, matching the existing behavior for question and result-mode
prompts.

### Per-Cycle Commit Buffer

Approved config deltas are held in a per-cycle commit buffer during the
executing phase.
The buffer is a list of `(tool_call_index, PartialAppConfig, Vec<UnsetPath>)`
entries, one per approved tool invocation, appended as tools complete.

**The live event stream sees nothing until the buffer commits.** Config deltas
approved in apply chrome are provisional and remain in memory until cycle end.

#### Within-cycle visibility

Tools within the same cycle do not see each other's provisional deltas; each
tool's `context.config` reflects the config state at cycle start.
Re-invoking a rejected tool also provides the cycle-start state rather than the
buffer's accumulating contents, keeping re-invocation deterministic regardless
of what other tools in the cycle do.

If a tool needs to observe another tool's config change, the second tool must
run in a later cycle (a subsequent LLM round-trip), matching the existing
constraint that tools cannot observe each other's output content within the same
cycle.

#### Commit at cycle end

At cycle end (once all tools complete and result-mode processing finishes), JP
folds the buffer into a single `ConfigDelta` event:

1. **Sort** buffer entries by `tool_call_index` (the order the LLM emitted the
   tool calls).
   Call order is more deterministic than completion order and matches the order
   recorded in conversation history.

2. **Fold per-path** in call order.
   For each config path touched by any buffered entry, the last operation in
   call order determines the final state:

   - If the last operation was a set (via `outcome.config`), the path is
     included in the merged `PartialAppConfig` with that value.
   - If the last operation was an unset (via `outcome.unset`), the path is
     included in the merged `unsets` list.

   `ConfigDelta.delta` and `ConfigDelta.unsets` therefore never reference the
   same path.

3. **Merge claims.** Union all modified and unset paths, mapping each to `None`
   (the explicit-unclaim marker; see [Claims on tool-generated
   deltas](#claims-on-tool-generated-deltas)).

4. **Emit** one `ConfigDelta { delta, unsets, claims }` event.
   The cycle then triggers re-resolve and proceeds to the next stream.

At most one `ConfigDelta` event is emitted per cycle, regardless of how many
tools contributed to it.
An empty buffer emits no event, and the cycle completes without re-resolving
config.

#### Cycle-termination semantics

The per-cycle commit buffer is in-memory state owned by the cycle coordinator,
and its fate depends on how the executing phase ends.
JP's tool-execution interrupt menu offers three choices:

- **Continue**: leaves the buffer unchanged.
  Tools keep running, new approvals append as they complete, and the cycle
  commits when all tools finish.

- **Stop & respond**: cancels in-flight tools and runs the normal cycle-end
  commit path.
  Deltas already approved at apply chrome commit to the event stream as a single
  merged `ConfigDelta`.
  The user's reply text is attached to each cancelled tool as its response
  content (wrapped in JP's cancellation message), so the LLM sees the cancelled
  tool responses rather than a new user request.
  Cutting the cycle short does not revoke explicit approvals from apply chrome.

- **Restart**: cancels in-flight tools and discards the buffer.
  The tool batch replays from the LLM's prior response, and the replayed tool
  invocations re-propose their deltas and re-prompt the user for approval,
  avoiding duplicate application from a partially completed run.

**Streaming interrupts** (`Continue`, `Reply`, `Stop`, and `Abort`) fire during
LLM streaming, not during tool execution.
Because no tools have run yet in the current cycle and no prior cycle's buffer
carries over into a new streaming phase, the buffer is always empty when a
streaming interrupt fires.
Deltas committed in earlier cycles of the same turn are already in the event
stream and remain unaffected.

**Process-level termination** (SIGKILL, crash, or OS-level kill) destroys the
in-memory buffer along with the process.
Because the buffer is not tracked by `ConversationMut`'s dirty-state
persistence, drop-persistence cannot leak provisional deltas to the event
stream.

**User implication.** Approvals in apply chrome remain provisional until the
cycle closes via Continue or Stop & respond.
If a user approves a delta in apply chrome and then selects Restart, the batch
and its provisional approvals are discarded so the replayed run starts clean.

The buffer is a plain `Vec` owned by the cycle coordinator outside
`ConversationMut`'s dirty-state tracking, cleared on Restart and flushed to the
stream on Continue or Stop & respond.

### Re-Resolve Failures

Structural validation at delta-application time does not check semantic
validity.
A delta such as `assistant.model.id.name = "nonexistent"` is a valid string at a
valid schema path, so it passes structural checks, but fails when the turn loop
re-resolves config and queries model details from the provider.

If re-resolve fails, the turn aborts with an error naming the offending config
path while the `ConfigDelta` remains in the event stream.
The user recovers by:

- Issuing a corrective config change (via `--cfg`, a tool, or editing the
  conversation), or
- Correcting the config manually and re-running the conversation.

JP does not attempt automatic rollback on re-resolve failure because
distinguishing transient failures (such as a provider API outage where a retry
would succeed) from persistent ones (such as an invalid model name) is not
reliable.

Cross-field and cross-reference validation at delta-application time (such as
checking whether a model name exists for the chosen provider) would benefit
`--cfg` and config files as well; see [Risks and Open
Questions](#risks-and-open-questions).

### JP-to-LLM Communication

This RFD introduces **no new host-to-LLM channel**.
Tool content still flows through the result-mode pipeline, which can replace or
edit `content` before it reaches the LLM: `Skip` substitutes `"Result delivery
skipped by configuration."` or `"Result delivery skipped by user."`, `Edit` lets
the user rewrite the content, and `Ask` can reject delivery.

When a tool exhausts its delta retry limit, JP authors the response content
directly by emitting a **synthesized `ToolCallResponse`** on the normal event
stream rather than introducing a new event type or side-channel.

### User-Facing Chrome for Delta Events

JP displays terminal chrome for four delta events:

- **Approval prompt.** Shown when a delta matches any rule with `apply = "ask"`,
  batched across multi-field deltas.

- **Invalid delta.** Informational notice without a prompt:

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

A turn consists of one or more cycles (stream an LLM response, execute tool
calls, stream the next response).
When a tool produces a config delta during the executing phase, the cycle
completes, JP commits the delta, and the agentic loop restarts with the
re-resolved config before the next stream begins:

1. The executing phase completes with all approved deltas in the per-cycle
   commit buffer (see [Per-Cycle Commit Buffer](#per-cycle-commit-buffer)).
2. If the buffer is non-empty, JP folds its entries into a single merged
   `ConfigDelta`, appends it to the event stream, and returns
   `CycleResult::ConfigChanged` from the inner loop.
3. The outer turn loop reloads `cfg` from the event stream (re-projecting across
   all events, including the new `ConfigDelta`) and re-resolves the derived
   values: provider, model, tool definitions, tool choice, and inquiry backend.
4. The inner loop re-enters at `TurnPhase::Streaming` (rather than `Idle`, which
   would emit another `TurnStart` event).
   The LLM receives the accumulated thread, including the tool's response
   content, and continues.

**Mid-stream reload is not supported.** Once a stream starts, the model and tool
set are fixed for that response; config changes take effect at the next stream
boundary.

**Re-resolve cost.** Each restart re-runs:

- `provider.model_details()`: typically cached, may query the provider API.
- `tool_definitions()`: may round-trip to MCP servers if MCP tools changed.
- `build_inquiry_backend()`: local construction.

For most config mutations (such as switching between two models from the same
provider), re-resolution takes under 100ms.
Changes that introduce new providers or MCP servers may take several hundred
milliseconds, which is acceptable because config mutations are infrequent
compared to ordinary tool calls.

Re-resolve failures follow [Re-Resolve Failures](#re-resolve-failures).

## Drawbacks

- **Prefix-based access control requires repetition.** Like [RFD 076]'s
  filesystem rules, each config rule is self-contained.
  Restricting a sub-path under a broader grant requires an explicit, more
  specific deny rule, which is verbose for complex policies.

- **Partial-merge semantics cannot express removals.** Because
  `PartialAppConfig` treats absent fields as "preserve", setting a field to
  `null` in `outcome.config` is ambiguous.
  Removals require the separate `outcome.unset` field, so tool authors must know
  both channels.

- **Cache invalidation on cacheable config mutations.** Modifying cacheable
  config (system prompt, instructions, persistent attachments) invalidates
  provider-side token caches on the next request.
  Workspace owners can restrict writes to these paths via `access.config` rules
  when cache preservation matters more than mutability.

- **Side effects on re-invocation.** Tools with external side effects (file I/O,
  network calls) may repeat those side effects if their delta is rejected and
  the tool is re-invoked.

- **Re-resolve latency between cycles.** Each config change triggers a
  between-cycle re-resolve of the provider, model, and tools.
  Most changes are fast, but switching providers or refreshing MCP tools can add
  hundreds of milliseconds to the turn.

## Alternatives

### Dedicated store system

Build a separate persistence layer for tool state, independent of `AppConfig`,
with custom events, rollback, and fork logic.

Rejected because it duplicates the config system's persistence, fork, and
rollback capabilities.

### Store-only scope, defer general config mutation

Limit `access.config` to a designated data namespace so tools cannot read or
write general config paths such as `assistant.model`.

Rejected because the mechanism is identical regardless of which paths are
granted.
Path-based grants already scope what a tool can touch, and restricting access to
a single namespace would only require lifting that restriction later on the same
infrastructure.

### Config mutation via dedicated API, not tool outcome

Provide a separate `config_set()` function or protocol message that tools call
explicitly instead of returning `outcome.config`.

Rejected because it adds a second communication channel between the tool and JP.
Returning content and config changes together in `Outcome` is simpler and lets
JP process both atomically.

## Non-Goals

- **Mid-stream config reload.** Once an LLM stream starts, the model and tool
  set are fixed.
  Config changes take effect at the next cycle boundary, not mid-response.
- **Cross-conversation config access.** Reading or writing another
  conversation's config is deferred to a follow-up RFD.
- **Builtin tool config access.** Builtin tools execute via
  `BuiltinTool::execute(arguments, answers)` without a `Context` parameter.
  Giving builtins config access requires a trait redesign, which is deferred to
  the follow-up RFD introducing general-purpose `config_get` and `config_set`
  builtins.
- **Built-in `config_get` / `config_set` tools.** General-purpose config tools
  for the LLM are deferred to the same follow-up RFD.
  Domain-specific tools that use `access.config` over the local tool protocol
  are in scope.
- **Expanding the hardcoded sensitive path list via config.** The list is fixed
  in JP's source.
  Users opt into sensitive writes via `write = "insecure_allow"` per rule, but
  cannot add to or remove from the sensitivity list.
- **MCP tool config access.** MCP tools use a separate protocol that does not
  carry JP config.
  Extending MCP with config access is future work.
- **Schema enforcement on config values.** Validating config values returned by
  tools against user-defined schemas is future work.
  JP's structural validation against the `AppConfig` schema is in scope.
- **Finer-grained capability split.** `write` covers all `outcome.config`
  operations and `delete` covers all `outcome.unset` operations.
  Finer distinctions (such as separating create from update within `write`) are
  out of scope because the two capabilities match the two outcome channels.

## Risks and Open Questions

- **Dependency on RFD 070.** This RFD builds on three parts of [RFD 070]:

  - The `unsets` field on `ConfigDelta` and its path syntax (including
    vec-element `path["json"]` format) for `outcome.unset`.
  - The `claims` field on `ConfigDelta` for storing `claims[path] = None` on
    tool-generated deltas.
  - The `None` explicit-unclaim semantics in claim walk-back, which prevent `-C`
    from reverting tool mutations.

  [RFD 070] is Accepted; Phase 2 of this RFD depends on Phase 1 of [RFD 070]
  landing first:

  - `ConfigDelta` in `crates/jp_conversation/src/stream.rs`: add `unsets` and
    `claims` alongside `{ timestamp, delta }`.
  - `ConversationStream::add_config_delta()`: accept and store the new fields.
  - `ConversationStream::config()` replay: apply unsets and carry claims forward
    through projection.
  - Persistence and deserialization paths for the new fields.

- **Merge semantics inheritance from partial config system.** Reusing the
  `PartialAppConfig` merge pipeline means tools inherit each field's configured
  merge strategy.
  If a tool author expects replacement on a `MergeableVec` field configured with
  append semantics, the write appends instead.
  This is the same constraint `--cfg` users face: document field-level semantics
  and use `outcome.unset` for removals.

- **Tool access to sensitive config.** Granting tools write access to paths such
  as `assistant.model` lets a misbehaving tool switch models mid-conversation.
  `apply = "ask"` mitigates this in interactive sessions by prompting on every
  write, while sensitive paths (`conversation.tools.*.access` and others)
  require `write = "insecure_allow"` in config.

- **Retry limit tuning.** The 3-rejection cap balances self-correction against
  wasted cycles (initial attempt, one correction, one fallback), and can be
  adjusted based on usage.

- **Re-resolve failure recovery.** Structural validation cannot catch all
  semantic failures (see [Re-Resolve Failures](#re-resolve-failures)).
  When re-resolve fails, the delta is already persisted and the user recovers
  manually because JP cannot reliably distinguish transient provider failures
  from invalid configuration values.

- **Cross-field and cross-reference validation.** Deserializing
  `PartialAppConfig` catches type and enum errors, but not cross-field
  consistency (such as model-parameter compatibility) or external references
  (such as attachment file existence or model availability on the provider).
  Catching these in the config layer would reduce re-resolve failures and
  improve errors for both `--cfg` and tools, and can be scoped in a follow-up.

## Implementation Plan

### Phase 1: `access.config` grants

Add `ConfigRule` to [RFD 076]'s `AccessPolicy` alongside `FsRule`, `NetRule`,
and `EnvRule` with fields `path`, `read`, `write` (`bool | "insecure_allow"`),
`delete`, and `apply` (`"ask" | "unattended"`).
Implement longest-prefix-match evaluation for dot-separated config paths,
mirroring [RFD 076]'s filesystem path evaluation, and single-segment `*`
wildcard matching validated against the schema to reject wildcards on non-map
paths.
Add the hardcoded sensitive path list and config-time validation that rejects
`write = true` on sensitive paths.
This phase defines the data model and evaluation logic without tool protocol
changes.

Depends on: [RFD 076] access policy types.

### Phase 2: Outcome plumbing + apply pipeline + re-invocation

Land the end-to-end outcome transport, apply pipeline, and re-invocation loop as
a single mergeable unit.
Splitting re-invocation into a later phase would let a tool's `content` ("Model
changed to sonnet") reach the LLM after JP rejected the delta.

**Plumbing.** Restructure the outcome path across crates per [Structured Outcome
Transport](#structured-outcome-transport):

- Add optional `config` and `unset` fields to `jp_tool::Outcome::Success`.
- Replace the bare `String` in `CommandResult::Success` and
  `ExecutionOutcome::Completed` with a structured payload carrying `content`,
  `config`, and `unset`.
- Surface the structured payload to `handle_tool_result` via
  `ExecutorResult::Completed`.
- Keep `ToolCallResponse` unchanged so config mutations leave the pipeline as a
  separate `ConfigDelta` event rather than response metadata.

**Apply pipeline.** In the coordinator, before result-mode dispatch:

- Filter `context.config` to paths matched by rules with `read = true`.
- Deserialize `outcome.config` as `PartialAppConfig`.
- Parse `outcome.unset` as [RFD 070] unset paths and validate each base path
  against the schema.
- Enumerate the concrete leaf paths in the deserialized partial, authorize each
  leaf against `write` rules, and authorize each unset path against `delete`
  rules.
- Enqueue `PendingPrompt::ApplyDelta` when any matched leaf has `apply = "ask"`,
  batched across writes and unsets.
  Reject `apply = "ask"` deltas in non-interactive sessions without prompting.
- On approval, append entries to the per-cycle commit buffer outside
  `ConversationMut`'s dirty-state tracking.

**Re-invocation.** On any delta rejection (invalid, unauthorized, user-rejected,
or confirmation-unavailable), re-invoke the tool with `context.delta_rejection`
while preserving `arguments` and `answers`.
Enforce the 3-rejection retry limit and abort the tool call with the synthesized
error response when exhausted.
Hold tool `content` until the delta is resolved so rejected runs never leak
content to the LLM.

**Buffer commit.** At cycle end, fold the buffer in `tool_call_index` order into
a single merged `ConfigDelta` with `delta`, `unsets`, and `claims[path] = None`
for each modified or unset path, and append one event to the stream.
Discard the buffer on Restart; commit it on Continue or Stop & respond.

**Chrome.** Render approval prompts and informational notices for invalid
deltas, user rejections, and retry exhaustion.

Depends on: Phase 1, and [RFD 070]'s `ConfigDelta` fields (`unsets`, `claims`),
`add_config_delta`, and replay support.

### Phase 3: Agentic loop restart

Restructure the turn loop to reload config between cycles.
When the executing phase commits a `ConfigDelta`, signal
`CycleResult::ConfigChanged` to the outer loop by extending or paralleling
`ExecutionResult { responses, restart_requested }` and `Action::SendFollowUp` so
the turn loop distinguishes a normal follow-up from a config reload.
The outer loop reloads `cfg` from the event stream and re-resolves derived
values (provider, model, tool definitions, tool choice, and inquiry backend)
before re-entering the inner loop at `TurnPhase::Streaming`.

Code touchpoints:

- `crates/jp_cli/src/cmd/query/turn_loop.rs::commit_tool_responses()`: flush the
  per-cycle commit buffer alongside tool responses and emit the merged
  `ConfigDelta` event to the stream.
- `crates/jp_cli/src/cmd/query/turn/coordinator.rs::TurnCoordinator::handle_tool_responses()`:
  signal `CycleResult::ConfigChanged` to the outer loop when the buffer emits a
  delta.
- `ExecutionResult { responses, restart_requested }`: carry the merged delta
  from the coordinator to the turn loop without bypassing response routing.
- Outer-loop restart path: re-project config from the stream, re-resolve the
  provider, model, tools, and inquiry backend, and re-enter at
  `TurnPhase::Streaming` rather than `Idle` (which would emit another
  `TurnStart`).

Handle re-resolve failures per [Re-Resolve Failures](#re-resolve-failures):
abort the turn with an error naming the offending path while keeping the
`ConfigDelta` in the stream.

Depends on: Phase 2.

## References

- [RFD 076: Tool Access Grants][RFD 076]: the access model that `access.config`
  extends with a fourth resource type.
- [RFD 038: Config Reset Keywords][RFD 038]: how config propagates through
  conversation creation.
- [RFD 042: Tool Options][RFD 042]: `options` on tool configuration;
  `access.config` belongs to the `access` model rather than `options`.
- [RFD 070: Negative Config Deltas][RFD 070]: key-removal and claim semantics
  used by `outcome.unset` and tool-generated deltas.

[RFD 038]: 038-config-reset-keywords.md
[RFD 042]: 042-tool-options.md
[RFD 070]: 070-negative-config-deltas.md
[RFD 076]: 076-tool-access-grants.md
[RFD 108]: 108-discoverable-tools.md
