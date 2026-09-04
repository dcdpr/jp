# The Cerebras model table describes an archived model

- **Status**: Todo
- **Kind**: Chore
- **Authors**: jp
- **Date**: 2026-09-04

`map_model` in `crates/jp_llm/src/provider/cerebras.rs` carries a hand-written
entry for `zai-glm-4.7` — display name, context window, output limit, reasoning
ladder.
The API reports the model as gone:

```json
{"message":"Model zai-glm-4.7 is archived and unavailable for the organization.",
 "type":"model_archived_error","param":"model","code":"model_archived"}
```

Nothing user-facing is wrong today.
`Provider::models` lists from the authenticated `/v1/models` endpoint and the
table only supplies metadata for ids that endpoint returns, so an archived model
is never offered.

Deliberately not deleted.
The message says archived *for the organization*, which leaves open that other
organizations still have access, and the entry exists because someone did.
Deleting it would degrade those users to `ModelDetails::empty` and a warning.
Filed so the next reader does not re-derive the question.

## The general version is worth more

The table is hand-maintained against a catalog that moves, and
`fetch_public_catalog` already reconciles most of it: context window, output
limit, structured-output support, and deprecation all come from the public
catalog when it is reachable.
The one thing the catalog does not report is the reasoning effort ladder.

So the question behind this ticket is whether the built-in table should shrink
to just the ladder, with everything else deferred to the catalog.
That trades a stale-metadata class of bug for a dependency on an unauthenticated
endpoint being up — `fetch_public_catalog` is best-effort by design and falls
back to the table today, which is the fallback that would disappear.
