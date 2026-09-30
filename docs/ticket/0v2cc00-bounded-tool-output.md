# Bounded Tool Output

- **Status**: Todo
- **Kind**: Feature
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-09-30
- **Implements**: 115
- **Label**: type=tracking

Tracking ticket for [RFD 115].

## Implementation plan

- **Add config types for size_threshold**: Introduces `SizeThreshold`
  (`"unlimited"` or `ByteSize`), adds `size_threshold` to `ToolsDefaultsConfig`
  and `ToolConfig` with the standard config trait impls, a resolver on
  `ToolConfigWithDefaults`, and a validator rejecting values below `"1KB"`.
  Pure config change with no behavior impact; mergeable on its own.
- **Let the Record barrier carry replacement content**: Extends the Record
  barrier's reply to carry replacement content, has the execution service
  deliver it, and has the Host send it when a review is replaced.
  Covered by a service test (replacement delivered to the MCP caller) and an
  executor test (unattended call with a replaced review completes without a
  delivery mismatch); mergeable alone and a dependency for phases 3 and 4.
- **Apply the configurable per-tool cap**: Adds `cap_review` and applies it
  where `execute_with_prompting` assembles `ExecutionResult::reviews`.
  Verified by two turn-loop integration tests (oversized result under
  `Unattended` and under `Edit`) asserting the persisted response is capped
  within threshold and the correct notice is printed; depends on phases 1 and 2.
- **Enforce the batch-wide ceiling**: Adds `MAX_TOOL_RESPONSE_BATCH_BYTES` in
  `commit_tool_responses`, truncating the largest responses first across merged
  reviews and emitting a notice per truncated tool.
  Tested with a single response exceeding the budget under `size_threshold =
  "unlimited"` and multiple individually-small responses that exceed the budget
  together; depends on phase 2.
- **Raise tool-local output caps**: Drops or raises `MAX_EXPANDED_BYTES` in
  `cargo_expand` so the new central `size_threshold` becomes the binding limit
  for that tool.
  Small change, only meaningful after phase 3 lands.
- **Document the new behavior**: Corrects the `InlineResults` doc comment that
  incorrectly promises full delivery to the assistant, and documents
  `size_threshold` in `docs/configuration.md`.

[RFD 115]: ../rfd/115-bounded-tool-output.md
