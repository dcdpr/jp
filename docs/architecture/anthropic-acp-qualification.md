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

The ACP transport emits a debug tracing event with a JSON `usage` field.
It is diagnostic data, not conversation metadata.
Each value is a snapshot identified by `native_session_id`, not an increment.
The live test captures this tracing event with a test-only subscriber.

- `requests` contains observed main-session model usage, keyed by native message
  ID.
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

Content is emitted and committed independently of usage reporting.
Replay and subagent messages do not enter the main-request accounting.
The transport logs its available snapshot when the connection finishes.
A cancelled or failed request may never receive final usage from the runtime.

## Controlled live comparison

`cache_reconstruction` replays `crates/jp_llm/tests/fixtures/acp/live.jsonl` by
default: no runtime is spawned and no allowance is spent, so it runs on every
commit like any other test.
A missing recording fails it rather than skipping it.

`RECORD=1` reaches the installed runtime instead, adds the cache measurements
only a live service can answer, and writes the recording back.
That run spends subscription allowance, so confirm paid Usage credits are
disabled in Claude's Settings > Usage first — nothing in the test can check
that for you.
No API-key fallback is configured.

From the repository root, with the pinned runtime on PATH:

```sh
RECORD=1 cargo test -p jp_llm cache_reconstruction -- --nocapture
cargo insta accept
```

The test checks the adapter and runtime versions and the active login itself,
before it spends anything.
Capture them separately when a report needs them:

```sh
claude-agent-acp --version
claude-agent-acp --cli --version
claude-agent-acp --cli auth status --json
```

Each run tags its system prompt with a fresh identifier, so an earlier run
cannot warm the initial prefix.
The test uses its own JP configuration, so workspace inquiry-model overrides do
not affect it.
It runs without tools and uses a synthetic invoice history with a long reference
prefix, a 128-token output limit, and a two-minute timeout per request.
It stops at the first failure.

The cases are:

| Case                          | What it checks                                                         |
| ----------------------------- | ---------------------------------------------------------------------- |
| Initial runtime-managed       | Claude Code creates a cache entry using its own retention policy.      |
| Reconstructed runtime-managed | An identical Thread in a different native session reads cached tokens. |
| `off`                         | The initial prefix produces neither cache reads nor writes.            |

Each successful request must also answer `INV-1042`.
Reports are printed before the cache assertions, so a failure retains the
counters that caused it.
TTL counters are retained when reported, but their values do not determine
whether the runtime-managed case passes.
Retain the reports and runtime versions; do not commit account-identifying auth
output.

The repeated runtime-managed case exercises process restart and native-history
reconstruction through JP.
It proves useful reuse, **not equivalent efficiency to continuing an existing
Claude Code session**.
To measure that separately, capture the native state immediately before the
comparison prompt, run that prompt through normal native continuation, then run
the corresponding JP Thread through reconstruction.
Hold the model, working directory, tool definitions, instructions, and content
constant; account for runtime-added context and run within the cache TTL.
JP does not enforce `short`, `long`, or custom retention durations in this flow.
Only `off` changes the runtime's caching policy.

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
