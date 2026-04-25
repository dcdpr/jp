# RFD D26: Streaming Policy Evaluation for Tool Call Arguments

- **Status**: Draft
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-07-24
- **Extends**: [RFD D25]

## Summary

This RFD extends [RFD D25]'s argument-conditional tool policies with streaming
evaluation. Instead of waiting for the complete argument object before
evaluating policy rules, JP evaluates rules incrementally as individual
parameters finish streaming. This enables early permission prompts — seconds
before large arguments like file content finish arriving — and early
cancellation when the user declines a tool call mid-stream.

## Motivation

[RFD D25] introduces conditional policy rules that match on argument values. Its
evaluation model waits for all arguments to arrive before evaluating:

```txt
ToolCallPart::Start { id, name }     ← tool name known
ArgumentChunk("{"path":"src/...")    ← arguments streaming
ArgumentChunk("...content":"fn...")  ← still streaming (possibly seconds)
ArgumentChunk("...main() {}")        ← still streaming
Flush                                ← arguments parsed, policy evaluated NOW
```

For `fs_create_file` with a rule `{ arg = "/path", prefix = "src/", mode =
"unattended" }`, the decision depends only on `path`. The `path` value finishes
streaming in the first chunk — but JP waits for `content` (potentially thousands
of lines) before acting on it.

The cost of waiting:

- **Wasted streaming time.** If the policy resolves to `ask` and the user
  declines, all time spent streaming `content` was wasted. For large files this
  is seconds of LLM output tokens billed but never used.
- **Delayed approval.** If the policy resolves to `unattended`, execution could
  begin as soon as `path` is known. Instead it waits for the full argument
  object.
- **Delayed prompt.** If the policy resolves to `ask`, the user could be
  reviewing and deciding while `content` streams in the background. Instead they
  wait, then decide, then wait again for execution.

## Design

### Prerequisites

This RFD depends on two prior RFDs:

- **[RFD D25]** defines the conditional policy types (`RunPolicy`, `RunRule`,
  `ParamCondition`, `TypedMatcher`), the `policy` config namespace, and the
  first-match-wins evaluation model. This RFD extends [RFD D25]'s post-flush
  evaluation with a streaming equivalent.
- **[RFD 043]** introduces incremental argument parsing via
  `ToolCallArgumentProgress` events. Each event carries a `StreamFragment`;
  `ObjectEntry { key, value: Done }` signals that a parameter's value is
  complete at any nesting level. This RFD consumes those signals to drive policy
  evaluation.

### Evaluation algorithm

The core evaluator is a pure function. It takes the policy's rule list and a map
of parameter values received so far, and returns `Option<RunMode>` — `Some` if a
rule matched, `None` if the first non-eliminated rule needs a parameter that
hasn't arrived yet.

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

The critical property: **a rule whose parameter hasn't arrived yet blocks
evaluation.** First-match-wins means that rule has higher priority than
everything below it. Skipping it to evaluate a lower-priority rule could produce
the wrong mode.

Policies without conditional rules (string aliases like `run = "ask"`, or a
single catch-all rule) resolve on the first call with an empty `known_params`
map — the catch-all matches immediately. These go through the existing
permission fast path in `ToolCoordinator::decide_permission`; no
`StreamingPolicyState` is created.

### Where the evaluator lives

The evaluation function lives in `jp_cli::cmd::query::tool::policy`. It imports
the rule types from `jp_config::conversation::tool::policy` (defined by [RFD
D25]) and operates on `serde_json::Value` for parameter values.

`jp_config` defines the types; `jp_cli` evaluates them at runtime.

### Integration with ToolCallArgumentProgress

[RFD 043] makes `EventBuilder::handle_part` return
`Vec<ToolCallArgumentProgress>` for `ArgumentChunk` events. Each progress event
carries a `StreamFragment`. The turn loop forwards these to the `ToolRenderer`
for incremental display.

This RFD adds a second consumer: the policy evaluator. A
`StreamingPolicyState` struct, held per in-flight tool call, tracks which
parameters have completed and feeds them to the evaluator:

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

When the turn loop receives a `ToolCallArgumentProgress` whose fragment
signals a parameter completion (see [Nested argument
paths](#nested-argument-paths) for how completion is detected at any depth), it:

1. Uses the `FragmentAggregator` (from [RFD 043]) to obtain the complete
   `serde_json::Value` for that parameter.
2. Inserts it into `known_params`.
3. Re-evaluates: calls the evaluator with the updated `known_params`.
4. If the result is `Decided`, triggers the permission flow immediately.

### Integration point in the turn loop

Today, permission is decided at the Flush boundary in `turn_loop.rs` (after
`EventBuilder` emits the final `ToolCallRequest`). The streaming evaluator runs
earlier — during the `StreamingLoopEvent::Llm` match arm, alongside the existing
`ToolCallArgumentProgress` forwarding to the renderer.

The flow per tool call becomes:

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

The Flush fallback handles providers that don't stream arguments incrementally
(e.g., Ollama emitting the entire argument object in one chunk). In that case,
all `ToolCallArgumentProgress` events arrive in the same batch as Flush, and
evaluation completes immediately. There is no separate "post-flush evaluation
path" — the same evaluator is called, just with all parameters available at
once.

### Early permission prompt with partial arguments

When the evaluator resolves to `ask` before all arguments have arrived, the
permission prompt is shown with partial argument information. The turn loop
renders completed parameters and shows a placeholder for in-progress ones.

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

The user sees the parameter that triggered the policy rule (e.g., `path`) and
can make an informed decision. If the user approves, streaming continues and
execution begins when all arguments are complete. If the user declines, the tool
call is cancelled immediately.

Partial argument rendering is new functionality that this RFD requires from
the `ToolRenderer`. [RFD 043] defines the progress event protocol but
explicitly defers renderer changes. This RFD's Phase 4 covers the renderer
work needed to display partial arguments during the permission prompt.

### Cancellation on rejection

When the user declines a tool call during streaming, JP:

1. Marks the tool call as `Completed` with a "skipped by user" response.
2. Discards remaining `ArgumentChunk` events for that tool call index.
3. **If this is the only pending tool call**, cancels the LLM response stream.
   LLMs emit tool calls as the terminal content of a response — no message
   text follows. Cancelling the stream avoids generating (and billing for)
   argument tokens that will be discarded. For a thousand-line `fs_create_file`,
   this can save significant token cost.
4. **If other tool calls are still in flight**, keeps the stream alive so
   those tool calls can complete. The rejected tool call's argument chunks are
   silently discarded.

Stream cancellation uses the existing `CancellationToken` infrastructure. The
turn loop already handles mid-stream cancellation via `Ctrl+C` signals; the
mechanism is identical.

### Nested argument paths

[RFD D25] supports `arg` pointers at any depth — `/path` (top-level),
`/patterns/paths` (nested array), `/patterns/old` (nested string). Streaming
evaluation works at all depths by consuming [RFD 043]'s nested `Done` signals.

#### Top-level parameters

For a top-level parameter like `/path`, completion is signaled by a top-level
`ObjectEntry { key: "path", value: Done }` in the fragment stream. The
aggregator yields the complete `serde_json::Value` for `path`. The evaluator
matches against it.

#### Nested parameters with array traversal

For a nested parameter like `/patterns/paths`, [RFD D25] defines existential
semantics: the condition is met if **any** resolved value satisfies the matcher.

Existential matching is monotonic: the decision can go from "no match yet" to
"match" as array elements arrive, but never from "match" to "no match." This
makes incremental evaluation safe.

[RFD 043]'s fragment protocol emits `Done` at each nesting level. For
`/patterns/paths`:

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

The evaluator processes this as follows:

1. As each leaf value completes (e.g., `"src/a.rs"` in `patterns[0].paths[0]`),
   check the matcher. If it matches, the rule is satisfied — return `Decided`
   immediately. No need to wait for the rest of the array.
2. If a leaf value doesn't match, continue. More elements may arrive.
3. When the top-level parameter completes (`patterns` Done), all nested
   values have been seen. If no element matched, the rule is eliminated.

The `StreamingPolicyState` tracks per-rule match state for array traversals: a
boolean "matched" flag. Once set, the rule is decided and further elements for
that rule are ignored. If the top-level parameter completes without a match, the
flag remains false and the rule is eliminated.

#### When evaluation blocks

A rule's `arg` pointer references a specific top-level parameter. The evaluator
returns `Waiting` only when that top-level parameter hasn't started arriving
yet. Once any fragment for the parameter appears, the evaluator is actively
checking. Once the parameter's `Done` arrives, the rule is either matched
(Decided) or eliminated (try next rule).

This means a rule on `/patterns/paths` doesn't block evaluation until Flush —
it resolves as soon as either (a) any nested `paths` element matches, or (b)
`patterns` finishes streaming without a match.

### RunMode::Edit waits for complete arguments

When the evaluator resolves to `edit`, the editor needs the complete argument
object — the user can't edit partial arguments. The streaming evaluator treats
`edit` as decided (the mode is known) but defers the actual editor prompt until
all arguments have arrived.

`ask` and `unattended` are the modes that benefit from early action. `skip`
also benefits trivially (the tool is rejected immediately).

### Parameter ordering convention

LLMs typically stream arguments in the order properties appear in the JSON
schema. JP controls this order through `IndexMap` iteration in
`ToolParameterConfig`, which reflects the declaration order in the tool's TOML
definition.

Most tool definitions already declare short, policy-relevant parameters first
(`path`, `source`, `util`) and large content parameters last (`content`,
`patterns`). This RFD documents this as a convention:

> Parameters referenced by `policy.run` or `policy.result` rules should be
> declared before large content parameters in the tool's parameter list.

This is a soft convention. JP does not enforce ordering or reorder parameters
automatically. If a provider streams parameters out of schema order, the
evaluator returns `Waiting` until the needed parameter arrives — no incorrect
decisions, just lost optimization. The worst case is identical to [RFD D25]'s
current behavior (evaluate after all arguments arrive).

## Drawbacks

- **Monotonic array evaluation adds per-rule state.** Each rule with an array
  traversal needs a "matched" flag tracked across multiple fragment events.
  This is simple bookkeeping, but it's state that doesn't exist in [RFD D25]'s
  post-complete evaluation where the full argument object is available.

- **Partial argument rendering requires new renderer work.** The permission
  prompt needs to display known parameters alongside streaming placeholders.
  This is new functionality in the `ToolRenderer` that doesn't exist today
  and isn't provided by [RFD 043].

## Alternatives

### Cancel the LLM stream unconditionally on rejection

Always cancel the LLM stream when the user rejects a tool call, even if other
tool calls are in flight. The other tool calls would be re-requested in the
next turn.

Rejected. Cancelling discards partially-streamed tool calls that may be nearly
complete. The re-request costs a full additional LLM round-trip. Selective
cancellation (only when the rejected call is the last pending) is strictly
better.

### Speculative evaluation past Waiting rules

When a rule returns `Waiting`, speculatively evaluate later rules to see if any
can be decided. If a later rule matches, use its mode provisionally and
re-evaluate when the pending parameter arrives.

Rejected. This violates first-match-wins semantics. A provisional decision
from rule 3 might be wrong if rule 1's parameter arrives and matches. The user
would see a prompt based on rule 3's mode, then potentially a different
behavior when rule 1 resolves.

### Top-level parameters only

Restrict streaming evaluation to rules whose `arg` pointer has a single segment
(e.g., `/path`). Multi-segment pointers (`/patterns/paths`) fall back to
evaluation after all arguments arrive.

Rejected. `fs_modify_file`'s `patterns` array with nested `paths` is a core
use case for argument-conditional policies. Deferring nested evaluation removes
the streaming benefit for one of the most important tools. The monotonic
property of existential array matching makes incremental nested evaluation
sound without excessive complexity.

## Non-Goals

- **LLM stream cancellation with multiple pending tool calls.** When other tool
  calls are still streaming, the rejected call's chunks are discarded but the
  stream stays alive.

- **Argument reordering.** Manipulating the JSON schema property order to
  ensure policy-relevant parameters stream first. The schema order is a
  convention, not a guarantee.

- **Partial argument execution.** Starting tool execution before all arguments
  arrive (e.g., beginning a file write as soon as `path` is known, before
  `content` finishes). Tool execution always uses the complete argument object.

## Risks and Open Questions

1. **RFD 043 completion signal granularity.** This RFD assumes [RFD 043]'s
   `ToolCallArgumentProgress` emits `Done` at each nesting level, including
   within arrays. If 043's fragment protocol changes during implementation,
   the nested evaluation integration may need adjustment.

2. **Prompt display during streaming.** The permission prompt shows partial
   arguments. For the `json` display style, the placeholder syntax (`"#":
   "streaming..."`) is not ideal — `#` is a valid JSON key. A dedicated
   partial-arguments rendering approach may be needed.

3. **Multiple tool calls in one response.** When the LLM emits multiple tool
   calls, each has independent streaming policy state. Concurrent prompts need
   sequencing — the existing `prompt_active` / `pending_prompts` queue in
   `ToolCoordinator` handles this.

4. **Provider streaming behavior.** The optimization assumes providers stream
   argument JSON incrementally in small chunks. If a provider buffers arguments
   and emits them as a single large `ArgumentChunk`, streaming evaluation adds
   no benefit (but no cost beyond creating the `StreamingPolicyState`). This
   should be validated empirically with Anthropic, OpenRouter, and OpenAI.

## Implementation Plan

### Phase 1: Policy evaluator function

- Implement `evaluate(rules: &[RunRule], known_params: &HashMap<String, Value>)
  -> PolicyDecision` in `jp_cli::cmd::query::tool::policy`.
- Handle both return cases: `Decided`, `Waiting`.
- For array-traversal rules, accept a pre-resolved `Value` in `known_params`
  (the caller handles fragment aggregation; the evaluator walks the value with
  [RFD D25]'s existential semantics).
- Unit tests covering: catch-all policies, single-rule match, rule
  elimination, waiting on missing param, catch-all after eliminations, implicit
  fallback, nested array existential matching.

No dependencies beyond [RFD D25]'s types. Can merge independently.

### Phase 2: StreamingPolicyState

- Implement `StreamingPolicyState` struct that wraps the evaluator with
  per-tool-call state (`known_params` map, current decision, per-rule array
  match flags).
- Method `feed_parameter(key: String, value: Value) -> Option<RunMode>`
  that inserts the value and re-evaluates if currently unresolved.
- Method `feed_nested_element(top_level_key: &str, path: &[PathSegment], value:
  Value) -> Option<RunMode>` for incremental array element evaluation.
- Unit tests covering: parameter arrival order, re-evaluation on each new
  param, early array match, array exhaustion without match.

Depends on Phase 1.

### Phase 3: Turn loop integration

- On `ToolCallPart::Start`: look up the tool's `RunPolicy`. If no conditional
  rules, use the existing fast path. Otherwise, create `StreamingPolicyState`.
- On `ToolCallArgumentProgress` with parameter completion: call
  `StreamingPolicyState::feed_parameter` or `feed_nested_element`.
  If resolved:
  - For `ask`: trigger the permission prompt with partial arguments.
  - For `unattended`: mark as pre-approved, render the tool call header with
    known arguments.
  - For `skip`: mark as completed with a skip response.
  - For `edit`: record the decision but defer the editor prompt until all
    arguments arrive.
- On Flush: if still `Waiting` (non-streaming provider fallback), all
  parameters are now available — evaluate once more.
- Wire up cancellation: if the user declines and this is the only pending tool
  call, cancel the LLM stream. Otherwise discard remaining argument chunks for
  that tool call index.

Depends on Phase 2 and [RFD 043] Phase 3 (event plumbing).

### Phase 4: Partial argument rendering

- Extend `ToolRenderer` to accept partial argument maps with a set of
  "streaming" parameter names for placeholder display.
- Implement placeholder rendering for `function_call` and `json` display styles.
- Update the permission prompt to render partial arguments when triggered
  during streaming.

Depends on Phase 3. Can be refined iteratively after the core integration lands.

## References

- [RFD D25] — Argument-conditional tool policy. Defines the rule types,
  matchers, first-match-wins evaluation, and post-flush evaluation model that
  this RFD extends.
- [RFD 043] — Incremental tool call argument streaming. Provides the
  `ToolCallArgumentProgress` events and `FragmentAggregator` that this RFD
  consumes for per-parameter completion signals.
- [RFD 075] — Tool sandbox and access policy.
- [RFD 076] — Tool access grants.

[RFD D25]: D25-argument-conditional-tool-policy.md
[RFD 043]: 043-incremental-tool-call-argument-streaming.md
[RFD 075]: 075-tool-sandbox-and-access-policy.md
[RFD 076]: 076-tool-access-grants.md
