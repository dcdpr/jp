# RFD 115: Bounded Tool Output

- **Status**: Accepted
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-07-27
- **Extends**: [RFD 109]
- **Summary**: Caps tool response text before persistence and delivery using a
  configurable per-tool `size_threshold` and a per-cycle ceiling.

## Summary

This RFD bounds tool call response text before it is persisted to the
conversation stream and delivered to the caller.
Two limits apply: a configurable per-tool `size_threshold` resolved through
`conversation.tools`, and a non-configurable byte ceiling across all tool
responses in a single cycle.
The persisted response and the delivered result always contain the same capped
text.

## Motivation

Tool call responses have no size limit.
Nothing between a tool's stdout and the provider request caps the output: not
the tool, the executor, or the coordinator.

A single oversized response can break a conversation permanently.
`commit_tool_responses` appends the response to the conversation stream and
calls `conv.flush()`, persisting it to disk.
Every subsequent turn re-sends that response and fails with the same `prompt is
too long` error from the provider.
The conversation remains unusable until the user strips the response with `jp
conversation compact --tools=strip-responses --over <size>` or edits the stream
by hand.

In one incident, a panicking `#[derive(Config)]` macro produced one diagnostic
per expansion site.
`cargo_test` embedded the full stderr output in its error message, producing a
1,293,623-token request against a 1,000,000-token context limit.

The Anthropic ACP flow fails at a lower threshold.
Claude Code replaces any tool result over 500,000 characters with a file
reference, which causes JP to abort the turn with a stream error ([RFD 110]).
Asking the model to continue re-issues the same tool call and hits the same
error.

Truncating output inside individual first-party tools (as the `cargo_*` tools
do) only covers built-in tools.
MCP servers, user-defined `local` tools, and responses synthesized by the
coordinator remain unbounded.

## Design

### Configuration

Tool configuration gains a `size_threshold` key, resolved per tool with fallback
to the `'*'` defaults, matching `run`, `result`, and `cancellation_response`:

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
If a tool truncates its own output below `size_threshold`, raising the threshold
cannot recover what the tool already discarded.
Accordingly, `cargo_expand`'s local `MAX_EXPANDED_BYTES` cap is raised or
removed, and first-party tools retain local limits only when those limits are
tighter than any threshold a user would configure.

Responses exceeding the threshold are truncated at a UTF-8 character boundary
and suffixed with a marker showing the original and truncated sizes via
`ByteSize::human()`:

```
... [truncated, 623.0 KB → 256.0 KB]
```

The threshold applies to the final string, including the marker (under 50
bytes): a response capped at `"256KB"` is at most 256 KB as persisted and
delivered.

Config validation rejects values below `"1KB"`, guaranteeing room for both the
marker and response content.
This also catches `size_threshold = 0`, which means "disabled" elsewhere in JP's
configuration but would reduce every response to a bare marker here.
The validation error directs users to `"unlimited"` to disable the limit, or
`result = "skip"` to omit the result entirely.

Finite values use `jp_config::types::byte_size::ByteSize` (also used by
compaction's `over = "1MB"` setting), which accepts a bare byte count or a
human-readable binary size where `"1MB"` is 1,048,576 bytes.
Because an unlimited compaction threshold is meaningless, `ByteSize` does not
support an `"unlimited"` variant.
Instead, tool configuration defines a local `SizeThreshold` enum (`Unlimited` or
`Bytes(ByteSize)`), resolved through `ToolConfigWithDefaults::size_threshold()`.

### What Is Bounded

The limit applies to `ToolCallResponse::content()`, the text string persisted to
the conversation stream and sent to the provider for both successful and failed
calls.
Because the conversation stream stores tool responses as text, images, audio,
and `structured_content` never enter the stream and are not measured against the
limit.

An MCP tool result can carry those non-text payloads.
If the text is within the limit, the full MCP result is delivered unchanged.
If the text is truncated, the result is rebuilt from the truncated text, like
any other Host edit ([RFD 109]), which drops its non-text blocks along with the
truncated tail.

### Enforcement Seams

Truncation runs at two seams in the tool execution pipeline:

| Seam                                                                                  | Scope                                   | Configurable |
| ------------------------------------------------------------------------------------- | --------------------------------------- | ------------ |
| `ToolCoordinator::execute_with_prompting`, as it assembles `ExecutionResult::reviews` | per-tool `size_threshold`, per response | yes          |
| `commit_tool_responses`, before the reviews are recorded                              | ceiling, whole batch                    | no           |

Both seams operate on a `Review` rather than a bare `ToolCallResponse`.
A `Review` holds the final response for a tool call and records whether the Host
modified the original content.
Truncating a response produces `Review::replaced`, so the rest of the pipeline
treats truncation as a Host edit.

The coordinator seam runs after `ResultMode` handling completes.
Every completion path (unattended execution, a result prompt under `Ask` or
`Edit`, a lost call, a failed inquiry, or a cancellation) writes its outcome
into the phase's per-call review slots.
When the phase ends, the coordinator collects those slots into
`ExecutionResult::reviews` in a single pass.
Calling `cap_review` there covers every path, including when a user pastes 5 MB
into the editor under `Edit`.
At this point the coordinator has each call's tool name to resolve
`size_threshold`, and `MockExecutor` exercises the same code path in turn-loop
integration tests.

Because truncation happens at the end of the phase, both `render_result` and the
edit prompt receive the full untruncated response.
The terminal's full-content temp file and the user's editor retain the trailing
output that is omitted from the assistant's context.

The `commit_tool_responses` seam is the final guard before persistence.
It bounds responses that bypass the per-tool cap: tools configured with
`size_threshold = "unlimited"`, MCP servers returning unbounded output under
that setting, and synthetic responses generated by the coordinator (unavailable
tool, orphan synthesis, or inquiry failure) that have no tool configuration.
This ceiling runs over the merged, index-ordered reviews, including pre-resolved
reviews, before their responses are written to the stream.

This second limit is a byte budget across the entire batch
(`MAX_TOOL_RESPONSE_BATCH_BYTES`, 2 MB), not a per-response cap.
Because `commit_tool_responses` persists as many responses as the model
requested in a cycle, a per-response ceiling would still leave total output
unbounded: two 2 MB responses already exceed the context window from the
reported failure.
The batch budget is enforced by truncating the largest responses first so a
single oversized response does not starve smaller ones.
Truncation markers count toward the 2 MB budget.

Because the terminal renders the untruncated output, the user needs a visible
indicator when the assistant receives less.
Whenever either limit truncates a response, JP prints a single notice line to
stderr alongside the tool call output, naming the tool, the original and
truncated sizes, and the limit that triggered:

```
fs_list_files: sent 256.0 KB of 3.1 MB to the assistant (size_threshold)
```

Truncation is also logged at `warn` level for trace files, but stderr output is
required because JP hides `warn` logs unless `-v` is passed.

### Keeping Persisted and Delivered Results Identical

`commit_tool_responses` appends `review.response` to the stream, flushes the
conversation, and then acknowledges each review to the execution service ([RFD
109]), which releases the MCP result to the caller.
For tool calls initiated by JP, the caller is JP's executor; for calls initiated
by an external agent (such as Claude Code in the Anthropic ACP flow), the caller
is that agent.

A modified review reaches the caller only if the call is parked on a barrier
that accepts replacement content.
Calls under `result = "ask"` or `"edit"` park on the Review barrier, which
already accepts replacement content.
Calls under the default `result = "unattended"` skip the Review barrier and park
on the Record barrier instead, whose reply carries no content: the execution
service waits for the Host to record the result and then delivers the original
result unchanged.
Truncating only the persisted text would record the capped response in the
stream while delivering the full payload to the caller.
For JP-submitted calls, the executor would report a delivery mismatch; on the
ACP flow, Claude Code would receive the multi-megabyte result intact and replace
it with an unreadable file reference.

To keep the persisted and delivered text identical, this RFD extends the Record
barrier reply to accept optional replacement content.
When a review is marked as replaced, the Host rebuilds the MCP result from the
persisted text, just as it does at the Review barrier, and sends it in the
Record barrier reply.
The execution service then delivers the replacement regardless of which barrier
the call parked on.

### Delivery Limit vs. Display Limit

`style.inline_results` only controls how many lines of a tool result are
rendered in the terminal.
Its documentation states that "the full tool call results will be sent back to
the assistant, regardless of this setting."
`size_threshold` controls what is persisted and sent to the assistant.
The two settings remain separate so a user can set `inline_results = 20` for
compact terminal output while still sending responses up to `size_threshold` to
the model.
The `inline_results` documentation is updated to note that full results are sent
up to `size_threshold`.

### Bytes, Not Tokens

Provider context windows are measured in tokens, but exact token counting
requires provider-specific tokenizers for a safeguard that only needs an
approximate bound.
Byte length is deterministic, free to compute, and provider-agnostic, and for
prose and source code it tracks token count within roughly a factor of two.

The draft attachment size policy (`large-attachment-size-policy`) uses the same
byte-based approach and shares the `size_threshold` key name and `ByteSize`
format.
Tool responses and attachments pose the same risk (external content of unknown
size entering the conversation context) and therefore share one configuration
vocabulary, matching the existing `ByteSize` syntax used by compaction
thresholds.

## Drawbacks

- **Truncating structured data produces invalid syntax.** Truncating a JSON or
  XML payload mid-document yields malformed output that the assistant may fail
  to parse, even though the truncation marker explains why the payload ended
  early.
- **Prefix truncation discards trailing summaries.** Test runners and build logs
  usually print the failure summary at the end, which prefix-only truncation
  drops.
- **Two separate limits to understand.** A user who raises `size_threshold` (or
  sets `"unlimited"`) can still hit the 2 MB batch ceiling.
  The stderr notice identifies which limit fired, but the user still has to
  account for both.
- **Batch budgeting makes truncation depend on sibling calls.** A tool call
  returning the same output may pass untouched when called alone and be
  truncated when called alongside other large tools in the same cycle.
- **Byte size only approximates token count.** A response below the byte
  threshold can still overflow a small context window, and dense encodings such
  as base64 consume far more tokens per byte than prose.
- **Per-cycle limits do not bound total conversation size.** Both limits cap
  only what a single cycle adds.
  Multi-cycle history can still exceed the context window; fitting the full
  conversation to the context window is the job of automatic compaction.
- **Truncated MCP results lose non-text content.** When an MCP result's text is
  truncated, rebuilding the result from text drops any attached images, audio,
  or `structured_content`.

## Alternatives

- **Truncate inside each tool only.** The `cargo_*` tools already truncate their
  own output, which gives the best quality because each tool knows which part of
  its output matters most.
  However, per-tool logic cannot cover external MCP servers or user-defined
  `local` tools.
  In-tool truncation remains useful as a complement, not a substitute.
- **Truncate in the execution service (`jp_mcp::server`).** Every executed tool
  call passes through `deliver_result`, which already reads the tool's
  configuration and determines the delivered result.
  Truncating there would keep the recorded and delivered values identical
  without changing the Record barrier.
  This was rejected for three reasons: the Host builds its display output from
  the service's recorded result, so the terminal would lose the truncated tail
  too; the service processes one call at a time and never sees
  coordinator-synthesized responses, so the batch ceiling would still require
  the Record barrier extension; and `MockExecutor` bypasses the service, leaving
  the configurable cap untested in turn-loop integration tests.
- **Token-based limits.** Counting tokens matches the provider constraint
  directly, but requires bundling provider-specific tokenizers for a limit that
  only needs to be approximate (see "Bytes, Not Tokens" above).
- **A `size_policy` with an `ask` variant.** Mirroring the attachment size
  policy by prompting the user mid-turn to approve an 800 KB tool response is
  disruptive, and `ResultMode::Ask` already supports reviewing tool results
  before delivery.
- **Fixed ceiling only, without per-tool configuration.** A non-configurable
  ceiling alone would prevent the 1.3M-token failure, but useful size limits
  vary by an order of magnitude across tools: most tools need only tens of
  kilobytes, whereas `cargo_expand` legitimately returns megabytes.

## Non-Goals

- **Spilling full output to a file the assistant can read.** Returning a file
  path that the assistant can inspect with `fs_read_file` is a natural
  follow-up, but requires a durable, content-addressed store ([RFD 066]) rather
  than the ephemeral temp files used for terminal OSC 8 links.
- **Head-and-tail or structure-aware truncation.** This RFD implements prefix
  truncation only; more sophisticated truncation strategies can be added later
  if needed.
- **`size_policy` variants (`ask`, `reject`, `allow`).** Deferred; if policy
  variants are added later, they will build on the same `size_threshold`
  setting.
- **Bounding tool request arguments, attachments, or assistant messages.** This
  RFD bounds tool call responses only.
- **Bounding non-text MCP payloads.** Images, audio, and `structured_content`
  are not measured against `size_threshold`; an untruncated response delivers
  any non-text blocks it carries unchanged.
  Sizing non-text payloads belongs with typed content blocks ([RFD 058]).
- **Unifying truncation markers with `.config/jp/tools`.** The
  `.config/jp/tools` crate is repository maintenance tooling rather than part of
  JP itself, so its internal truncation markers are unchanged.
  Phase 5 updates only the byte limit in `cargo_expand`.

## Risks and Open Questions

- **Default `size_threshold` value.** 256 KB (roughly 64k tokens) is large
  enough that existing first-party tools stay below it, while still preventing
  the 1.3M-token failure.
  A smaller 64 KB default would catch more wasted tokens, but would immediately
  truncate `cargo_expand` and large `git_diff_commit` outputs.
  Starting at 256 KB avoids breaking existing workflows.
- **Sizing the 2 MB batch ceiling.** The 2 MB ceiling (roughly 500k tokens) is a
  safeguard against runaway output, not a guarantee that the provider request
  fits the model's context window.
  Smaller models (such as Cerebras models with a 131,072-token window) can still
  overflow, and no per-cycle byte limit can prevent overflow once multi-turn
  history accumulates.
  The 2 MB value sits high enough to accommodate raised per-tool thresholds
  while still blocking multi-megabyte failures.
- **Interaction with the ACP character limit.** Claude Code replaces tool
  results exceeding 500,000 characters with a file reference, causing JP to
  abort the turn ([RFD 110]).
  The 256 KB default stays below that threshold, but raising `size_threshold`
  above 500 KB or setting `"unlimited"` re-exposes the failure.
  Enforcing a provider-supplied per-response ceiling (via
  `Provider::mcp_tool_metadata`, which already carries the ACP limit) alongside
  `size_threshold` would close this gap.
- **Batch truncation strategy.** Truncating the largest responses first until
  the batch fits under `MAX_TOOL_RESPONSE_BATCH_BYTES` is simple, but
  proportional or equal allocation across oversized responses may work better in
  practice.
  This can be tuned against real multi-tool workloads.
- **Existing conversations with oversized responses.** Bounding new responses
  does not repair conversations that already contain an oversized response in
  their history.
  Users can strip existing oversized responses from provider requests using `jp
  conversation compact --tools=strip-responses --over <size>` (which leaves
  stored events intact) or by editing the conversation stream directly.
- **Notice visibility under `--quiet`.** Suppressing the stderr notice under
  `--quiet` would hide the only indication that the assistant received truncated
  output.
  Ticket T-0ez6tx1 tracks the same question for abnormal turn endings; the
  truncation notice will follow the convention established there.
- **Stability of the truncation marker format.** Once the truncation marker
  appears in persisted responses, models and user scripts may match on its exact
  text.
  The marker format, including the `ByteSize::human()` representation, should be
  treated as stable after release.

## Implementation Plan

**Phase 1: Config types.** Add `SizeThreshold` (`"unlimited"` or a `ByteSize`),
add `size_threshold` to `ToolsDefaultsConfig` and `ToolConfig` (implementing
`AssignKeyValue`, `PartialConfigDelta`, `FillDefaults`, and `ToPartial`), and
add a resolver method on `ToolConfigWithDefaults`.
Add a `Validator` that rejects values below `"1KB"`, including a test verifying
that `0` fails with a hint pointing to `"unlimited"`.
This phase introduces no runtime behavior change and can merge independently.

**Phase 2: Replacement at the Record barrier.** Extend the Record barrier reply
to carry optional replacement content, update the execution service to deliver
that content when present, and update the Host to populate it whenever a review
is replaced.
Add a service test verifying that the MCP caller receives the replacement
content from the Record barrier, and an executor test verifying that an
unattended call acknowledged with a replaced review completes without a delivery
mismatch.
This phase can merge independently; phases 3 and 4 depend on it.

**Phase 3: Configurable per-tool cap.** Implement `cap_review` and call it when
`execute_with_prompting` assembles `ExecutionResult::reviews`.
Add two turn-loop integration tests using `MockExecutor`: one returning an
oversized result under `ResultMode::Unattended`, and one editing an oversized
result under `ResultMode::Edit`.
Both tests assert that the persisted response is truncated within the threshold
(including the marker) and that the expected notice line is printed.
Depends on phases 1 and 2.

**Phase 4: Batch ceiling.** Enforce `MAX_TOOL_RESPONSE_BATCH_BYTES` in
`commit_tool_responses`, truncating the largest responses first across the
merged reviews, marking truncated entries as replaced, and printing a notice for
each truncated tool.
Add two tests: one with a single response exceeding the batch budget under
`size_threshold = "unlimited"`, and one with multiple responses that
individually pass `size_threshold` but together exceed the batch budget.
Both tests assert that the total batch size including markers stays within the
budget and verify the printed notice lines.
Depends on phase 2 and follows phase 3.

**Phase 5: Raise tool-local caps.** Remove or raise `MAX_EXPANDED_BYTES` in
`cargo_expand` so `size_threshold` becomes the active limit for that tool.
Depends on phase 3.

**Phase 6: Documentation.** Update the `InlineResults` doc comment to reflect
`size_threshold`, and document `size_threshold` in `docs/configuration.md`.

## References

- [RFD 058]: Typed content blocks, where non-text payload limits belong.
- [RFD 066]: Content-addressable blob store for persisting and retrieving full
  tool output.
- [RFD 109]: In-process MCP execution service and the Host barriers extended by
  this RFD.
- [RFD 110]: Anthropic ACP flow and its 500,000-character inline tool result
  limit.
- Draft RFD `large-attachment-size-policy`: Attachment size policy that shares
  the `size_threshold` vocabulary.

[RFD 058]: 058-typed-content-blocks-for-tool-responses.md
[RFD 066]: 066-content-addressable-blob-store.md
[RFD 109]: 109-in-process-jp-mcp-server.md
[RFD 110]: 110-anthropic-subscription-queries-via-acp.md
