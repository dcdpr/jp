# Anthropic ACP qualification

The ACP subscription flow accepts `claude-agent-acp` 0.76.0 with Claude Code
2.1.257.
Model availability is decided by the runtime, not a JP allowlist.
The cache qualification fixture uses `claude-opus-5`; that choice does not
restrict ordinary queries.
The setup instructions are in [Providers].
Qualification uses the production provider entry point, not a separate CLI
wrapper.
Automated protocol fixtures need no runtime, credentials, or quota.
They do not prove live cache hits or subscription allowance savings.

## Usage accounting

Provider event metadata contains `anthropic_acp_usage`.
Each value is a **snapshot**, identified by `native_session_id`, not an
increment.
Keep the latest snapshot for each native session; do not sum snapshots from
different content blocks or tool calls in that session.

- `requests` contains completed main-session model responses, keyed by native
  message ID.
  Repeated SDK observations update that entry.
  Uncached `input_tokens`, `cache_creation_input_tokens`,
  `cache_read_input_tokens`, and `output_tokens` remain separate.
  Missing or null counters mean unreported, not zero.
- `cache_creation`, when supplied, separates five-minute and one-hour writes.
- `runtime.usage` and `runtime.model_usage` retain SDK aggregate snapshots.
  These overlap the main requests and can include runtime helper activity.
  Do not add them to the request counters or infer that every model in the
  aggregate answered the user's request.
- `runtime.estimated_cost_usd` is the SDK's list-price estimate, not a bill or a
  subscription quota measurement.

Text streams as it arrives.
The last content block's commit waits for the usage notification; structured
answers and tool-only responses also carry snapshots.
Replay and subagent messages do not enter the main-request accounting.
The transport logs its available snapshot at debug level when the connection
finishes, including for auxiliary consumers that discard event metadata.
A cancelled or failed request may never receive final usage from the runtime.

## Controlled live comparison

This is an ignored test because it spends subscription allowance.
Confirm paid Usage credits are disabled in Claude's Settings > Usage before
enabling it.
The environment flag is the operator's confirmation, not a billing setting.
No API-key fallback is configured.

From the repository root, with the pinned runtime on PATH:

```sh
claude-agent-acp --version
claude-agent-acp --cli --version
claude-agent-acp --cli auth status --json

env JP_ACP_LIVE_NO_OVERAGE=1 JP_ACP_CACHE_RUN=qualification-001 \
  cargo test -p jp_llm live_cache_reconstruction -- --ignored --nocapture
```

Use a fresh `JP_ACP_CACHE_RUN` value for each run so an earlier test does not
warm the initial prefix.
The test uses its own JP configuration, so workspace inquiry-model overrides do
not affect it.
It runs without tools and uses a synthetic invoice history with a long reference
prefix, a 128-token output limit, and a two-minute timeout per request.
It stops at the first failure.

The cases are:

| Case                  | What it checks                                                         |
| --------------------- | ---------------------------------------------------------------------- |
| Initial `short`       | A five-minute cache write, with no one-hour write.                     |
| Reconstructed `short` | An identical Thread in a different native session reads cached tokens. |
| `long`                | A separate prefix writes a one-hour entry, with no five-minute write.  |
| `off`                 | The initial prefix produces neither cache reads nor writes.            |

Each successful request must also answer `INV-1042`.
Reports are printed before the cache assertions, so a failure retains the
counters that caused it.
Missing TTL counters fail qualification rather than being interpreted as zero.
Retain the reports and runtime versions; do not commit account-identifying auth
output.

The repeated-short case exercises process restart and native-history
reconstruction through JP.
It proves useful reuse, **not equivalent efficiency to continuing an existing
Claude Code session**.
To measure that separately, capture the native state immediately before the
comparison prompt, run that prompt through normal native continuation, then run
the corresponding JP Thread through reconstruction.
Hold the model, working directory, tool definitions, instructions, and content
constant; account for runtime-added context and run within the cache TTL.
The long-cache case deliberately uses another prefix and is not an efficiency
comparison with the short-cache cases.

The existing protocol and transcript fixtures cover replay exclusion, tool ID
stability, repeated SDK message IDs, runtime helper totals, configuration
mapping, and unchanged model-visible history across native bookkeeping changes.
The live test does not qualify arbitrary tool inventories or tool-result sizes.

## Subscription observations

Record plan usage immediately before and after a controlled run, with other
account activity paused and no quota reset crossing the run.
Record the active model and cache policy alongside those observations.
The plan's usage display can be delayed or coarse; report that uncertainty
rather than derive a precise multiplier from a small change.

More cache hits for the same work generally conserve input-processing expense,
but cache hits still occupy the context window.
API pricing ratios and SDK dollar estimates are not a published formula for
subscription-window percentages.
Neither latency benchmarks nor credential-switch experiments are required by
this procedure.

[Providers]: ../README/providers.md#anthropic-subscription-flows
