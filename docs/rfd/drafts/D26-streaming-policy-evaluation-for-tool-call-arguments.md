# RFD D26: Streaming Policy Evaluation for Tool Call Arguments

- **Status**: Draft
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-07-24
- **Extends**: [RFD D25]
- **Requires**: [RFD 043]
- **Summary**: Evaluates argument-conditional tool policies incrementally as
  parameters stream so permission prompts and cancellations happen before full
  payload arrival.

## Summary

This RFD extends [RFD D25]'s argument-conditional tool policies to evaluate
rules incrementally as individual parameters finish streaming ([RFD 043]).
JP can prompt for permission or cancel the stream before the full argument
payload arrives.

## Motivation

[RFD D25] introduces conditional policy rules that match on argument values, but
evaluates them only after all arguments arrive:

```txt
ToolCallPart::Start { id, name }     ← tool name known
ArgumentChunk("{"path":"src/...")    ← arguments streaming
ArgumentChunk("...content":"fn...")  ← still streaming (possibly seconds)
ArgumentChunk("...main() {}")        ← still streaming
Flush                                ← arguments parsed, policy evaluated NOW
```

For `fs_create_file` with a rule `{ arg = "/path", prefix = "src/", mode =
"unattended" }`, the decision depends only on `path`.
The `path` value finishes streaming in the first chunk, yet JP waits for
`content` (potentially thousands of lines) before acting on it.

Waiting has three costs:

- **Wasted streaming time.** If the policy resolves to `ask` and the user
  declines, every token spent streaming `content` is wasted.
  For large files, that is seconds of billed LLM output that is never used.
- **Delayed approval.** If the policy resolves to `unattended`, execution could
  begin as soon as `path` is known instead of waiting for the full argument
  object.
- **Delayed prompt.** If the policy resolves to `ask`, the user could review and
  decide while `content` streams in the background, instead of waiting for
  streaming to finish before deciding and then waiting again for execution.

## Design

### Prerequisites

- **[RFD D25]** defines the conditional policy types (`RunPolicy`, `RunRule`,
  `ParamCondition`, `TypedMatcher`), the `policy` config namespace, and the
  first-match-wins evaluation model.
  This RFD extends its post-flush evaluation to run on streaming arguments.
- **[RFD 043]** introduces incremental argument parsing via
  `ToolCallArgumentProgress` events.
  Each event carries a `StreamFragment`, where `ObjectEntry { key, value: Done
  }` signals that a parameter value is complete at any nesting level.
  This RFD consumes those signals to drive policy evaluation.

### Evaluation algorithm

The core evaluator is a pure function in `jp_cli::cmd::query::tool::policy`.
It imports the rule types from `jp_config::conversation::tool::policy` ([RFD
D25]) and matches against `serde_json::Value` parameter values: `jp_config`
defines the types and `jp_cli` evaluates them at runtime.

Given the policy's rule list and the map of parameter values received so far,
`evaluate` returns `Option<RunMode>`: `Some` if a rule matched, or `None` if the
first non-eliminated rule needs a parameter that has not arrived yet.

```rust
fn evaluate(rules, known_params) -> Option<RunMode>:
    for rule in rules:
        if rule has no condition:
            return Some(rule.mode)      // catch-all
        if rule.arg is in known_params:
            if matcher matches value:
                return Some(rule.mode)  // match found
            else:
                continue                // rule eliminated, try next
        else:
            return None                 // can't skip — higher priority

    // all conditional rules eliminated, no catch-all
    return Some(RunMode::Ask)           // implicit safety fallback
```

**A rule whose parameter has not arrived yet blocks evaluation.** Under
first-match-wins semantics, that rule has higher priority than every rule below
it, so skipping ahead to a lower-priority rule could return the wrong mode.

Policies without conditional rules (string aliases like `run = "ask"`, or a
single catch-all rule) resolve on the first call with an empty `known_params`
map because the catch-all matches immediately.
These policies take the existing permission fast path in
`ToolCoordinator::decide_permission` without creating a `StreamingPolicyState`.

### Integration with `ToolCallArgumentProgress`

In [RFD 043], `EventBuilder::handle_part` returns
`Vec<ToolCallArgumentProgress>` for `ArgumentChunk` events, and the turn loop
forwards each event's `StreamFragment` to `ToolRenderer` for incremental
display.
This RFD adds the policy evaluator as a second consumer.

A `StreamingPolicyState` struct, held per in-flight tool call, tracks completed
parameters and feeds them to the evaluator:

```rust
struct StreamingPolicyState {
    /// The policy rules for this tool (from config).
    rules: Vec<RunRule>,
    /// Parameter values received so far, keyed by parameter name.
    known_params: HashMap<String, Value>,
    /// Resolved mode, if any. `None` while waiting for parameters.
    resolved: Option<RunMode>,
}
```

When a `ToolCallArgumentProgress` fragment signals that a parameter is complete
(see [Nested argument paths](#nested-argument-paths) for nested completion
detection), the turn loop:

1. Reads the complete `serde_json::Value` for that parameter from the
   `FragmentAggregator` ([RFD 043]).
2. Inserts the value into `known_params`.
3. Calls the evaluator with the updated `known_params`.
4. Triggers the permission flow immediately if the result is `Decided`.

### Turn loop integration

Without streaming evaluation, `turn_loop.rs` decides permission at the Flush
boundary after `EventBuilder` emits the final `ToolCallRequest`.
The streaming evaluator runs earlier in the `StreamingLoopEvent::Llm` match arm,
alongside `ToolCallArgumentProgress` forwarding to the renderer.

Each tool call follows this flow:

```txt
ToolCallPart::Start { id, name }
  → Look up RunPolicy from tool config
  → If no conditional rules: use existing fast path (no StreamingPolicyState)
  → Otherwise: create StreamingPolicyState, resolved = None

ArgumentChunk → ToolCallArgumentProgress events
  → Forward to ToolRenderer (existing)
  → On parameter completion: feed to StreamingPolicyState
    → If Some: trigger early permission
    → If None: continue accumulating

Flush → ToolCallRequest with complete arguments
  → If already resolved: skip evaluation
  → If still None: all parameters are now known, evaluate once more
```

The Flush step handles providers that do not stream arguments incrementally (for
example, Ollama emitting the entire argument object in one chunk).
All `ToolCallArgumentProgress` events then arrive in the same batch as Flush,
and the same evaluator runs once with all parameters available.

### Early permission prompt with partial arguments

When the evaluator resolves to `ask` before all arguments arrive, the turn loop
prompts for permission using completed parameters and placeholders for
parameters still streaming.

For the `function_call` display style:

```
Calling tool fs_create_file(path: "src/main.rs", streaming "content"...)
Allow? [y/n/e]
```

For the `json` display style:

```json
{
  "path": "src/main.rs",
  "#": "streaming argument \"content\"..."
}
```

Because the parameter that matched the policy rule (such as `path`) is already
visible, the user can decide immediately.
Approving lets streaming continue and starts execution once all arguments
arrive; declining cancels the tool call immediately.

[RFD 043] defines the progress event protocol but defers renderer changes, so
Phase 4 extends `ToolRenderer` to render partial arguments during the permission
prompt.

### Cancellation on rejection

When the user declines a tool call during streaming, JP:

1. Marks the tool call as `Completed` with a "skipped by user" response.
2. Discards remaining `ArgumentChunk` events for that tool call index.
3. **If this is the only pending tool call**, cancels the LLM response stream.
   Because tool calls are the terminal content of an LLM response, no message
   text follows; cancelling avoids generating and paying for discarded argument
   tokens (such as a thousand-line `fs_create_file` payload).
4. **If other tool calls are still in flight**, keeps the stream open so the
   remaining tool calls can complete.

Stream cancellation reuses the `CancellationToken` mechanism that handles
mid-stream `Ctrl+C` interrupts in the turn loop.

### Nested argument paths

[RFD D25] supports `arg` pointers at any depth, such as `/path` (top-level),
`/patterns/paths` (nested array), and `/patterns/old` (nested string).
Streaming evaluation works at all depths by consuming [RFD 043]'s nested `Done`
signals.

#### Top-level parameters

For a top-level parameter like `/path`, a top-level `ObjectEntry { key: "path",
value: Done }` fragment signals completion.
The aggregator yields the complete `serde_json::Value` for `path`, and the
evaluator matches against it.

#### Nested parameters with array traversal

For a nested parameter like `/patterns/paths`, [RFD D25] defines existential
semantics: the condition is met if **any** resolved value satisfies the matcher.
Because existential matching is monotonic, a rule can transition from unmatched
to matched as array elements arrive, never the reverse, which makes incremental
evaluation sound.

[RFD 043]'s fragment protocol emits `Done` at each nesting level.
For `/patterns/paths`:

```
ObjectEntry { key: "patterns", value: ArrayItem { index: 0, value:
    ObjectEntry { key: "paths", value: ArrayItem { index: 0, value:
        String("src/a.rs") } } } }
ObjectEntry { key: "patterns", value: ArrayItem { index: 0, value:
    ObjectEntry { key: "paths", value: ArrayItem { index: 0, value: Done } } } }
...
ObjectEntry { key: "patterns", value: ArrayItem { index: 0, value:
    ObjectEntry { key: "paths", value: Done } } }       ← paths[0] array complete
ObjectEntry { key: "patterns", value: ArrayItem { index: 0, value: Done } }
ObjectEntry { key: "patterns", value: ArrayItem { index: 1, value: ... } }
...
ObjectEntry { key: "patterns", value: Done }             ← patterns array complete
```

`StreamingPolicyState` tracks a boolean "matched" flag per array-traversal rule
and processes fragments as they arrive:

1. When a leaf value completes (such as `"src/a.rs"` in `patterns[0].paths[0]`),
   test it against the matcher.
   On a match, set the rule's matched flag, ignore further elements for that
   rule, and return `Decided` immediately.
2. When a leaf value does not match, keep waiting for subsequent array elements.
3. When the top-level parameter completes (`patterns` emits `Done`) with the
   matched flag still false, all nested values have been inspected without a
   match, so the rule is eliminated.

#### When evaluation blocks

Each rule's `arg` pointer targets a specific top-level parameter.
The evaluator returns `Waiting` while that parameter has not started arriving,
checks leaf values as fragments stream in, and either resolves (`Decided`) on
the first matching element or eliminates the rule when the top-level parameter
emits `Done`.
A rule on `/patterns/paths` therefore resolves as soon as any nested `paths`
element matches or `patterns` finishes streaming without a match, rather than
blocking until Flush.

### `RunMode::Edit` waits for complete arguments

Because the user cannot edit partial arguments, `edit` requires the complete
argument object.
When a rule resolves to `edit`, the streaming evaluator records the decision
immediately but defers the editor prompt until all arguments arrive.

Early resolution therefore benefits `ask`, `unattended`, and `skip` (which
rejects the tool call immediately).

### Parameter ordering convention

LLMs typically stream arguments in the order properties appear in the JSON
schema.
JP controls this order through `IndexMap` iteration in `ToolParameterConfig`,
which reflects the declaration order in the tool's TOML definition.

Most tool definitions already declare short, policy-relevant parameters first
(`path`, `source`, `util`) and large content parameters last (`content`,
`patterns`).
This RFD documents that ordering as a convention:

> Parameters referenced by `policy.run` or `policy.result` rules should be
> declared before large content parameters in the tool's parameter list.

This convention is advisory; JP does not enforce ordering or reorder parameters
automatically.
If a provider streams parameters out of schema order, the evaluator returns
`Waiting` until the needed parameter arrives.
Correctness is unaffected, and the worst case matches [RFD D25]'s post-flush
evaluation.

## Drawbacks

- **Per-rule state for array traversal.** Each array-traversal rule tracks a
  matched flag across fragment events, state that [RFD D25]'s post-flush
  evaluator does not need because the full argument object is already assembled.

- **Partial argument rendering in `ToolRenderer`.** Displaying known parameters
  alongside streaming placeholders in the permission prompt requires new
  `ToolRenderer` functionality beyond [RFD 043].

## Alternatives

### Cancel the LLM stream unconditionally on rejection

Always cancel the LLM stream when the user rejects a tool call, even if other
tool calls are in flight, and let the model re-request them in the next turn.

Rejected because cancelling discards partially-streamed tool calls that may be
nearly complete and forces an extra LLM round-trip.
Selective cancellation (only when the rejected call is the last pending) avoids
that cost.

### Speculative evaluation past `Waiting` rules

When a rule returns `Waiting`, speculatively evaluate later rules, use a
matching rule's mode provisionally, and re-evaluate when the pending parameter
arrives.

Rejected because it violates first-match-wins semantics.
A provisional decision from rule 3 is wrong if rule 1's parameter later arrives
and matches, which would show the user a prompt for rule 3's mode and then
switch behavior when rule 1 resolves.

### Top-level parameters only

Restrict streaming evaluation to single-segment `arg` pointers (such as `/path`)
and fall back to post-flush evaluation for multi-segment pointers
(`/patterns/paths`).

Rejected because `fs_modify_file`'s `patterns` array with nested `paths` is a
core use case for argument-conditional policies.
Monotonic existential array matching supports nested evaluation incrementally
without excessive complexity.

## Non-Goals

- **LLM stream cancellation with multiple pending tool calls.** When other tool
  calls are still streaming, JP discards the rejected call's chunks and keeps
  the stream open.

- **Argument reordering.** JP does not reorder JSON schema properties to make
  policy-relevant parameters stream first; schema order is a convention, not a
  guarantee.

- **Partial argument execution.** Tool execution always waits for the complete
  argument object (for example, `fs_create_file` does not start writing as soon
  as `path` is known, before `content` finishes).

## Risks and Open Questions

1. **RFD 043 completion signal granularity.** Nested evaluation assumes [RFD
   043]'s `ToolCallArgumentProgress` emits `Done` at each nesting level,
   including within arrays.
   If [RFD 043]'s fragment protocol changes during implementation, nested
   evaluation integration may need adjustment.

2. **Prompt display during streaming.** For the `json` display style, rendering
   in-progress parameters as `"#": "streaming..."` in the permission prompt
   collides with `#` as a valid JSON key, so a dedicated partial-arguments
   rendering format may be needed.

3. **Multiple tool calls in one response.** When the LLM emits multiple tool
   calls, each has independent streaming policy state.
   The existing `prompt_active` and `pending_prompts` queue in `ToolCoordinator`
   sequences concurrent prompts.

4. **Provider streaming behavior.** Early evaluation assumes providers stream
   argument JSON incrementally in small chunks.
   If a provider buffers arguments into a single large `ArgumentChunk`,
   streaming evaluation adds no benefit (and no cost beyond creating
   `StreamingPolicyState`).
   This should be validated empirically with Anthropic, OpenRouter, and OpenAI.

## Implementation Plan

### Phase 1: Policy evaluator function

- Implement `evaluate(rules: &[RunRule], known_params: &HashMap<String, Value>)
  -> PolicyDecision` in `jp_cli::cmd::query::tool::policy`, handling both
  `Decided` and `Waiting`.
- For array-traversal rules, accept a pre-resolved `Value` in `known_params`
  (the caller handles fragment aggregation; the evaluator walks the value with
  [RFD D25]'s existential semantics).
- Add unit tests covering catch-all policies, single-rule match, rule
  elimination, waiting on a missing param, catch-all after eliminations,
  implicit fallback, and nested array existential matching.

No dependencies beyond [RFD D25]'s types; can merge independently.

### Phase 2: StreamingPolicyState

- Implement `StreamingPolicyState` to wrap the evaluator with per-tool-call
  state (`known_params` map, current decision, and per-rule array match flags).
- Add `feed_parameter(key: String, value: Value) -> Option<RunMode>` to insert a
  value and re-evaluate while unresolved.
- Add `feed_nested_element(top_level_key: &str, path: &[PathSegment], value:
  Value) -> Option<RunMode>` for incremental array element evaluation.
- Add unit tests covering parameter arrival order, re-evaluation on each new
  param, early array match, and array exhaustion without a match.

Depends on Phase 1.

### Phase 3: Turn loop integration

- On `ToolCallPart::Start`, look up the tool's `RunPolicy`.
  Use the existing fast path when there are no conditional rules; otherwise
  create `StreamingPolicyState`.
- On `ToolCallArgumentProgress` with parameter completion, call
  `StreamingPolicyState::feed_parameter` or `feed_nested_element`.
  When resolved:
  - For `ask`: trigger the permission prompt with partial arguments.
  - For `unattended`: mark as pre-approved and render the tool call header with
    known arguments.
  - For `skip`: mark as completed with a skip response.
  - For `edit`: record the decision and defer the editor prompt until all
    arguments arrive.
- On Flush, if still `Waiting` (non-streaming provider fallback), evaluate once
  more with all parameters available.
- Wire up cancellation: if the user declines and this is the only pending tool
  call, cancel the LLM stream; otherwise discard remaining argument chunks for
  that tool call index.

Depends on Phase 2 and [RFD 043] Phase 3 (event plumbing).

### Phase 4: Partial argument rendering

- Extend `ToolRenderer` to accept partial argument maps with a set of
  "streaming" parameter names for placeholder display.
- Implement placeholder rendering for `function_call` and `json` display styles.
- Update the permission prompt to render partial arguments when triggered during
  streaming.

Depends on Phase 3; can be refined iteratively after the core integration lands.

## References

- [RFD D25]: Argument-conditional tool policy.
  Defines the rule types, matchers, first-match-wins evaluation, and post-flush
  evaluation model that this RFD extends.
- [RFD 043]: Incremental tool call argument streaming.
  Provides the `ToolCallArgumentProgress` events and `FragmentAggregator` that
  this RFD consumes for per-parameter completion signals.
- [RFD 075]: Tool sandbox and access policy.
- [RFD 076]: Tool access grants.

[RFD 043]: ../043-incremental-tool-call-argument-streaming.md
[RFD 075]: ../075-tool-sandbox-and-access-policy.md
[RFD 076]: ../076-tool-access-grants.md
[RFD D25]: D25-argument-conditional-tool-policy.md
