# RFD D24: Bounded Tool Output

- **Status**: Draft
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-07-27

## Summary

Cap the size of a tool call response before it is delivered to the assistant and
persisted to the conversation stream: a configurable per-tool `size_threshold`
resolved through `conversation.tools`, plus a non-configurable hard ceiling that
applies to every response regardless of source.

## Motivation

A tool call response is unbounded today. Nothing between a tool's stdout and the
provider request enforces a size limit — not the tool, not the executor, not the
coordinator.

A single oversized response does more than waste tokens. `commit_tool_responses`
writes it into the stream and calls `conv.flush()`, so it is durably persisted.
Every subsequent turn re-sends it and gets the same `prompt is too long` error
from the provider. The conversation is unusable until someone edits the stream by
hand.

This is not hypothetical. A panicking `#[derive(Config)]` produced one diagnostic
per expansion site; `cargo_test` embedded the whole stderr in its error message
and the resulting request was 1,293,623 tokens against a 1,000,000 limit.

Bounding output inside each first-party tool (already done for the `cargo_*`
tools) does not solve this. It covers only the tools we wrote. MCP servers,
user-defined `local` tools, and the responses the coordinator synthesizes itself
remain unbounded.

## Design

### Configuration

One new key, resolved per-tool then from the `'*'` defaults, exactly like `run`,
`result`, and `cancellation_response`:

```toml
[conversation.tools.'*']
# Maximum size of a single tool response delivered to the assistant.
# Accepts human-readable sizes ("512KB", "1MB"), a bare byte count, or
# "unlimited".
size_threshold = "256KB"

[conversation.tools.some_mcp_tool]
# Raise it where the large payload is the point of the tool.
size_threshold = "1MB"
```

`size_threshold` is an upper bound, not a floor. A tool that caps its own output
lands below the configured value and raising the threshold does not recover what
the tool already discarded — so `cargo_expand`'s local `MAX_EXPANDED_BYTES` is
raised or dropped when this lands, and first-party tools keep local caps only
where they are tighter than any threshold a user would set.

Oversized content is cut at a UTF-8 character boundary and a marker is
appended:

```
... [truncated, 623 KB → 256 KB]
```

`size_threshold` accepts a new `ByteSize` type in `jp_config::types`, and
`ToolConfigWithDefaults::size_threshold()` resolves it.

### Two layers

The cap is applied at two seams, for two different reasons.

| Seam | Scope | Configurable |
| --- | --- | --- |
| `ToolCoordinator::handle_tool_result` | per-tool `size_threshold`, per response | yes |
| `commit_tool_responses` | hard ceiling, whole batch | no |

The coordinator seam is where `ResultMode` already dispatches: it has
`tools_config` in hand and is on the path that `MockExecutor` takes, so it is
reachable from the turn-loop integration tests.

Within that seam the cap applies to the copy stored as `tracked_response`, and
only after rendering and after any `ResultMode::Edit` editing. `render_result`
and the edit prompt both receive the raw response, so the terminal's
full-content temp file and the user's editor keep the tail that the assistant
does not get — which is the whole point of calling this a delivery limit.
Capping after editing also keeps the editor from becoming a way around the cap.

The `commit_tool_responses` seam is the last stop before the write. Without it
the configurable cap is a suggestion: it would not hold for `size_threshold =
"unlimited"`, for an MCP server that ignores everything, or for the responses the
coordinator builds itself (unavailable tool, orphan synthesis, inquiry failure)
which have no tool config to read.

This ceiling is a budget over the whole batch, not a per-response limit.
`commit_tool_responses` persists a vector whose length is whatever the model
emitted, so a per-response ceiling leaves the total unbounded — two 2 MB
responses already exceed the context window that produced the reported failure.
The budget (`MAX_TOOL_RESPONSE_BATCH_BYTES`, 2 MB) is spent across the batch by
truncating the largest responses first, so one oversized response does not
starve its siblings, and a `warn!` names each tool that was cut.

### This is a delivery limit, not a display limit

`style.inline_results` already truncates results, and its documentation promises
that "the full tool call results will be sent back to the assistant, regardless
of this setting." That is a terminal-rendering axis. `size_threshold` is a
delivery axis. The two stay separate keys: collapsing them would break the
reader who sets `inline_results = 20` for a quiet terminal and expects the model
to still see everything. That documented promise becomes "up to
`size_threshold`" once this lands.

### Bytes, not tokens

Tokens are what actually bind, but a per-provider tokenizer is a real dependency
for a guard that only needs to be approximately right. Byte size is
deterministic, free to compute, and provider-agnostic; for prose and code it
tracks token count within roughly a factor of two. The draft attachment size
policy reached the same conclusion for the same reason.

The naming (`size_threshold`, human-readable sizes, byte-based comparison) is
shared with that draft deliberately. Tool output and attachment content are the
same problem — untrusted content of unknown size entering the context — and
should not end up with two parallel vocabularies. Whichever ships first defines
`ByteSize`; the other adopts it.

## Drawbacks

- **A truncated result can be a broken result.** Cutting a JSON or XML payload
  mid-structure yields something the assistant may fail to parse, where the
  untruncated response would have worked. The marker tells it what happened, but
  the turn is still degraded.
- **Head-only truncation discards the conclusion.** Test output and logs often
  put the useful summary last. This cut keeps the beginning.
- **Two caps to reason about.** A user hitting the ceiling with a raised
  `size_threshold` gets truncation they explicitly configured against, and has to
  find the warning to understand why.
- **The batch budget makes one tool's size depend on its siblings.** The same
  tool returning the same output is truncated in a busy cycle and not in a quiet
  one. That is the price of bounding the total, but it does make the ceiling
  non-deterministic from any single tool's point of view.
- **Bytes are the wrong unit for the actual constraint.** A response under the
  threshold can still overflow a small context window; a base64 blob costs far
  more tokens per byte than prose.

## Alternatives

**Cap inside each tool only.** What the `cargo_*` tools do now. Best truncation
quality, because the tool knows which end matters — but it cannot cover MCP
servers or user-defined tools, which is where the risk actually lives. Kept as a
complement, not a substitute.

**Cap in `ToolDefinition::execute` (`jp_llm`).** The single convergence point for
`local`, `mcp`, and `builtin` sources, and it already receives
`ToolConfigWithDefaults`, so the cap would need no new plumbing. Rejected because
`MockExecutor` and `TestExecutorSource` bypass it entirely: none of the turn-loop
integration tests would exercise the cap.

**Token-based limits.** Correct unit, disproportionate cost. See "Bytes, not
tokens."

**A `size_policy` with an `ask` variant**, mirroring the attachment size policy.
Prompting mid-turn to approve an 800 KB result is a poor interaction, and
`ResultMode::Ask` already covers "let me look before this goes to the model."

**No configuration, ceiling only.** Simpler, and it does fix the reported bug.
Rejected because the useful thresholds differ by an order of magnitude between
tools: a diagnostic dump wants tens of kilobytes, `cargo_expand` legitimately
wants megabytes.

## Non-Goals

- **Spilling the full output somewhere the assistant can read it.** Handing back
  a path the assistant can `fs_read_file` is the natural follow-up and is
  orthogonal to the cap. The renderer already writes results to a temp file, but
  only for the terminal's OSC 8 link. Doing it properly needs a stable,
  content-addressed location — [RFD 066]'s territory.
- **Head-and-tail or content-aware truncation.** A plain head cut first; a second
  strategy needs a case behind it.
- **`size_policy` variants** (`ask`, `reject`, `allow`). Deferred, not rejected —
  the threshold is the same concept if they arrive.
- **Bounding request arguments, attachments, or assistant messages.** This RFD
  covers tool responses only.
- **Unifying the marker text with `.config/jp/tools`.** That crate is project
  maintenance tooling, not part of the main codebase; its in-tool markers stay
  as they are. Phase 4 changes one cap *value* there, not the wording.

## Risks and Open Questions

- **What default?** 256 KB (roughly 64k tokens) is generous enough that no
  first-party tool reaches it and tight enough to stop the reported failure. A
  tighter 64 KB would catch more real waste but would silently start truncating
  `cargo_expand` and large `git_diff_commit` output — a behavior change users
  would notice. Tightening later is easy; loosening after complaints is not.
- **Is 2 MB the right ceiling?** It has to be well above any plausible
  `size_threshold` while staying below the smallest context window we support.
  Worth checking against the actual per-provider limits before settling.
- **Largest-first apportioning is a guess.** Truncating the largest responses
  until the batch fits is the obvious rule, but an even split or a
  proportional one may read better in practice. Worth deciding against a real
  multi-tool cycle rather than in the abstract.
- **Conversations already carrying an oversized response** are not repaired by
  this RFD. They still need manual stream editing.
- **Hyrum's Law on the marker text.** Once the marker string is in tool
  responses, assistants and user scripts will match on it. It should be settled
  before the first release, not iterated on.

## Implementation Plan

**Phase 1 — config types.** `ByteSize` in `jp_config::types` (parses `"512KB"`,
`"1MB"`, a bare integer, `"unlimited"`), `size_threshold` on
`ToolsDefaultsConfig` and `ToolConfig` with the usual `AssignKeyValue`,
`PartialConfigDelta`, `FillDefaults`, and `ToPartial` impls, and a resolver on
`ToolConfigWithDefaults`. Pure config, no behavior change. Mergeable alone.

**Phase 2 — the configurable cap.** Apply `size_threshold` in
`ToolCoordinator::handle_tool_result`, on the value assigned to
`tracked_response` and after the render and edit paths have seen the raw
response. One vertical slice: a turn-loop integration test with a `MockExecutor`
returning an oversized result, asserting the *persisted* response is capped and
carries the marker. A second test edits an oversized result via
`ResultMode::Edit` and asserts the edited value is capped too. Depends on
phase 1.

**Phase 3 — the batch ceiling.** `MAX_TOOL_RESPONSE_BATCH_BYTES` in
`commit_tool_responses`, apportioned largest-first across the response vector,
with a `warn!` per truncated tool. Two tests: one response over the budget with
`size_threshold = "unlimited"` configured, and several responses individually
under it that exceed the budget together. Independent of phase 2, but reviewed
after it so the interaction between the two caps is visible in one place.

**Phase 4 — raise the tool-local caps.** Drop or raise `MAX_EXPANDED_BYTES` in
`cargo_expand` so the central threshold is the binding limit for that tool.
Small, and only meaningful once phase 2 has landed.

**Phase 5 — documentation.** Correct the `InlineResults` doc comment, which
promises full delivery to the assistant, and document `size_threshold` in
`docs/configuration.md`.

## References

- [RFD 066] — content-addressable blob store, the eventual home for full-output
  retrieval.
- `D12-large-attachment-size-policy` (draft) — the attachment-side size policy
  this RFD shares its vocabulary with.

[RFD 066]: ../066-content-addressable-blob-store.md
