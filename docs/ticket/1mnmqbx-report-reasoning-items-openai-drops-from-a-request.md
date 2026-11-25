# Report reasoning items OpenAI drops from a request

- **Status**: Done
- **Authors**: jp
- **Date**: 2026-11-17
- **Label**: domain=llm
- **Label**: llm-provider=openai
- **Label**: package=jp_llm
- **Label**: relevant-rfd=105
- **Label**: type=enhancement

From GPT-6.2 on, the Responses API removes replayed reasoning items when it
detects an edit to the conversation before them, and reports each removal in the
response's `input_transformations` with a reason:

- `prefix_mismatch`: content before the item was inserted or edited (6.2+).
- `end_user_mismatch`: `safety_identifier` differs between the creating and the
  reusing request (6.2+).
  JP sends none, so this should never fire.
- `model_mismatch`: a switch to another model family, or a downgrade.
- `organization_mismatch`: the item comes from another organization.
- `turn_boundary`: the model uses current-turn reasoning only.

JP does not read `input_transformations`, so a dropped item is invisible: the
turn costs more tokens and may lose quality, and nothing says why.

Source:
<https://developers.openai.com/api/docs/guides/reasoning#reasoning-item-removal-across-requests>
and <https://developers.openai.com/api/docs/guides/keep-prefix-unchanged>.

## Why first

JP edits the prefix in several ways (mid-conversation config deltas that change
`instructions` or `tools`, attachments that change between requests, compaction
summaries placed before kept turns, a changed `reasoning.effort`).
How often each one costs reasoning in practice decides how to order the
append-only work under RFD 105.
This ticket produces that measurement.

## Scope

- Parse `input_transformations` from the response.
  The `openai_responses` fork may not model the field yet; check before
  assuming.
- Log each removal with its reason and item id.
- Surface `prefix_mismatch` and `model_mismatch` to the user as a notice, since
  both follow from something the user or JP changed.

## Not in scope

Avoiding the edits that cause removals.
That needs a shared design across providers (RFD 105, and T-0q1k0j4 on the
Anthropic side).

## Comments

-----

- **From**: jp
- **Date**: 2026-11-17T21:55:19Z

Blocked on the `openai_responses` fork (JeanMertz/openai-responses-rs at
`db70ddd`).

Its `Client::stream` and `Client::create` deserialize every SSE event and
response body into the fork's own `types::Response`.
That type has no `input_transformations` field, and serde drops unknown fields,
so JP never sees the data.
There is no hook for the raw body.

The fork change:

```rust
// src/types/response.rs, on `Response`
/// Input items the API withheld from the model, with the reason.
#[serde(default)]
pub input_transformations: Vec<InputTransformation>,

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InputTransformation {
    /// The id of the input item, e.g. a reasoning item's `rs_…` id.
    pub id: String,
    /// `removed`.
    pub action: String,
    /// `prefix_mismatch`, `end_user_mismatch`, `model_mismatch`,
    /// `organization_mismatch`, or `turn_boundary`.
    pub reason: String,
    /// `reasoning`.
    pub r#type: String,
}
```

The shape comes from the 200 example on
<https://developers.openai.com/api/reference/cli/resources/responses/methods/create>.
Strings rather than enums, so a reason added later does not fail
deserialization.

The JP side then reads it in `map_event`'s `ResponseCompleted |
ResponseIncomplete | ResponseFailed` arm, which already handles the final
`Response`, and in `map_non_streaming_response`.
It logs each removal and emits an `Event::Notice` for `prefix_mismatch` and
`model_mismatch`.

-----

- **From**: jp
- **Date**: 2026-11-17T22:32:27Z
- **Re**: #1

Done against the fork at `d89991f`.
`input_transformation_notices` in `provider/openai/input_transformations.rs`:

- logs every withheld item at `debug` with its id, action, reason, and type;
- emits one `Event::Notice` per reason for removed reasoning with
  `prefix_mismatch` or `model_mismatch`, giving the count, ahead of the
  `Finished` event;
- leaves `turn_boundary`, `end_user_mismatch`, and `organization_mismatch`
  log-only.

Both transports report: the streaming terminal event in `map_event`, and
`map_non_streaming_response`.

Known trade-off: the API reports a removal on every request whose replayed
history still carries the edit, so after one mid-conversation change the notice
repeats on every request (every tool round included) for the rest of the
conversation.
That is accurate (each request loses the reasoning again), but noisy.
Deduplicating by item id would need state on the provider.

-----

- **From**: jp
- **Date**: 2026-11-25T15:26:58Z
- **Re**: #2

The repetition described as a known trade-off above is gone: a removal is
reported once per provider.
A query builds one provider per turn, so a turn reports each removed item once,
on the first request that loses it, and the next turn reports again.

The state the earlier comment said dedup would need lives on the provider as
`Openai::seen_notices`.
`SeenNotices` is in `provider/notices.rs` and is shared with Anthropic, which
already used it for skipped credentials and quota warnings.

`input_transformation_notices` takes the set and keys each removal on its item
id rather than on the notice text.
A later request in the same turn that loses more reasoning reports only the
items no earlier request reported, so the count in the notice is the number of
newly removed items, not the running total.
A reason whose removals have all been reported emits no notice.

Unchanged: every withheld item is still logged at `debug` on every request, so
the per-request measurement this ticket was filed for is still available from
the logs.
`turn_boundary`, `end_user_mismatch`, and `organization_mismatch` stay log-only.

OpenAI's `skipping <credential>` notices go through the same set, since they
repeated on every request for the same reason.
Credential switches and reset redemptions are events and are still reported each
time.

Tests are in `provider/openai/notice_tests.rs`.
