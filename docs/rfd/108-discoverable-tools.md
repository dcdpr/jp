# RFD 108: Discoverable Tools

- **Status**: Discussion
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-09-08
- **Extends**: [RFD 081], [RFD 078]
- **Summary**: Adds a `discoverable` tool state whose schema reaches the model
  only after a built-in `search_tools` query matches it.

## Summary

Tool enablement gains a third state, `discoverable`: a tool is permitted to run,
but its schema is omitted from provider requests until the model searches for
it.
A built-in `search_tools` tool queries the permitted set and promotes matching
tools to `on` for the rest of the conversation.
This keeps large tool catalogues out of the context window without disabling
tools the model may need.

## Motivation

JP sends every enabled tool's definition on every request.
That works while the catalogue is small, but degrades in two ways as it grows:

- **Context cost.** A multi-server MCP setup spends tens of thousands of tokens
  on tool definitions before the model does any work.
- **Selection accuracy.** Anthropic reports that model tool selection degrades
  past roughly 30-50 available tools ([tool search][anthropic-tool-search],
  [Advanced tool use][anthropic-advanced]).
  That figure is unverified against JP's catalogue, but the direction is clear,
  and JP's catalogue grows with every added MCP server.

JP already has one tier of progressive disclosure:
`ToolDocs::schema_description` puts `summary` in the tool schema, and the
`describe_tools` builtin returns the full documentation on request.
This RFD adds a tier below that: the tool is omitted from the schema entirely
until the model searches for it.

Without this tier, the context window sets the ceiling on JP's tool catalogue.

## Design

### Configuration

A tool's `enable` setting accepts a third value:

```toml
# Offered up front, as today.
[conversation.tools.fs_read_file]
enable = "on"

# Permitted, but not offered until the model searches for it.
[conversation.tools.github_list_workflow_runs]
enable = "discoverable"

# Not offered, not discoverable.
[conversation.tools.fs_delete_file]
enable = "off"
```

The string shorthands on `enable` are presets across both of its fields:
`"always"` sets locked-on and `"explicit"` sets off-unless-named.
`"discoverable"` also expands to both fields:

```toml
enable = "discoverable"
# is exactly
enable = { availability = "discoverable", allow_toggle = "any" }
```

The availability-only table form is different.
An omitted table field stays unset and inherits from `[conversation.tools.'*']`
([RFD 081]).
Writing `enable = { availability = "discoverable" }` preserves the toggle policy
from the defaults layer, whereas the `"discoverable"` shorthand overwrites
`allow_toggle` with `"any"`.
When the defaults layer sets `allow_toggle = "if_named"`, only the shorthand
allows a bulk `-t` to enable the tool.

For a large MCP server, a practical default is to mark
`[conversation.tools.'*']` as discoverable and set two or three common tools to
`on`.
Using the string shorthand in `[conversation.tools.'*']` likewise sets both
`availability` and `allow_toggle`.

**`explicit` and `discoverable` differ in who asks for the tool.** `explicit`
withholds a tool until the *user* names it on the command line.
`discoverable` withholds it until the *model* finds it through search.
The two compose: `enable = { availability = "discoverable", allow_toggle =
"if_named" }` lets the model discover the tool while preventing a bulk `-t` from
enabling it.

### CLI directives

`allow_toggle` still controls which `--tool` and `--no-tool` directives may
change a tool at bulk, named, and named-group scope.
When `allow_toggle` permits a directive, it transitions the three availability
states as follows:

| Directive          | `off`  | `discoverable` | `on`    |
| ------------------ | ------ | -------------- | ------- |
| `-t` / `--tool`    | → `on` | → `on`         | no-op   |
| `-T` / `--no-tool` | no-op  | → `off`        | → `off` |

A bulk `-t` promotes every discoverable tool whose `allow_toggle` permits bulk
toggling.
`-t` means "give the model everything"; tools that should stay discoverable
under a bulk `-t` can set `allow_toggle = "if_named"`.

**Forced tool use remains a separate axis.** `jp q -u NAME` applies to a single
turn: JP reads it from CLI flags at query time rather than layering it into
conversation config, and `tool_definitions` includes a forced tool in the
definition set even when disabled (unless locked off).
Forcing a discoverable tool offers it for that invocation without emitting a
promotion delta or modifying its stored availability.
A persisted `assistant.tool_choice` naming a function keeps the same exemption
on every turn.

Because forcing can place an `off` tool into the resolved definition set, that
set does not reflect availability alone and cannot serve as the search index.

### Availability state

Two questions determine how JP presents a tool:

1. May the model call it?
2. Is its definition included in context up front, or only after discovery?

Question 2 is meaningless when the answer to question 1 is no, leaving three
valid states.
Encoding them as a three-variant enum rather than two booleans makes the invalid
state unrepresentable:

```rust
pub enum Availability {
    /// Not offered, not discoverable.
    Off,
    /// Permitted, offered once discovered.
    Discoverable,
    /// Offered up front.
    On,
}
```

This widens `Enable::state`, which is a `bool` in [RFD 081].
Its sibling field, `allow_toggle`, is unchanged.

### Permitted and offered tool sets

A single `Vec<ToolDefinition>` currently serves four roles: it populates the
provider's `tools` array, builds the documentation map for `describe_tools`,
gates tool execution in the executor, and determines (via `is_enabled()`)
whether to start an MCP server.
A single set cannot represent a discoverable tool, which must be searchable,
describable, and callable once promoted while remaining out of the provider's
`tools` array.

The tool set therefore splits into two sets, where the second is a strict subset
of the first:

- The **permitted set** contains every tool the model may call on this
  invocation: tools with availability `discoverable` or `on`, plus any tool
  named by the forced-tool exemption, minus any tool locked off.
  Every member is fully resolved, including the MCP round-trip that fetches a
  server tool's schema and description.
- The **offered set** is the subset sent to the provider: tools with
  availability `on`, plus any forced tool.

The forced-tool exemption must widen the permitted set, not just the offered
set.
Running `jp q -u NAME` on an `off` tool is supported, and JP builds a tool
executor by looking up the tool's definition in the permitted set; offering a
tool that is missing from the permitted set would let the model call a tool JP
cannot execute.
Similarly, `configure_active_mcp_servers` starts a forced tool's MCP server
regardless of its enable state so its schema can be resolved.

Each consumer reads from one of these sets:

| Consumer               | Set                                        | Why                                                  |
| ---------------------- | ------------------------------------------ | ---------------------------------------------------- |
| Provider `tools` array | offered                                    | avoids sending schemas for undiscovered tools        |
| `search_tools`         | availability `discoverable`, minus offered | an `off` tool never appears in search, forced or not |
| `describe_tools`       | permitted                                  | a found tool must be explainable before it is called |
| Executor eligibility   | permitted                                  | a promoted tool is callable in the same turn         |
| MCP server startup     | permitted                                  | an unstarted server has no metadata to search        |

The `search_tools` row filters by `discoverable` availability rather than
subtracting offered from permitted so that forced `off` tools (and any future
permitted-set additions) never appear in search results.

**Discovery saves context, not startup.** An MCP server with discoverable tools
starts with the conversation and JP fetches its schemas up front, just like an
`on` tool.
Eager resolution makes the permitted set searchable, so the savings are in
prompt tokens rather than startup latency or process count.
Lazy server resolution (starting an MCP server only when `search_tools` queries
it) can be added later if needed.

If a tool's backing MCP server fails to start, JP drops the tool from the
permitted set, matching `tool_definitions` behavior.
Unavailable tools are excluded from search results so the model does not waste
tokens calling a tool that cannot run.

### Renaming `state` to `availability`

`state` was passable for a boolean, but for a three-valued enum sitting next to
`allow_toggle`, `availability` states what the field actually controls.
The field is renamed to `availability`, and `state` remains as a permanent
deserialization alias mapping `true` to `on` and `false` to `off`.

The alias is permanent because `enable = { state = false }` appears in stored
conversation configs and `ConfigDelta` entries in existing streams, which are
read without being rewritten.

Three code paths read or write this field, so each must handle the alias:

- **Deserialization.** The custom `Deserialize` implementation on
  `PartialEnableConfig` accepts `state` alongside `availability`, mapping `true`
  to `on` and `false` to `off`.
- **Key assignment.** `--cfg conversation.tools.foo.enable.state=false` goes
  through `AssignKeyValue`, which matches key strings directly without invoking
  the deserializer.
  Its `state` arm remains alongside the new `availability` arm.
- **File editing.** Handled on write as described below.

#### Normalizing legacy `state` keys on write

A read-only alias would cause `jp config set` to produce tables containing both
spellings.
`config set` serializes the delta using the canonical key name and deep-merges
it into the existing TOML document, updating only the keys present in the delta.
Setting `availability` on a tool configured with `enable = { state = false }`
would otherwise produce:

```toml
enable = { state = false, availability = "discoverable" }
```

**Writing to a tool's `enable` table removes any sibling `state` key.** This
normalization is scoped to the single table being edited; every other `state`
key in that file, in other config files, and in stored conversation streams
remains untouched.
Config files that JP never modifies are never rewritten.

**Having both `state` and `availability` in the same table is an error.**
Because write normalization prevents JP from generating both keys, their
co-occurrence only happens through manual editing.
Picking one key by precedence would silently ignore the other; for example,
ignoring `state = false` next to `availability = "on"` would expose a tool the
user intended to disable.
Deserialization rejects tables containing both keys and names which key to keep.

### `search_tools`

`search_tools` is a builtin tool with the following signature:

```
search_tools(query: string, limit?: integer = 10) -> [{ name, summary }]
```

**Registration is monotone within a conversation.** JP offers `search_tools`
once any config position in the conversation stream has at least one
`discoverable` tool, and keeps offering it for the rest of the conversation.
Like tool availability, registration is derived from the stream ([RFD 054])
rather than held in memory, so it persists across separate CLI invocations.

Both cases follow from this rule:

- Adding `--cfg conversation.tools.deploy.enable=discoverable` to an active
  conversation exposes `search_tools` on the next turn because the delta records
  a discoverable tool in the stream.
- Promoting every discoverable tool in a conversation does not remove
  `search_tools` because an earlier stream position still had one.

If registration depended only on the current stream position, promoting the last
discoverable tool would simultaneously withdraw `search_tools`, mutating the
`tools` array twice and removing a tool the model just called.
The trade-off of monotonicity is that a conversation whose discoverable tools
have all been promoted or disabled retains `search_tools`, which then returns
empty results.

**Search fields.** `search_tools` matches against tool names and parameter names
from the resolved schemas (`ToolDefinition.parameters`), plus summaries,
descriptions, and parameter descriptions from `ToolDocs`.
Reading parameter names from the schema rather than `ToolDocs.parameters`
ensures undocumented parameters (such as an undocumented `deployment_id`) are
still searchable.
Ranking uses case-insensitive substring matching across these fields; BM25 or
embedding-based ranking ([RFD 032]) can be added later behind the same
signature.

**Result limits and promotion scope.** Because each returned match triggers a
durable config change, the return contract is bounded:

- `limit` defaults to 10 and is clamped to `1-50`, preventing a single search
  from loading an unbounded number of tool schemas into context.
- Only returned tools are promoted.
  If a query matches 200 tools, `search_tools` returns `limit` matches and
  promotes only those.
- Tools already in the offered set are excluded from results so `search_tools`
  does not emit redundant promotion deltas.
- An empty query returns an error rather than listing every tool; loading all
  tools at once is the job of a bulk `-t`.

This composes with the existing disclosure tiers: `search_tools` locates the
tool, `describe_tools` returns its full documentation, and the model invokes the
tool.

### Promoting discovered tools

A `search_tools` call promotes its returned matches from `discoverable` to `on`
by emitting a `ConfigDelta` into the conversation stream.

Recording discovery as a `ConfigDelta` keeps the conversation stream as the
single source of truth for tool availability ([RFD 054]).
Because promoted tools use the standard config path, promotions survive
compaction, appear in `jp config` output, remain visible in the stream, and can
be reversed with `--no-tool`.

**Promoted tools are callable on the model's next reply.** [RFD 078] defines a
*cycle* as one LLM response plus the execution of its tool calls, with a turn
comprising one or more cycles.
A `ConfigDelta` emitted while executing tool calls commits at the end of the
cycle, and the turn loop re-resolves derived state (provider, model, **tool
definitions**, and tool choice) before entering `TurnPhase::Streaming` for the
next cycle.
The request that sends the `search_tools` result back to the model also carries
the newly promoted tool definitions, so discovery costs one tool call rather
than a full turn.

This between-cycle re-resolution is why this RFD extends [RFD 078].

### Promotion privileges

`search_tools` is a **privileged builtin**, in the same sense [RFD 094] uses for
`describe_tools`: it reads tool metadata unavailable to standard tools and
performs a single constrained config transition without a general
`access.config` grant on `conversation.tools.*`.

By construction, `search_tools` can only transition a tool from `discoverable`
to `on`.
It cannot enable an `off` tool, modify `allow_toggle`, or write to any other
config path.
Because `off` tools are excluded from the permitted set, they never appear in
search results and cannot be promoted; model-initiated discovery only exposes
tools the user already permitted.

Granting `search_tools` a general `access.config` permission with `apply =
"unattended"` on `conversation.tools` is discussed below and rejected on
least-privilege grounds.

#### Promotion channel from `execute_builtin`

In [RFD 078], if a builtin returns `Outcome::Success` with `config` or `unset`
populated, `execute_builtin` drops those fields and logs a warning before the
outcome reaches the coordinator, because builtins run without a `Context` or
`access.config` evaluation.
[RFD 078] listed builtin config mutations as future work; this RFD adds only the
narrow channel needed for tool promotion.

`execute_builtin` continues to drop and warn on `config` and `unset`.
Instead, `search_tools` returns the set of matched tool names on a dedicated
host-owned channel.
Because that channel carries only tool names (not config paths or arbitrary
values), the builtin cannot express an arbitrary config write, and the
coordinator constructs the `discoverable → on` `ConfigDelta` directly.

#### Conflicting writes in one cycle

Under [RFD 078], every tool in a cycle receives the config snapshot from the
start of the cycle, and the coordinator folds the commit buffer in tool-call
index order at the end of the cycle.
A single cycle can therefore contain conflicting writes:

1. A workflow tool at call index 0 proposes disabling `deploy`, and the user
   approves.
2. `search_tools("deploy")` at call index 1 matches `deploy`, which was still
   `discoverable` in the cycle-start snapshot.

Applying the promotion unconditionally at index 1 would overwrite the approved
disable from index 0.

The coordinator therefore evaluates each promotion against the folded config
state **at that call's position** in the commit buffer rather than the
cycle-start snapshot:

| Folded state at this position | Promotion          | Reported as        |
| ----------------------------- | ------------------ | ------------------ |
| `discoverable`                | applied            | usable             |
| `on`                          | no-op, none needed | usable             |
| `off`                         | skipped            | found, unavailable |

The `on` case occurs whenever a cycle contains multiple searches with
overlapping matches.
For example, if `search_tools("deploy")` and `search_tools("staging")` run in
the same cycle and both match a discoverable `deploy_staging` tool, the first
call promotes it to `on` and the second call sees it already `on`.
Treating `on` the same as `off` would report `deploy_staging` as unavailable in
the very request that includes its schema.

The `search_tools` response reports the post-fold outcome rather than the raw
match against the cycle-start snapshot.
Reporting a tool as usable when an earlier call in the cycle disabled it would
cause the model's next tool call to fail unexpectedly, whereas reporting an
already-promoted tool as unavailable would hide a tool the model can now call.

### Provider behaviour

Discovery itself is provider-agnostic.
The offered set determines which tools the model can see; how each provider
serializes that set on the wire takes one of two shapes:

**Without tool directives, the offered set is the provider's `tools` array.**
When a promotion lands, the `tools` array grows on the next request.
This saves context tokens up front at the cost of invalidating the prompt prefix
cache whenever a tool is promoted.
Every provider except directive-enabled Anthropic uses this path.

**With [RFD 105] tool directives, the `tools` array stays fixed.** [RFD 105]
sends every known tool in a stable array (deferred behind a single anchor tool)
and controls availability through `tool_addition` and `tool_removal` blocks so
the request prefix never changes.
Under [RFD 105], the offered set drives directive derivation instead of the
`tools` array: promoting a tool emits a `tool_addition` block while the `tools`
array stays byte-identical.
To build that fixed array, the provider reads [RFD 105]'s full tool list rather
than this RFD's offered set.

**Distinguishing [RFD 105]'s catalogue from the permitted set.** [RFD 105]'s
catalogue includes every tool regardless of enablement, including `off` tools,
so its fixed array covers any tool a later directive might enable.
This RFD's permitted set is narrower because `off` tools are neither searchable
nor callable.
[RFD 105]'s array is therefore a superset of the permitted set.

| Provider                                     | Fewer definitions in context | Cache preserved across a discovery  |
| -------------------------------------------- | ---------------------------- | ----------------------------------- |
| Anthropic with [RFD 105] tool directives | yes                          | yes, subject to catalogue stability |
| Anthropic without them                       | yes                          | no                                  |
| OpenAI, Google, OpenRouter, Cerebras         | yes                          | no                                  |
| Ollama, llama.cpp                            | yes                          | no                                  |

Cache preservation depends entirely on [RFD 105], and its stability guarantee
carries an important caveat for discoverable tools: [RFD 105] guarantees cache
stability only for local and builtin tool definitions, which are reproducible
from the conversation stream, and excludes MCP tools, which are fetched from the
server on each turn.
An MCP server that is offline, slow, or returns its tools in a different order
alters the catalogue array and invalidates the cache regardless of discovery.
Because large MCP catalogues are the primary use case for `discoverable`, cache
invalidation on MCP server changes remains the common case.

Local runtimes (Ollama and llama.cpp) have no cache-preserving mechanism for
mid-conversation tool additions.
JP sends an OpenAI-compatible `tools` array (`llamacpp.rs::convert_tools`),
which the server renders through the model's chat template ahead of the message
history.
Changing the offered set therefore invalidates the KV-cache prefix, and neither
runtime supports an additive directive like `tool_addition`.
Even so, local runtimes still benefit from the smaller context footprint in the
first column.

### Window accounting

`estimate_overhead_chars` counts every `ToolDefinition` passed to it.
Passing the offered set rather than the permitted set ensures undiscovered tools
add zero overhead to context window estimates.

## Drawbacks

**Discovery costs an extra tool call.** Finding a tool adds one round trip plus
the tokens of the `search_tools` call and its response.
Below roughly ten tools, that overhead exceeds the token cost of sending the
definitions up front.
The default remains `on` everywhere; `discoverable` is opt-in.

**Undiscovered tools go unused.** If the model never searches for a discoverable
tool, it answers without using the tool and gives no indication that a relevant
tool existed.
Listing searchable tool categories in the system prompt and keeping frequently
used tools `on` reduces this risk, but does not eliminate it.

**Widening `state` changes a core config type.** [RFD 081] split tool enablement
into `state` and `allow_toggle`, and existing callers of `Enable::is_enabled()`
assume a boolean.
Replacing that boolean with a three-valued enum touches every consumer of
`Enable`.

**The `state` alias carries permanent maintenance cost.** Supporting `state`
indefinitely requires maintaining the alias across three code paths: the
deserializer, `AssignKeyValue`, and write-time TOML normalization.
Widening `state` in place (keeping the field name and accepting `true` and
`false` on the new enum) would have required only one custom deserializer.
Renaming the field to `availability` makes its purpose clear alongside
`allow_toggle`, at the cost of maintaining three alias sites.

**A builtin mutates conversation config outside grant evaluation.**
`search_tools` is the first builtin that modifies tool enablement, and its
`discoverable → on` restriction is enforced in Rust code rather than through
[RFD 078]'s user-visible grant policy.
It also introduces a dedicated promotion channel out of `execute_builtin`
alongside the `config` and `unset` fields that [RFD 078] drops and warns on.

**MCP servers with discoverable tools still start eagerly.** Because JP resolves
the permitted set up front, a workspace with six MCP servers starts all six
processes and fetches every schema regardless of whether the model searches.
This RFD reduces prompt token usage, not startup latency.

**Substring search can miss relevant tools.** Case-insensitive substring
matching over names and descriptions misses tools when the model's query uses
different phrasing than the tool's documentation, resulting in a silent miss
rather than an explicit error.

## Alternatives

**Anthropic's server-side tool search.** Anthropic's
`tool_search_tool_regex_20251119` and its BM25 variant execute searches on
Anthropic's servers without requiring a local search implementation.
Rejected because they require persisting and replaying provider-specific
`server_tool_use` and `tool_search_tool_result` blocks in the conversation
stream, coupling JP's storage format to Anthropic's wire format, and they do not
work with any other provider.

**`tool_reference` blocks from a custom search tool.** The Anthropic API expands
`tool_reference` blocks returned inside a `tool_result`, allowing a custom
search tool to expose discovered tools without modifying the top-level `tools`
array.
Rejected as the primary mechanism because it is Anthropic-only, creates a second
source of truth for availability outside config, and relies on replayed message
history (so context window truncation or compaction dropping that turn would
silently unload the tool).
It remains a viable future optimization for rendering config-backed promotions
on Anthropic.

**Deriving the offered set by scanning the stream for `search_tools`
responses.** Avoids depending on [RFD 078], but rejected because it creates a
second source of truth for tool availability alongside config deltas.

**Promoting via a general `access.config` grant.** Configuring `search_tools`
with `path = "conversation.tools"`, `write = true`, and `apply = "unattended"`
would reuse [RFD 078]'s grant machinery directly.
Rejected on least-privilege grounds: that grant would allow the builtin to
modify any field under `conversation.tools` (including `allow_toggle` and
`command`), whereas a dedicated channel restricts it to `discoverable → on`.

**Adding a separate `discoverable: bool` field beside `state`.** Rejected
because `discoverable` has no meaning when `state = false`, making an invalid
state representable and pushing validation to runtime.

**Tool groups as the unit of discoverability.** [RFD 055] would allow marking an
entire tool group as discoverable at once.
Per-tool and `[conversation.tools.'*']` settings already cover MCP servers, and
group-level configuration will compose naturally once [RFD 055] lands.

## Non-Goals

- **Cache preservation.** [RFD 105] handles prompt cache stability across
  mid-conversation tool changes.
- **Ranking quality.** Substring matching is the initial implementation; BM25
  and embedding-based retrieval can be added later behind the same signature.
- **Automatic deferral policies.** JP does not automatically mark tools
  discoverable past a threshold count; users configure `discoverable`
  explicitly.
- **Withdrawing discovered tools.** Once promoted, a tool stays `on` for the
  rest of the conversation unless the user disables it.

## Risks and Open Questions

**Will the model choose to search?** Discovery only helps if the model calls
`search_tools` when its offered set lacks a needed capability, rather than
answering without it.
Users who mark a tool `discoverable` opt into this trade-off and can switch the
tool back to `on` with a single config change.
Even so, JP should track how often the model searches on turns where a
discoverable tool applies, and add a system prompt section listing searchable
categories if recall is low.

**Restart interaction.** Under [RFD 078], a `Restart` clears the uncommitted
cycle buffer while preserving deltas committed in earlier cycles of the same
turn.
If a promotion commits in one cycle and a later cycle in the same turn triggers
a restart, the promoted tools should remain `on` because the `search_tools`
result stays in the conversation stream.
A test should assert this behaviour explicitly.

**Search results and unapproved grants.** If a discoverable tool requires an
unapproved access grant ([RFD 076]) and appears in `search_tools` results, the
model may call it and trigger a permission prompt for a tool the user never
explicitly enabled.
Whether to rely on the permission prompt or filter unapproved tools out of
search results remains open.

**Permitted-set size and search cost.** `search_tools` scans the permitted set
in-process on every call, so search cost scales linearly with the permitted set.
This is negligible at current catalogue sizes, though worth monitoring as MCP
catalogues grow.

**Signalling truncated search results.** Clamping `limit` to 50 bounds how many
schemas a single call can promote, but a broad query against a 500-tool
catalogue receives a truncated list without knowing additional matches exist.
Returning the total match count alongside the results would expose the
truncation at minimal token cost; whether that improves model follow-up queries
is untested.

**Naming collision between `discoverable` and `explicit`.** Both names suggest a
tool that is available on demand, which may confuse users reading the config
reference.
If review shows the pair is too easy to conflate, one should be renamed before
`discoverable` ships, since `explicit` already has an existing user base.

## Implementation Plan

**Phase 1: Availability state and `state` alias.** Widen `Enable::state` into
`availability: Availability`, thread the third variant through
`PartialEnableConfig::effective`, `Enable::is_enabled`, and `--tool` /
`--no-tool` directive application, and implement all three `state` alias paths
(deserialization, `AssignKeyValue`, and write-time TOML normalization) in the
same change.
Shipping the read alias and write normalization together ensures existing
`enable = { state = false }` entries continue to deserialize and `jp config set`
never writes both keys into one table.
Tests cover round-tripping a stored `enable = { state = false }`, resolving
`--cfg ...enable.state=false`, and normalizing a legacy table on edit.
Because `search_tools` is not yet present, `discoverable` behaves as not-offered
and existing configs are unaffected.
Independently mergeable.

**Phase 2: Splitting the permitted and offered sets.** Separate permitted-set
resolution from provider tool exposure: `configure_active_mcp_servers` and
`tool_definitions()` build the permitted set from `discoverable` and `on` tools
plus any forced tool; the provider request receives only the offered subset; and
the `describe_tools` documentation map and executor lookup use the permitted
set.
Tests verify that a discoverable tool is omitted from the provider request while
remaining present in the `describe_tools` map, and that `jp q -u NAME` on an
`off` MCP tool still starts its server and builds an executor.
Depends on Phase 1.

**Phase 3: `search_tools` builtin.** Implement `search_tools`, substring
matching over permitted-set names, schemas, and `ToolDocs`, the `limit` clamp,
and stream-derived monotone registration.
In this phase `search_tools` returns matching tool summaries without promoting
them.
Tests verify that a conversation starting with no discoverable tools registers
`search_tools` after a `--cfg` delta introduces one, and retains `search_tools`
after all discoverable tools are promoted.
Depends on Phase 2.

**Phase 4: Tool promotion.** Add the host-owned promotion channel from
`execute_builtin`, coordinator generation of `discoverable → on` `ConfigDelta`
entries, and position-aware folding against earlier writes in the same cycle.
Tests verify that promoted tool definitions appear in the next provider request
of the same turn alongside the `search_tools` result, that a promotion folded
after an approved disable in the same cycle leaves the tool `off` and reports it
unavailable, and that two searches in the same cycle matching the same tool both
report it usable while emitting a single delta.
Depends on Phase 3 and [RFD 078].

**Phase 5: Measurement.** Add search-rate instrumentation and, if needed, a
system prompt section listing searchable tool categories.

## References

- [RFD 032]: semantic search, a potential ranking backend for `search_tools`
- [RFD 054]: config deltas positioned in the conversation stream
- [RFD 055]: tool groups as a future unit of discoverability
- [RFD 076]: access grants and their interaction with search results
- [RFD 078]: between-cycle re-resolution and the builtin config mutation
  boundary
- [RFD 081]: the `state` and `allow_toggle` enablement model widened here
- [RFD 094]: privileged builtins
- [RFD 105]: prompt cache preservation across mid-conversation tool changes
- [Tool search tool][anthropic-tool-search]: Anthropic's server-side search tool
  and the source of the 30-50 tool degradation figure
- [Advanced tool use][anthropic-advanced]: Anthropic's overview of tool
  catalogue scaling

[RFD 032]: 032-grizzly-semantic-search.md
[RFD 054]: 054-split-conversation-config-and-events.md
[RFD 055]: 055-tool-groups.md
[RFD 076]: 076-tool-access-grants.md
[RFD 078]: 078-tool-config-mutation.md
[RFD 081]: 081-decompose-tool-enable-into-state-and-allow_toggle.md
[RFD 094]: 094-built-in-tell_user-tool-for-mid-turn-user-addressed-messages.md
[RFD 105]: 105-mid-conversation-operator-directives.md
[anthropic-advanced]: https://www.anthropic.com/engineering/advanced-tool-use
[anthropic-tool-search]: https://platform.claude.com/docs/en/agents-and-tools/tool-use/tool-search-tool
