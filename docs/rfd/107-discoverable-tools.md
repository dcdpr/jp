# RFD 107: Discoverable Tools

- **Status**: Discussion
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-09-08
- **Extends**: [RFD 081], [RFD 078]

## Summary

Tool enablement grows a third state: a tool can be permitted, but not offered to
the model until the model asks for it.
A `search_tools` builtin searches the permitted set and promotes matches to
offered for the rest of the conversation.
This keeps large tool catalogues out of the context window without hiding
capability from the user.

## Motivation

JP sends every enabled tool's definition on every request.
That is correct while the catalogue is small, and it degrades in two ways as it
grows:

- **Context cost.** A multi-server MCP setup spends tens of thousands of tokens
  on definitions before the model does any work.
- **Selection accuracy.** Anthropic reports that model tool selection degrades
  past roughly 30–50 available tools ([tool search][anthropic-tool-search],
  [Advanced tool use][anthropic-advanced]).
  That figure is theirs and unverified against JP's catalogue, but the direction
  is not in dispute, and JP's catalogue grows every time someone adds an MCP
  server.

JP already has one tier of progressive disclosure:
`ToolDocs::schema_description` puts `summary` in the tool schema, and the
`describe_tools` builtin returns the full docs on request.
This RFD adds the tier below it — the tool is not in the schema at all until
the model finds it.

Doing nothing means the ceiling on JP's tool catalogue is set by the context
window rather than by what is useful.

## Design

### What the user sees

A third value for a tool's enablement:

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

`enable`'s string shorthands are presets over both of its fields, not values of
one: `"always"` is locked-on and `"explicit"` is off-unless-named.
`"discoverable"` joins them, expanding to both fields:

```toml
enable = "discoverable"
# is exactly
enable = { availability = "discoverable", allow_toggle = "any" }
```

The availability-only table form is **not** the same thing.
An omitted table field stays unset and inherits from `[conversation.tools.'*']`
([RFD 081]), so `enable = { availability = "discoverable" }` keeps whatever
toggle policy the defaults layer supplies, while the shorthand overwrites it
with `any`.
Where a defaults layer sets `allow_toggle = "if_named"`, the two forms disagree
about whether a bulk `-t` may promote the tool.

A sensible default for a large MCP server is to mark the whole server
discoverable in `[conversation.tools.'*']` and pull two or three tools up to
`on` — with the same caveat, since a shorthand in the defaults layer sets both
fields there too.

**`explicit` and `discoverable` are different, and the words are close enough to
confuse.** Both mean "you have to ask for this first"; they differ in who asks.
`explicit` withholds a tool until the *user* names it on the command line.
`discoverable` withholds it until the *model* finds it by searching.
They compose: `enable = { availability = "discoverable", allow_toggle =
"if_named" }` is a tool the model can find and a bulk `-t` cannot sweep up.

### CLI directives

`allow_toggle` already decides which `--tool` / `--no-tool` directives may move
a tool, at bulk, named, and named-group scope.
That axis is unchanged and this RFD adds nothing to it.
What needs stating is only what a permitted directive *means* against three
values:

| Directive          | `off`  | `discoverable` | `on`    |
| ------------------ | ------ | -------------- | ------- |
| `-t` / `--tool`    | → `on` | → `on`         | no-op   |
| `-T` / `--no-tool` | no-op  | → `off`        | → `off` |

A bulk `-t` therefore loads every discoverable tool into context, for every tool
whose `allow_toggle` permits it.
That is the correct default: `-t` means "give the model everything," and a user
who wants a discoverable tool exempt from it has `allow_toggle = "if_named"`
already.

**Forced tool use is a separate axis and stays as it is.** `jp q -u NAME` binds
a single turn: it is read from the flags at query time rather than layered into
the conversation's config, and `tool_definitions` already keeps a forced tool in
the definition set even when it is disabled, unless it is locked off.
Forcing a discoverable tool therefore offers it for that invocation, emits no
promotion delta, and leaves its stored availability untouched.
A persisted `assistant.tool_choice` naming a function keeps its existing
exemption, on every turn rather than one; this RFD does not change that
precedence.

The consequence matters for the next section: because forcing can put an `off`
tool into the resolved definition set, that set is not a statement about
availability and must not be used as the search surface.

### The state

Two questions decide how a tool is presented:

1. May the model call it?
2. Is its definition in context up front, or only after discovery?

Question 2 has no meaning when the answer to question 1 is no, so three of the
four combinations are valid.
Encoding them as a three-valued enum rather than two booleans makes the invalid
combination unrepresentable:

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

This widens `Enable::state`, which is a `bool` today ([RFD 081]).
`allow_toggle`, its sibling field, is untouched.

### Two sets, not one

Today a single `Vec<ToolDefinition>` does four jobs: it is the provider's tools
array, the docs map handed to `describe_tools`, the lookup the executor consults
to decide whether a call can run, and — through the same `is_enabled()`
predicate — the reason an MCP server is started at all.
One set cannot serve a discoverable tool, which has to be searchable,
describable and callable-once-promoted while staying out of the provider's tools
array.

The set splits in two, with the second a strict subset of the first:

- The **permitted set** is every tool the model may call on this invocation:
  availability `discoverable` or `on`, plus any tool named by the forced-tool
  exemption, minus anything locked off.
  Every member is fully resolved, including the MCP round-trip that fetches a
  server-side tool's schema and description.
- The **offered set** is the subset whose definition goes to the provider:
  availability `on`, plus the forced tool.

The forced-tool exemption has to widen the permitted set rather than only the
offered set.
`jp q -u NAME` on an `off` tool is a supported operation, and an executor is
built by looking the tool's definition up in the resolved set; a tool offered to
the model but absent from that lookup is one the model can call and JP cannot
run.
The same applies one layer down: `configure_active_mcp_servers` already starts a
forced tool's backing server regardless of its enable state, because a server
that never starts leaves the tool unresolvable and the forced choice
unsatisfiable.

Consumers divide along those two sets:

| Consumer               | Set                                        | Why                                                  |
| ---------------------- | ------------------------------------------ | ---------------------------------------------------- |
| Provider `tools` array | offered                                    | the context cost this RFD exists to avoid            |
| `search_tools`         | availability `discoverable`, minus offered | an `off` tool never appears in search, forced or not |
| `describe_tools`       | permitted                                  | a found tool must be explainable before it is called |
| Executor eligibility   | permitted                                  | a promoted tool is callable in the same turn         |
| MCP server startup     | permitted                                  | an unstarted server has no metadata to search        |

The search row names an availability rather than a subtraction because it says
what the set *is*, and because it stays correct if the permitted set later grows
a member that is neither offered nor discoverable.

**Discovery saves context, not startup.** A discoverable MCP tool's server
starts with the conversation and its schema is fetched up front, exactly as an
enabled tool's is today.
Eager resolution is what makes the permitted set searchable, and it means the
saving is measured in prompt tokens, not in latency or process count.
A cheaper lazy scheme — resolving a server only once one of its tools is
searched for — is possible later and is not required here.

A tool whose backing server is not running is dropped from the permitted set,
the same treatment `tool_definitions` gives it today.
It does not appear in search results as an unavailable entry: a result the model
cannot act on costs tokens and invites a call that cannot succeed.

### Renaming `state`

`state` was an adequate name for a boolean and is a poor one for a three-valued
enum — the state of *what* is not in the name, and it now sits next to a field
called `allow_toggle` that governs it.
The field becomes `availability`, and `state` stays as a deserialization alias
mapping `true`/`false` onto `on`/`off`.

The alias is permanent, not a migration window: `enable = { state = false }`
appears in stored conversation configs and in `ConfigDelta` entries on existing
streams, and those are read, not rewritten.

The alias has to be written three times, because three separate paths reach the
field:

- **Deserialization.** The hand-written `Deserialize` on `PartialEnableConfig`
  accepts `state` wherever it accepts `availability`, mapping `true` to `on` and
  `false` to `off`.
- **Key assignment.** `--cfg conversation.tools.foo.enable.state=false` goes
  through `AssignKeyValue`, which matches key strings directly and never sees
  the deserializer.
  Its `state` arm keeps working and gains an `availability` sibling.
- **File editing.** See below.

#### Editing a file that still says `state`

A read alias alone leaves `jp config set` producing a file with both spellings.
`config set` serializes the delta and merges it into the existing document, and
that merge only touches keys the delta contains: a nested key set against an
inline table deep-merges the subfield instead of replacing the table.
Serialization always emits the canonical name.
So setting `availability` on a tool whose file says `enable = { state = false }`
produces:

```toml
enable = { state = false, availability = "discoverable" }
```

Two keys for one field, disagreeing.

**A write that touches this setting normalizes the legacy key it touched.** When
the edit targets a tool's `enable`, a sibling `state` in that same table is
removed as the canonical name is written.
The rule is deliberately narrow: it rewrites the one table the user is already
editing, and leaves every other `state` in the file, in other files, and in
stored conversation data exactly where it is.
A config file JP never writes to never changes.

**Both keys in one table is an error, not a precedence rule.** Normalization
means JP cannot produce that shape, so it only arises from hand-editing.
Giving one spelling precedence would silently discard the other, and the
direction that gets discarded matters here: ignoring a `state = false` next to
an `availability = "on"` hands the model a tool the user believed was off.
The error names both keys and says which to keep.

### `search_tools`

A builtin tool:

```
search_tools(query: string, limit?: integer = 10) -> [{ name, summary }]
```

**Registration is monotone within a conversation.** `search_tools` is offered
once any config position in the conversation's stream has held at least one tool
at `discoverable`, and stays offered from there on.
Like availability itself, that is derived from the stream ([RFD 054]) rather
than latched in memory, so it survives the separate CLI invocations a
conversation is made of.

Both directions follow from the same rule.
A user who adds `--cfg conversation.tools.deploy.enable=discoverable` to a
running conversation gets the builtin on the next turn, because the delta puts a
discoverable tool at a stream position.
A conversation whose discoverable tools have all been promoted keeps it, because
an earlier position still holds one.

Evaluating the current position instead would withdraw the builtin the moment
the last discoverable tool is promoted — a second change to the tools array on
top of the promotion, and a tool disappearing under the model for no reason it
can see.
The cost of monotonicity is the mirror case: a conversation that once had
discoverable tools and no longer does keeps a builtin whose searches come back
empty.
That is a wasted definition, not a wrong answer.

**What it searches.** Tool names and parameter names come from the resolved
schemas (`ToolDefinition.parameters`); summaries, descriptions and parameter
descriptions come from `ToolDocs`.
The split matters: `ToolDocs.parameters` holds only parameters that carry
documentation, so building parameter-name search from it would silently miss
`deployment_id` on a tool that never documented it.
Ranking starts as case-insensitive substring matching over those fields.
BM25 or embedding-backed ranking ([RFD 032]) is a later refinement behind the
same signature.

**What it returns and what that commits.** The contract is narrow because each
result is a durable config change, not just output:

- `limit` defaults to 10 and is clamped to 1–50.
  A search cannot load an unbounded slice of the permitted set into context on
  one call.
- Only the tools actually returned are promoted.
  A query matching 200 tools returns `limit` of them and promotes exactly those.
- Already-offered tools are excluded from results.
  They are callable already, and including them would emit promotion deltas that
  change nothing.
- An empty query is an error, not "list everything".
  Loading everything at once is what a bulk `-t` is for.

The result composes with the existing tiers: `search_tools` finds the tool,
`describe_tools` explains it, the tool call runs it.

### How a discovery takes effect

A `search_tools` call promotes its matches from `discoverable` to `on` by
emitting a `ConfigDelta`.
The promotion is scoped to the conversation and durable in its stream.

This is the decisive choice in the design, and it is a choice about sources of
truth rather than about mechanism.
Availability is already derived from config at a stream position ([RFD 054]);
routing discovery through the same path means there is one answer to "which
tools are offered here", not two that must be reconciled.
Discovery then inherits everything config already does: it survives compaction,
appears in `jp config` output, is visible in the stream, and is undone by an
ordinary `--no-tool`.

**The promoted tool is callable in the model's very next reply.** [RFD 078]
defines a *cycle* as one LLM response plus the execution of its tool calls, with
a turn containing one or more cycles.
A delta produced during a cycle's executing phase commits at cycle end, and the
outer turn loop then re-resolves the derived values — provider, model, **tool
definitions**, tool choice — before re-entering at `TurnPhase::Streaming`.
So the request that carries the `search_tools` result also carries the promoted
definitions, and the model can call them immediately.
Discovery costs one tool call, not one turn.

This is the reason the RFD extends [RFD 078]: not for its grant machinery, but
for the between-cycle re-resolve that makes mid-turn tool changes take effect at
all.

### Who is allowed to promote

`search_tools` is a **privileged builtin**, in the sense [RFD 094] uses for
`describe_tools`: it reads tool metadata no other tool can see, and it writes
one specific transition.
It does not take a general `access.config` grant on `conversation.tools.*`.

The write it performs is restricted by construction to `discoverable → on`.
It cannot enable an `off` tool, cannot change `allow_toggle`, and cannot touch
any other config path.
A tool the user turned off is not in the permitted set, does not appear in
search results, and cannot be promoted — model-initiated discovery never widens
what the user permitted, only what the model can currently see.

The alternative, granting the builtin `access.config` with `apply =
"unattended"` on `conversation.tools`, is discussed below and rejected on
least-privilege grounds.

#### Crossing the boundary [RFD 078] closed

[RFD 078] deliberately shuts this door: if a builtin returns `Outcome::Success`
carrying `config` or `unset`, `execute_builtin` drops those fields and logs a
warning before the outcome reaches the coordinator, because the builtin
execution path has no `Context` and no `access.config` plumbing.
Adding config access to builtins is the follow-up 078 names in its Non-Goals.
This is that follow-up, and it opens the door by exactly one inch rather than
removing it.

That rejection stays in force for `config` and `unset`.
Promotion does not travel through them.
It leaves `execute_builtin` on a separate host-owned channel whose payload is a
set of tool names and nothing else — no config paths, no values, no way to
express a write the coordinator would have to authorize.
The coordinator turns that set into the `discoverable → on` delta itself.
A builtin that populates `config` still gets 078's drop-and-warn; the shape of
what `search_tools` returns makes the general case unreachable rather than
merely disallowed.

#### Conflicting writes in one cycle

[RFD 078] hands every tool in a cycle the config as of cycle start and folds the
commit buffer by tool-call index at cycle end.
So this is possible:

1. A workflow tool at call index 0 proposes disabling `deploy`; the user
   approves.
2. `search_tools("deploy")` at index 1 matches `deploy`, which was still
   `discoverable` in the cycle-start snapshot it was given.

Promoting unconditionally would fold the promotion over the approved disable and
silently undo it.

A promotion is therefore evaluated against the folded state **at its own
position** in the buffer, not against the cycle-start snapshot.
The folded state answers three ways, and the distinction between the last two is
the whole point:

| Folded state at this position | Promotion          | Reported as        |
| ----------------------------- | ------------------ | ------------------ |
| `discoverable`                | applied            | usable             |
| `on`                          | no-op, none needed | usable             |
| `off`                         | skipped            | found, unavailable |

The `on` row is not hypothetical.
[RFD 078] folds by tool-call index because a cycle can carry several calls, and
two searches with overlapping matches are ordinary model behaviour:
`search_tools("deploy")` and `search_tools("staging")` both match a discoverable
`deploy_staging`, and the second sees a tool the first already promoted.
Collapsing `on` and `off` into one "not `discoverable`" case would tell the
model a tool is unavailable in the same request that offers its definition.

The search response reports what committed rather than what was matched.
A response that promised a tool the fold left `off` would put the model in a
state where its next call fails for reasons the transcript does not explain, and
a response that withheld a tool the fold left `on` would waste a capability the
model just paid a call to find.

### Provider behaviour

Nothing in the mechanism is provider-specific.
The offered set decides what the model can see; how that reaches the wire is the
provider's business, and there are two shapes.

**Without tool directives, the offered set *is* the tools array**, and the array
grows when a promotion lands.
That buys the token saving and loses the prefix cache, which is the trade every
provider except directive-enabled Anthropic makes.

**With [RFD 105], the array is not the offered set at all.** [RFD 105] sends
every tool it knows about in a fixed array, deferred behind one anchor tool, and
expresses availability through `tool_addition` and `tool_removal` blocks so the
prefix never changes.
Under that model the offered set feeds directive derivation instead of the
array: a promotion becomes a `tool_addition`, and the array stays
byte-identical.
The provider needs the full stable definitions to do this, so it reads [RFD
105]'s array rather than this RFD's offered set.

**Two documents, two meanings of "catalogue".** [RFD 105]'s catalogue is every
tool regardless of enablement, including `off` ones, because its array has to
cover any tool a later directive might name.
This RFD's permitted set is narrower by construction, since an `off` tool is
never searchable or callable.
The two compose — 105's array is a superset of this RFD's permitted set — but
they are not the same set, which is why this RFD does not reuse the word.

| Provider                                     | Fewer definitions in context | Cache preserved across a discovery  |
| -------------------------------------------- | ---------------------------- | ----------------------------------- |
| Anthropic with [RFD 105] tool directives | yes                          | yes, subject to catalogue stability |
| Anthropic without them                       | yes                          | no                                  |
| OpenAI, Google, OpenRouter, Cerebras         | yes                          | no                                  |
| Ollama, llama.cpp                            | yes                          | no                                  |

Cache preservation is the only row that varies, and it varies on a mechanism
[RFD 105] already owns.
That first row carries a qualification worth reading, because it lands on the
case this RFD exists for: [RFD 105] scopes its guarantee to local and builtin
tool definitions, which are reproducible from the stream, and excludes MCP
tools, which are fetched from the server each turn.
A server that is offline, slow, or returns its tools in a different order
perturbs the catalogue array and busts the cache regardless of what discovery
does.
Since the motivating case for marking tools discoverable is a large MCP
catalogue, that exception is the common case rather than the corner.

For local runtimes JP has no lever at all.
It sends an OpenAI-compatible `tools` array (`llamacpp.rs::convert_tools`) and
the server renders it through the model's chat template; those templates place
tool definitions ahead of the messages, so a change to the offered set is
expected to invalidate the KV-cache prefix, and neither runtime has an additive
equivalent of `tool_addition`.
The templating claim is inferred from how current chat templates are written
rather than measured, and it changes nothing in this design either way — the
token saving in column one is the benefit local runtimes get.

### Window accounting

`estimate_overhead_chars` counts every `ToolDefinition` it is handed.
It keeps doing exactly that; the caller hands it the offered set rather than the
permitted set.
Discoverable-but-undiscovered tools cost nothing because they are not sent.

## Drawbacks

**Discovery costs a tool call.** One extra round trip, plus the tokens of the
search call and its result.
Below roughly ten tools that costs more than the definitions it avoids.
The default stays `on` everywhere; `discoverable` is opt-in per tool.

**The model cannot use what it cannot see.** A tool nobody searches for is a
tool that never runs, and the failure is silent — the model answers without it
rather than reporting that it looked.
Mitigations are a system prompt section naming the searchable categories, and
keeping frequently-used tools `on`.
Neither is a guarantee.

**A widened `state` touches settled ground.** [RFD 081] decomposed enablement
into `state` and `allow_toggle`, and every consumer of `Enable::is_enabled()`
assumes a boolean.
This re-opens that model.

**The rename is not free, and its cost is permanent.** Keeping `state` readable
forever means the alias lives in three places — the deserializer, the key
assignment arm, and a normalization rule in the file writer — and every one of
them is a place a future change can forget.
Widening `state` in place, keeping the name and accepting `true`/`false` as
values of the new enum, would have cost one site instead of three.
The rename buys a field name that says what it holds, standing next to an
`allow_toggle` that governs it; that is the whole return, and it is a judgement
call rather than a forced one.

**A builtin gains a config write.** `search_tools` is the first builtin that
mutates enablement, and the transition it is allowed to make is enforced in code
rather than by [RFD 078]'s grant evaluation.
That is the least-privilege choice, and the cost is that the restriction lives
in one function instead of in the policy the user can read.
It also puts a second write channel out of `execute_builtin` next to the one 078
closed, and the two have to stay distinguishable for 078's drop-and-warn to keep
meaning what it says.

**Discoverable MCP servers still start eagerly.** The permitted set is resolved
up front, so a workspace with six MCP servers boots six servers and fetches
every schema whether or not the model searches.
The saving is prompt tokens only.
A workspace whose cost is startup latency rather than context gets nothing from
this RFD.

**Search quality is now JP's problem.** Substring matching over summaries will
miss tools whose description does not use the user's vocabulary, and the symptom
is an absent capability rather than an error.

## Alternatives

**Anthropic's server-side tool search.** `tool_search_tool_regex_20251119` and
its BM25 sibling run the search on Anthropic's servers and need no search
implementation from JP.
Rejected: it requires persisting and replaying `server_tool_use` and
`tool_search_tool_result` blocks verbatim in the durable conversation store,
which commits JP's on-disk format to a provider's internal block shape, and it
is unavailable to every other provider.

**`tool_reference` blocks from a custom search tool.** The Anthropic API expands
`tool_reference` blocks returned in a `tool_result`, which would let a discovery
change the offered set without touching the tools array.
Rejected as the primary mechanism: it is Anthropic-only, it introduces a second
source of truth for availability alongside config, and the reference lives in
replayed history — so window truncation or compaction dropping that turn
silently unloads the tool.
It remains available later as an Anthropic-specific *rendering* of a discovery
that is still recorded as a delta.

**Derive the offered set by scanning the stream for `search_tools` responses.**
Avoids depending on [RFD 078].
Rejected for the same reason: two sources of truth for one question.

**Promote through a general `access.config` grant.** Ship `search_tools` with
`path = "conversation.tools"`, `write = true`, `apply = "unattended"` and let
[RFD 078]'s existing machinery carry it.
Rejected on least privilege: that grant lets the builtin write any field of any
tool's config, including `allow_toggle` and `command`, when the one operation it
needs is `discoverable → on`.
An unattended write grant over the tool tree is a large surface to hand a
builtin to save a match arm.

**A separate `discoverable: bool` beside `state`.** Rejected: it is meaningless
when the tool is off, so it makes an invalid combination representable and
pushes the validation into runtime.

**Tool groups as the unit of discoverability.** [RFD 055] would let a whole
group be marked discoverable in one line.
Not rejected, just not required — per-tool and per-defaults settings cover the
MCP-server case, and group support falls out for free once groups exist.

## Non-Goals

- **Cache preservation.** [RFD 105] owns that, through a mechanism this RFD does
  not depend on and does not duplicate.
- **Ranking quality.** Substring matching is the starting point.
  BM25 and embedding-backed retrieval are refinements behind an unchanged
  signature.
- **Automatic deferral policies.** No "mark everything discoverable past N
  tools".
  Users decide.
- **Withdrawing a tool the model discovered.** Once promoted, a tool stays
  offered for the conversation unless the user disables it.

## Risks and Open Questions

**Will the model choose to search?** Discovery only pays off if the model calls
`search_tools` when its offered set is insufficient, instead of answering
without the capability.
This is a tuning concern rather than a design gate — a user who marks a tool
discoverable has opted into exactly this trade, and moving it back to `on` is
one edit.
It is still worth instrumenting: count searches against turns where a
discoverable tool would have helped, and add a prompt section naming the
searchable categories if the rate is poor.

**Restart interaction.** [RFD 078]'s commit buffer is cleared on Restart, and
deltas committed in prior cycles of the same turn survive.
A promotion that lands and is then followed by a restart within the same turn
needs a decided answer: the discovered tools presumably stay promoted, since the
search result they came from is still in the stream, but this should be asserted
in a test rather than assumed.

**Search results and unapproved grants.** A discoverable tool carrying an
unapproved access grant ([RFD 076]) surfaces in search results, so the model
calls it and the user gets a permission prompt for a tool they never explicitly
enabled.
That may be fine — the prompt is the control point, and the user did mark the
tool discoverable — or search results should be filtered by grant status.
Unresolved.

**Permitted-set size and search cost.** `search_tools` runs in-process over the
permitted set, so cost scales with its size on every call.
At JP's current scale this is irrelevant; it is worth a note only because the
feature's premise is that catalogues grow.

**Is the `limit` clamp the right shape?** Capping a search at 50 results bounds
how much context one call can commit, but a model searching a 500-tool catalogue
with a broad query gets a truncated view and no signal that it was truncated.
Returning a match count alongside the results would fix that and costs a few
tokens; whether it changes model behaviour for the better is untested.

**Is `discoverable` the right word?** It sits next to `explicit`, which means a
nearly identical thing to a user skimming the config reference.
If the pair reads as confusing in review, the fix is renaming one of them, and
`explicit` is the one with an existing user base.

## Implementation Plan

**Phase 1 — The state and its alias.** Widen `Enable::state` into
`availability: Availability`, thread the third value through
`PartialEnableConfig::effective`, `Enable::is_enabled`, and the `--tool` /
`--no-tool` directive application, and land all three alias sites in the same
change.
The read alias is not separable: without it every stored `enable = { state =
false }` stops deserializing the moment the field is renamed.
The write-side normalization ships here too, so no version of JP can produce a
table carrying both spellings.
Three tests carry the phase: a stored `enable = { state = false }` round-trips,
`--cfg ...enable.state=false` still resolves, and editing a legacy file leaves
exactly one spelling behind.
No `search_tools` yet, so `discoverable` resolves as not-offered and nothing
changes for existing configs.
Independently mergeable.

**Phase 2 — Splitting the sets.** Separate permitted-set construction from
provider exposure: `configure_active_mcp_servers` and `tool_definitions()` build
the permitted set from `discoverable | on` plus the forced tool, the provider
request takes the offered subset, and the `describe_tools` docs map and the
executor's definition lookup take the permitted set.
Still no `search_tools`, so a discoverable tool is resolvable and unreachable.
Two tests carry the phase: a discoverable tool's definition is absent from the
request while its docs are present in the map, and `jp q -u NAME` on an `off`
MCP tool still starts its server and builds an executor.
Depends on phase 1.

**Phase 3 — `search_tools`.** The builtin, its search over permitted-set names
and schemas, the `limit` contract, and its stream-derived monotone registration.
Returns matches as text; promotion is not wired up, so calling it is
informational.
Two tests carry the registration rule: a conversation that starts with no
discoverable tool gains the builtin after a `--cfg` delta introduces one, and a
conversation keeps it after every discoverable tool has been promoted.
Depends on phase 2.

**Phase 4 — Promotion.** The host-owned promotion channel out of
`execute_builtin`, the coordinator turning it into a `ConfigDelta` restricted to
`discoverable → on`, and position-aware folding against a conflicting write in
the same cycle.
Three tests carry the phase: after a `search_tools` call the promoted
definitions appear in the very next request of the same turn alongside the
search result; a promotion folded after an approved disable of the same tool
leaves it `off` and says so in the search response; and two searches in one
cycle matching the same tool report it usable both times, with one delta between
them.
Depends on phase 3 and on [RFD 078] being implemented.

**Phase 5 — Measurement.** Search-rate instrumentation against the first open
question, and a prompt section naming searchable categories if the rate warrants
one.

## References

- [RFD 032] — semantic search, a later ranking backend
- [RFD 054] — config deltas positioned in the conversation stream
- [RFD 055] — tool groups as a possible unit of discoverability
- [RFD 076] — access grants, and their interaction with search results
- [RFD 078] — the between-cycle re-resolve that makes promotion take effect,
  and the builtin mutation boundary this RFD opens by one inch
- [RFD 081] — the enablement model this RFD widens
- [RFD 094] — privileged builtins
- [RFD 105] — cache preservation for mid-conversation tool changes
- [Tool search tool][anthropic-tool-search] — the server-side alternative, and
  the source of the 30–50 tool figure
- [Advanced tool use][anthropic-advanced] — Anthropic's write-up of the scaling
  problem

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
