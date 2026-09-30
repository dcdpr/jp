# RFD 115: Bounded Tool Output

- **Status**: Accepted
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-07-27
- **Extends**: [RFD 109]
- **Summary**: Bounds the text of each tool call response before it is recorded
  or delivered, with a per-tool `size_threshold` and a fixed per-cycle ceiling.

## Summary

Cap the text of a tool call response before it is persisted to the conversation
stream and delivered to its caller: a configurable per-tool `size_threshold`
resolved through `conversation.tools`, plus a non-configurable ceiling over each
cycle's responses that applies regardless of source.
The recorded response and the delivered result are always the same capped text.

## Motivation

A tool call response is unbounded today.
Nothing between a tool's stdout and the provider request enforces a size limit
— not the tool, not the executor, not the coordinator.

A single oversized response does more than waste tokens.
`commit_tool_responses` writes it into the stream and calls `conv.flush()`, so
it is durably persisted.
Every subsequent turn re-sends it and gets the same `prompt is too long` error
from the provider.
The conversation is unusable until the user strips the response, with `jp
conversation compact --tools=strip-responses --over <size>` or by editing the
stream by hand.

This is not hypothetical.
A panicking `#[derive(Config)]` produced one diagnostic per expansion site;
`cargo_test` embedded the whole stderr in its error message and the resulting
request was 1,293,623 tokens against a 1,000,000 limit.

The Anthropic ACP flow fails sooner.
Claude Code substitutes a file reference for a tool result over 500,000
characters, and JP ends the turn with a stream error when it does ([RFD 110]).
Asking the model to continue re-issues the same call and fails the same way.

Bounding output inside each first-party tool (already done for the `cargo_*`
tools) does not solve this.
It covers only the tools we wrote.
MCP servers, user-defined `local` tools, and the responses the coordinator
synthesizes itself remain unbounded.

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

`size_threshold` is an upper bound, not a floor.
A tool that caps its own output lands below the configured value and raising the
threshold does not recover what the tool already discarded — so
`cargo_expand`'s local `MAX_EXPANDED_BYTES` is raised or dropped when this
lands, and first-party tools keep local caps only where they are tighter than
any threshold a user would set.

Oversized content is cut at a UTF-8 character boundary and a marker is appended,
with both sizes rendered by `ByteSize::human()`:

```
... [truncated, 623.0 KB → 256.0 KB]
```

The limit counts the final text, marker included: a response capped at `"256KB"`
is at most 256 KB as recorded and delivered.
The cut leaves room for the marker, which is under 50 bytes.

Values below `"1KB"` are rejected when the configuration loads, so the marker
always fits with room left for content.
This also catches `size_threshold = 0`, which elsewhere in JP's configuration
means "disabled" and here would reduce every response to a bare marker.
The error points at `"unlimited"` for no limit, and at `result = "skip"` for no
result at all.

Finite values use `jp_config::types::byte_size::ByteSize`, the type compaction
thresholds already use (`over = "1MB"`): a bare byte count or a human-readable
size, with binary units, so `"1MB"` is 1,048,576 bytes.
`ByteSize` has no unlimited value and does not gain one; an unlimited compaction
threshold means nothing.
The key takes a small wrapper local to the tool config instead, `SizeThreshold`,
which is either `Unlimited` or `Bytes(ByteSize)`, and
`ToolConfigWithDefaults::size_threshold()` resolves it.

### What is bounded

The limit applies to the response text: `ToolCallResponse::content()`, the
string the conversation stream records and a provider receives, whether the call
succeeded or failed.
The stream stores tool responses as text, so images, audio, and
`structured_content` never reach it, and the limit does not measure them.

A result delivered over MCP can carry those payloads.
An untruncated result keeps them.
A truncated one is rebuilt from its text, as any Host edit is ([RFD 109]), so
its non-text blocks are dropped along with the tail.

### Two layers

The cap is applied at two seams, for two different reasons.

| Seam                                                                                  | Scope                                   | Configurable |
| ------------------------------------------------------------------------------------- | --------------------------------------- | ------------ |
| `ToolCoordinator::execute_with_prompting`, as it assembles `ExecutionResult::reviews` | per-tool `size_threshold`, per response | yes          |
| `commit_tool_responses`, before the reviews are recorded                              | ceiling, whole batch                    | no           |

Both seams cap a `Review`, not a bare `ToolCallResponse`.
A `Review` is what the Host settled on for one call: the response the
conversation records, and whether the Host changed the content it was offered.
A truncation produces `Review::replaced`, so the rest of the pipeline treats it
as a Host edit.

The coordinator seam comes after `ResultMode` has run.
Every route a call finishes by writes into the phase's per-call review slots:
unattended completion, a result prompt under `Ask` or `Edit`, a lost call, a
failed inquiry, a cancellation.
The coordinator turns those slots into `ExecutionResult::reviews` in one place
when the phase ends, so one `cap_review` call there covers every route,
including the user who pastes 5 MB into the editor.
It knows each call's tool name, so it can resolve `size_threshold`, and it is on
the path `MockExecutor` takes, so the turn-loop integration tests reach it.

Because the cap runs when the phase ends, `render_result` and the edit prompt
both receive the raw response.
The terminal's full-content temp file and the user's editor keep the tail the
assistant does not get, which is the whole point of calling this a delivery
limit.

The `commit_tool_responses` seam is the last stop before the write.
Without it the configurable cap is a suggestion: it would not hold for
`size_threshold = "unlimited"`, for an MCP server that ignores everything, or
for the responses the coordinator builds itself (unavailable tool, orphan
synthesis, inquiry failure) which have no tool config to read.
It applies to the merged, index-ordered reviews, pre-resolved ones included,
before their responses are handed to the stream.

This ceiling is a budget over the whole batch, not a per-response limit.
`commit_tool_responses` persists a vector whose length is whatever the model
emitted, so a per-response ceiling leaves the total unbounded — two 2 MB
responses already exceed the context window that produced the reported failure.
The budget (`MAX_TOOL_RESPONSE_BATCH_BYTES`, 2 MB) is spent across the batch by
truncating the largest responses first, so one oversized response does not
starve its siblings.
The marker bytes of every response it cuts count against the budget.

The terminal shows the raw response, so without a signal the user sees the full
output and has no reason to think the assistant saw less.
When either cap changes a response, JP prints one line to stderr alongside the
tool call's other chrome, naming the tool, both sizes, and which cap cut it:

```
fs_list_files: sent 256.0 KB of 3.1 MB to the assistant (size_threshold)
```

It is a notice, not a prompt.
The same fact is also logged at `warn` level for traces, but a log line alone is
invisible: JP prints only errors unless `-v` is given.

### Recorded and delivered are one decision

`commit_tool_responses` records `review.response`, flushes, and only then
acknowledges each review to the execution service ([RFD 109]), which releases
the call's MCP result to its caller.
That caller is JP's own executor for a call JP submitted, and the external agent
for one an agent submitted, such as Claude Code on the Anthropic ACP flow.

A replaced review only reaches that caller if the call is parked on a barrier
that accepts replacement content.
A call under `result = "ask"` or `"edit"` is parked on the Review barrier, which
does.
A call under `result = "unattended"`, the default, never reaches a Review
barrier: the service asks the Host to record its result and then delivers that
same result, and the Record barrier's reply carries no content.
Capping only the recorded text would store the truncated response while the
caller still received the full payload.
For a JP-submitted call the executor would report a delivery mismatch; Claude
Code would receive the multi-megabyte result intact and replace it with a file
reference the model cannot read.

This RFD extends the Record barrier: its reply may carry replacement content,
and the service delivers that instead of its own result.
The Host sends a replacement exactly when the review is replaced, rebuilt from
the recorded text as at every other barrier.
Every capped response is then delivered as recorded, whichever barrier its call
is parked on.

### This is a delivery limit, not a display limit

`style.inline_results` already truncates results, and its documentation promises
that "the full tool call results will be sent back to the assistant, regardless
of this setting."
That is a terminal-rendering axis.
`size_threshold` is a delivery axis.
The two stay separate keys: collapsing them would break the reader who sets
`inline_results = 20` for a quiet terminal and expects the model to still see
everything.
That documented promise becomes "up to `size_threshold`" once this lands.

### Bytes, not tokens

Tokens are what actually bind, but a per-provider tokenizer is a real dependency
for a guard that only needs to be approximately right.
Byte size is deterministic, free to compute, and provider-agnostic; for prose
and code it tracks token count within roughly a factor of two.
The draft attachment size policy reached the same conclusion for the same
reason.

The naming (`size_threshold`, human-readable sizes, byte-based comparison) is
shared with that draft deliberately.
Tool output and attachment content are the same problem — untrusted content of
unknown size entering the context — and should not end up with two parallel
vocabularies.
`ByteSize` already exists for compaction thresholds, so `size_threshold` reads a
size the way a user already writes one, and the attachment policy adopts it too.

## Drawbacks

- **A truncated result can be a broken result.** Cutting a JSON or XML payload
  mid-structure yields something the assistant may fail to parse, where the
  untruncated response would have worked.
  The marker tells it what happened, but the turn is still degraded.
- **Head-only truncation discards the conclusion.** Test output and logs often
  put the useful summary last.
  This cut keeps the beginning.
- **Two caps to reason about.** A user hitting the ceiling with a raised
  `size_threshold` gets truncation they explicitly configured against.
  The notice names the batch ceiling as the cap that fired, but the user still
  has to learn that there are two.
- **The batch budget makes one tool's size depend on its siblings.** The same
  tool returning the same output is truncated in a busy cycle and not in a quiet
  one.
  That is the price of bounding the total, but it does make the ceiling
  non-deterministic from any single tool's point of view.
- **Bytes are the wrong unit for the actual constraint.** A response under the
  threshold can still overflow a small context window; a base64 blob costs far
  more tokens per byte than prose.
- **Neither cap bounds the request.** Both bound what one cycle adds.
  History accumulated over many cycles can still exceed the context window;
  fitting a request to the window is automatic compaction's job, not this RFD's.
- **A truncated MCP result loses its non-text content.** Images, audio, and
  structured content on a result whose text is cut are dropped with the tail.

## Alternatives

**Cap inside each tool only.** What the `cargo_*` tools do now.
Best truncation quality, because the tool knows which end matters — but it
cannot cover MCP servers or user-defined tools, which is where the risk actually
lives.
Kept as a complement, not a substitute.

**Cap in the execution service (`jp_mcp::server`).** Every executed call passes
through `deliver_result`, which already reads the tool's config and settles the
delivered result, so a cap there would be recorded and delivered as one value
with no barrier change.
Rejected for three reasons.
The Host builds its response from the service's recorded result, so the terminal
would lose the tail along with the assistant.
The service sees neither the batch nor the responses the coordinator
synthesizes, so the ceiling would still need the Record barrier extension.
And `MockExecutor` bypasses it, so the turn-loop integration tests would not
exercise the configurable cap.

**Token-based limits.** Correct unit, disproportionate cost.
See "Bytes, not tokens."

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
  orthogonal to the cap.
  The renderer already writes results to a temp file, but only for the
  terminal's OSC 8 link.
  Doing it properly needs a stable, content-addressed location — [RFD 066]'s
  territory.
- **Head-and-tail or content-aware truncation.** A plain head cut first; a
  second strategy needs a case behind it.
- **`size_policy` variants** (`ask`, `reject`, `allow`).
  Deferred, not rejected — the threshold is the same concept if they arrive.
- **Bounding request arguments, attachments, or assistant messages.** This RFD
  covers tool responses only.
- **Bounding non-text MCP payloads.** Images, audio, and `structured_content`
  are not measured; a result under the text limit is delivered with whatever
  else it carries.
  Measuring them belongs with typed content blocks ([RFD 058]).
- **Unifying the marker text with `.config/jp/tools`.** That crate is project
  maintenance tooling, not part of the main codebase; its in-tool markers stay
  as they are.
  Phase 5 changes one cap *value* there, not the wording.

## Risks and Open Questions

- **What default?** 256 KB (roughly 64k tokens) is generous enough that no
  first-party tool reaches it and tight enough to stop the reported failure.
  A tighter 64 KB would catch more real waste but would silently start
  truncating `cargo_expand` and large `git_diff_commit` output — a behavior
  change users would notice.
  Tightening later is easy; loosening after complaints is not.
- **Is 2 MB the right ceiling?** It is a workload safeguard, not a promise that
  a request fits.
  At the rate above (256 KB for roughly 64k tokens), 2 MB is around 500k tokens,
  several times the 131,072-token window of the smaller Cerebras models, and no
  byte value could guarantee a fit once history accumulates.
  It has to sit well above any plausible `size_threshold` while still stopping
  the reported 1.3M-token failure.
- **The ACP flow has a tighter limit than either cap.** Claude Code substitutes
  a file reference for a result over 500,000 characters, and JP ends the turn
  when it does ([RFD 110]).
  The 256 KB default sits below that, but a user who raises `size_threshold`
  past it, or sets `unlimited`, gets the hard failure back.
  A provider-supplied per-response ceiling, applied as the lower of it and
  `size_threshold`, would close that; `Provider::mcp_tool_metadata` already
  carries the ACP limit.
- **Largest-first apportioning is a guess.** Truncating the largest responses
  until the batch fits is the obvious rule, but an even split or a proportional
  one may read better in practice.
  Worth deciding against a real multi-tool cycle rather than in the abstract.
- **Conversations already carrying an oversized response** are not repaired by
  this RFD.
  `jp conversation compact --tools=strip-responses --over <size>` removes it
  from what the provider sees without calling a model; the payload stays in
  storage.
  Editing the stream by hand remains the fallback.
- **Does the notice show under `--quiet`?** Hiding it hides the only sign that
  the assistant got less than the terminal showed.
  Ticket T-0ez6tx1 asks the same question of abnormal turn endings; this notice
  follows whatever that settles rather than deciding separately.
- **Hyrum's Law on the marker text.** Once the marker string is in tool
  responses, assistants and user scripts will match on it.
  It should be settled before the first release, not iterated on, and that
  includes the `ByteSize::human()` format it embeds.

## Implementation Plan

**Phase 1 — config types.** `SizeThreshold` (`"unlimited"` or a `ByteSize`),
`size_threshold` on `ToolsDefaultsConfig` and `ToolConfig` with the usual
`AssignKeyValue`, `PartialConfigDelta`, `FillDefaults`, and `ToPartial` impls,
and a resolver on `ToolConfigWithDefaults`, plus a `Validator` that rejects
values below `"1KB"`, with a test that `0` fails with the hint toward
`"unlimited"`.
Pure config, no behavior change.
Mergeable alone.

**Phase 2 — replacement at the Record barrier.** Let the Record barrier's reply
carry replacement content, have the service deliver it, and have the Host send
it when the review is replaced.
Two tests: a service test that a replacement given at Record is what the MCP
caller receives, and an executor test that an unattended call acknowledged with
a replaced review completes without a delivery mismatch.
Mergeable alone; phases 3 and 4 depend on it.

**Phase 3 — the configurable cap.** Add `cap_review` and apply it where
`execute_with_prompting` assembles `ExecutionResult::reviews`.
Two turn-loop integration tests: a `MockExecutor` returning an oversized result
under `ResultMode::Unattended`, and an oversized result edited under
`ResultMode::Edit`, each asserting the *persisted* response is capped, is within
the threshold including its marker, and that the exact notice line was printed.
Depends on phases 1 and 2.

**Phase 4 — the batch ceiling.** `MAX_TOOL_RESPONSE_BATCH_BYTES` in
`commit_tool_responses`, apportioned largest-first across the merged reviews,
with truncated entries replaced and a notice per truncated tool.
Two tests: one response over the budget with `size_threshold = "unlimited"`
configured, and several responses individually under it that exceed the budget
together, each asserting the batch fits the budget including markers and the
exact notice lines.
Depends on phase 2; reviewed after phase 3 so the interaction between the two
caps is visible in one place.

**Phase 5 — raise the tool-local caps.** Drop or raise `MAX_EXPANDED_BYTES` in
`cargo_expand` so the central threshold is the binding limit for that tool.
Small, and only meaningful once phase 3 has landed.

**Phase 6 — documentation.** Correct the `InlineResults` doc comment, which
promises full delivery to the assistant, and document `size_threshold` in
`docs/configuration.md`.

## References

- [RFD 058] — typed content blocks, where non-text payload limits belong.
- [RFD 066] — content-addressable blob store, the eventual home for full-output
  retrieval.
- [RFD 109] — the execution service and the Host barriers this RFD extends.
- [RFD 110] — the ACP flow and its inline tool result limit.
- RFD titled `large-attachment-size-policy` (draft) — the attachment-side size
  policy this RFD shares its vocabulary with.

[RFD 058]: 058-typed-content-blocks-for-tool-responses.md
[RFD 066]: 066-content-addressable-blob-store.md
[RFD 109]: 109-in-process-jp-mcp-server.md
[RFD 110]: 110-anthropic-subscription-queries-via-acp.md
