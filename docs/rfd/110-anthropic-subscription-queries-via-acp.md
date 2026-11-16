# RFD 110: Anthropic Subscription Queries via ACP

- **Status**: Accepted
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-09-11
- **Extends**: [RFD 090]
- **Summary**: Routes Anthropic subscription queries through `claude-agent-acp`
  and the official Claude Code runtime by default.

## Summary

The `anthropic` provider adds an Agent Client Protocol (ACP) subscription flow
backed by `claude-agent-acp` and the unmodified Claude Code runtime.
ACP becomes the default for subscription authentication, while the existing
direct HTTP flow remains available through explicit configuration.
Model IDs, the `--auth` interface, JP conversation ownership, and JP tool
policies are unchanged.

## Motivation

[RFD 090] and [PR 1151] through [PR 1153] introduced subscription credentials,
provider-owned credential selection and fallback, and the `--auth` flag.
That direct Anthropic subscription flow sends HTTP requests from JP using Claude
Code OAuth credentials stored in JP.
While functional, extracting and replaying those credentials outside Claude Code
is not sanctioned by Anthropic and risks account restriction.

Anthropic's June 15, 2026 [subscription clarification] confirms that usage
through the Agent SDK, `claude -p`, and third-party applications continues to
draw from subscription allowances rather than requiring per-token API billing.
Driving the official Claude Code runtime through `claude-agent-acp` uses this
supported path without extracting credentials or reimplementing authentication.

The probe harness in `.config/jp/experiments/` verified that JP can reconstruct
role-bearing conversation history in Claude Code's native format, execute tools
through the hosted MCP server from [RFD 109], and control runtime behavior.
Because ACP is a transport mechanism for Anthropic subscription queries rather
than a distinct model vendor, it belongs inside the `anthropic` provider as a
subscription flow rather than as a separate public `acp` provider.

## Design

### User interface and setup

The `--auth` flag selects the billing mode without changing the provider or
model ID:

```sh
jp q -n --auth sub -m anthropic/claude-opus-5 "Review this change."
jp q -n --auth api -m anthropic/claude-opus-5 "Review this change."
```

The ACP subscription flow requires Node.js 22 or later and npm.
Install and authenticate the qualified adapter release:

```sh
npm install --global @agentclientprotocol/claude-agent-acp@0.76.0
claude-agent-acp --cli auth login --claudeai
claude-agent-acp --cli --version
claude-agent-acp --cli auth status --json
```

Keep npm's optional dependencies enabled so the SDK installs Claude Code's
native binary on supported platforms without a separate Claude Code
installation.
Both `node` and `claude-agent-acp` must be on JP's `PATH`.
Compatibility is qualified against `claude-agent-acp` 0.76.0 and Claude Code
2.1.257; newer releases are not guaranteed to work without qualification.

Authentication uses Claude Code's browser flow and native credential store
without copying tokens into JP.
Before running a query, JP verifies that subscription authentication is active;
ambient API keys, credential helpers, or cloud-provider settings must not
silently switch billing sources, and child-process environment overrides must
not leak into JP's parent process or API flow.
To restrict usage to the included subscription allowance, disable paid **Usage
credits** in Claude's **Settings > Usage**.

Passing `--auth sub` on the command line uses the ACP flow without any
configuration file changes.
To make subscription authentication the workspace default, configure
`providers.llm.anthropic` in `.jp/config.toml`:

```toml
[providers.llm.anthropic]
auth = ["subscription"]
subscription_flow = "acp"

[assistant.model]
id = "anthropic/claude-opus-5"
```

`subscription_flow` defaults to `acp` and can be omitted.
Once configured, ordinary queries use the ACP flow:

```sh
jp query --new "Review the changes in this workspace."
jp query "Focus on error handling."
```

For each query, JP starts the adapter and the hosted MCP server, prepares the
conversation history, coordinates tool execution, and records the response.
Users do not run a background daemon, duplicate tool definitions in Claude Code,
or manage external session IDs; assistant settings, instructions, attachments,
and tools remain configured in JP.

### Flow selection and migration

`subscription_flow` selects the transport implementation for `subscription`
authentication entries without changing the billing kind, model, or service
tier.
The field accepts `acp` (default) or `direct`, rejects unknown values, and
participates in standard scalar config layering and conversation deltas.

| Authentication         | `subscription_flow` | Implementation                                                        |
| ---------------------- | ------------------- | --------------------------------------------------------------------- |
| `api_key` / `api`      | Either value        | Existing JP Anthropic API implementation.                             |
| `subscription` / `sub` | `acp` (default)     | Claude Code through the qualified ACP adapter.                        |
| `subscription` / `sub` | `direct`            | Existing JP-stored subscription credentials and direct HTTP requests. |

To retain the direct subscription flow:

```toml
[providers.llm.anthropic]
subscription_flow = "direct"
```

Setting `subscription_flow = "direct"` explicitly opts into the policy and
account risks of direct OAuth replay; JP never falls back to `direct`
automatically when ACP is missing, unsupported, or fails.
The default `auth` chain remains `["api_key"]`, so API-key users are unaffected
and do not need Node.js or Claude Code.

Existing subscription users who omit `subscription_flow` must either install and
authenticate the external runtime or set `subscription_flow = "direct"`.
JP does not delete existing JP-stored credentials or import them into Claude
Code.
Initial ACP support uses whichever subscription account is active in the Claude
Code runtime; if a configuration specifies a named JP subscription credential,
the ACP flow fails rather than silently using the runtime's active login.
Named credentials remain usable with `direct`, while profile mapping for native
logins and `jp provider llm auth` integration are deferred to follow-up work.

Existing credential-chain and `--auth` semantics remain authoritative.
Paid API fallback occurs only when the `auth` chain includes an `api_key` entry;
this feature never injects an API entry or switches automatically between `acp`
and `direct`.
When a subscription-only configuration exhausts its allowance, both primary and
auxiliary requests stop.
Because account-level paid usage credits are billed by Anthropic outside JP's
credential chain, users who want hard allowance caps must disable usage credits
in Claude's settings rather than rely on client-side cache or token cost
estimates.

### Provider and execution boundaries

```text
anthropic provider
  +-- API-key authentication --> existing API implementation
  +-- subscription/direct ----> existing direct subscription implementation
  +-- subscription/acp -------> internal Claude integration
                                  +-- ACP client and runtime lifecycle
                                  +-- Thread-to-native-transcript conversion
                                  +-- streamed events and notices
                                  +-- JP MCP execution host
```

The ACP integration lives inside the `anthropic` provider; `model.id` syntax is
unchanged, and JP does not expose a public `acp` provider, an arbitrary ACP
executable selector, or an unrestricted SDK-options bag.
While internal ACP transport code can be structured for reuse, the provider owns
the `claude-agent-acp` command invocation and version compatibility policy.

Credential selection remains in the provider ([PR 1151]), and ACP protocol
handling and transcript conversion stay inside the Claude integration rather
than in CLI command handlers.
Tool execution and the MCP execution-host interface are defined by [RFD 109] and
reuse JP's existing tool coordination and terminal rendering.
Because the external agent invokes MCP tools before its ACP prompt request
returns, the query runner must service the MCP host concurrently while the ACP
prompt is in flight; waiting for prompt completion before handling MCP requests
would deadlock.

A single ACP prompt drives the external agent's entire model-and-tool
continuation loop.
Internal turn orchestration must account for host-executed tools so that tool
calls observed on the ACP stream are not executed a second time by JP's standard
API tool-execution phase.
The internal provider and request interface for this execution contract is
settled in the first vertical slice without affecting public provider or
authentication configuration.

### JP remains the conversation authority

The ACP flow consumes the same `Thread` and provider-visible projection as the
direct Anthropic request builder, including the Compacted View.
Rather than flattening prior turns into a synthetic user-message memo, it
converts prior history into Claude-native transcript records that preserve
message roles, ordering, supported content blocks, and paired tool calls and
results.
It loads that transcript prefix through ACP and submits the pending input once
as the live prompt, never duplicating it in the loaded prefix.
Continuations without a new user request preserve the existing provider
continuation semantics without repeating an earlier request.

Claude-native transcript files are a derived provider representation.
The transcript encoder generates record bookkeeping locally from JP message
content without requiring a seed response from Claude Code.
Because Claude Code's storage format is version-specific, the encoder and
decoder require checked-in fixtures for qualified runtime versions.
Opaque reasoning metadata follows the existing Anthropic conversion rules rather
than fabricating signatures or treating reasoning blocks as interchangeable
across providers.

Because every request projects the current `Thread` through this converter,
provider switches, turn replays, selected-turn forks, compaction, and attachment
edits work without a manual handoff.
For example, switching to OpenAI for one turn and returning to Anthropic
automatically includes the OpenAI turn in the reconstructed transcript:

```sh
jp q -n --auth sub -m anthropic/claude-opus-5 "Review the design."
jp q --auth sub -m openai/gpt-6-astra "Check the assumptions."
jp q --auth sub -m anthropic/claude-opus-5 "Continue from that review."
```

A native Claude Code session can be reused only when its transcript and settings
still match the current `Thread` and resolved configuration; a saved session ID
alone is not sufficient.
When the `Thread` or configuration has diverged, JP writes a separate native
transcript file and loads it into a new session.
Disposable native transcript files are stored separately from immutable JP
conversation history, record-ID remapping preserves input content unmodified,
and JP never edits a user's unrelated Claude Code sessions.

When ACP loads a prior transcript, it replays historical events over the session
stream; JP treats load-time replay as historical observation rather than new
output or authorization to re-execute a historical tool call.
Only newly generated events are imported once into the JP conversation stream,
and conversation locking and durable writes remain JP's responsibility.
Independent requests, including auxiliary queries and side queries triggered by
tool callbacks, use distinct ACP sessions and must not block on a mutex held by
an active native session.

### Tools, output, and model support

The ACP flow registers JP's hosted MCP server under a stable tool namespace,
disables Claude Code's built-in side-effectful tools and unconfigured MCP
servers, and suppresses optional hooks, skills, and background features through
qualified runtime controls.
JP verifies the runtime's active tool surface at startup.
These controls constrain the agent's configuration rather than acting as an OS
sandbox, and any native context injected by the runtime must be accounted for
separately rather than mistaken for JP attachments.

The hosted MCP server executes tools through JP's standard policies: enablement,
user approval, argument editing, tool options, access checks, inquiries, result
editing, and event recording.
ACP tool-call updates are treated as observations only.
JP correlates each MCP tool invocation with its ACP tool-call ID using
`_meta["claudecode/toolUseId"]` on the MCP call request rather than relying on
request numbers or matching arguments, and it preserves the distinction between
requested arguments and user-edited execution arguments.

Sequential tool dispatch by the external runtime is acceptable as long as
call-to-result pairing, JP approval behavior, and session isolation are
preserved.
Forced tool selection retains JP's existing best-effort semantics.
Cancellation propagates to pending user inquiries and running tools, and a
disconnection after a tool may have caused a side effect fails the turn rather
than repeating the call automatically.

Supported models and request controls come from `ModelDetails.subscription` and
the existing model capability fields for the selected flow.
The provider resolves canonical model IDs against qualified model metadata
without silently substituting a different model, records the actual response
model in metadata, and does not run ACP discovery for API-only requests.
Explicitly requested controls that the flow does not support use JP's existing
capability error handling rather than being silently dropped, while
HTTP-specific transport settings remain scoped to the HTTP implementations.

Each request applies the resolved system prompt and structured-output schema.
Custom prompts use `snapshot: false` so native prompt snapshots do not suppress
subsequent JP configuration changes.
Streamed text and thinking blocks use JP's existing rendering, and structured
results arrive through the adapter's raw SDK result extension to become JP
structured responses.
A model refusal maps to `FinishReason::Refused` (including its refusal category
when supplied), and an SDK result with `subtype: success` and `is_error: true`
is treated as an error rather than a success.

### Large tool results

By default, Claude Code replaces a large MCP tool result with a file reference
before the next model request.
Because the ACP flow disables Claude Code's native file tools, the model cannot
read that substituted file even though JP still stores the full result in the
conversation stream.

To keep large results inline, JP applies both controls verified in the probe
experiments:

- `MAX_MCP_OUTPUT_TOKENS=100000` in the runtime environment.
- `_meta["anthropic/maxResultSizeChars"] = 500000` on each relevant JP tool's
  `tools/list` entry.

Together, these settings preserve a 240,052-byte text result across the
SDK-visible boundary and allow the model to answer from its footer, whereas the
environment variable alone fails the same test.
Integration tests must assert that the full text is preserved inline rather than
retrieved from a substituted file through an uncontrolled native tool.

The [documented size override] caps `_meta["anthropic/maxResultSizeChars"]` at
500,000 characters.
This is an inline-text threshold for a single tool invocation, not a limit on
JP's stored conversation, the full request, or the model's context window.
Single tool results above 500,000 characters, multiple large results in one
turn, or other runtime context-management policies can still trigger file
substitution.

Handling results outside the qualified range remains an open compatibility edge:
the implementation must either provide a provider-controlled continuation or
reconstruction mechanism that preserves them, or document the ceiling as an
explicit limitation before claiming parity.
Silent truncation, automatic fallback to `direct`, and bypassing JP tool
permissions are not acceptable solutions, and emitting a diagnostic avoids
silent data loss without making the oversized workflow succeed.

### Prompt caching and subscription usage

Prompt caching reuses an identical request prefix on the server rather than a
local session file.
As [Claude Code's cache documentation] explains, requests with matching prefixes
can share cache entries across sessions, so reconstructing a native transcript
preserves caching as long as the rendered prefix does not change.

The probe traces already demonstrate cache reads: an ordinary follow-up turn
reads 836 cached input tokens, and reconstructed-history requests each read
1,322 cached tokens using one-hour cache writes.
Because those traces used different prompts and models, they prove that
reconstructed transcripts can reuse a cached prefix, not that an arbitrary JP
`Thread` achieves the same cache efficiency as native session continuation.

To keep the prefix stable when the `Thread` is unchanged, the converter must
preserve tool names, tool definitions and ordering, message content, and
wire-visible tool-call IDs.
Transient session IDs, listener addresses, and per-request temporary working
directories must not appear in the model-visible prefix, though native
transcript file paths can vary as long as the agent's logical working directory
stays fixed.
Prefix stability must also account for runtime-injected environment and git
context, without sacrificing current instructions or accurate history to keep a
cache hit.

The ACP flow maps `assistant.request.cache` onto the runtime as follows:

| JP policy                             | ACP flow mapping                                                 |
| ------------------------------------- | ---------------------------------------------------------------- |
| `off`                                 | `DISABLE_PROMPT_CACHING=1`.                                      |
| `short`, `long`, or a custom duration | No override: Claude Code's own default and managed policy apply. |

The ACP flow does not enforce a cache retention duration; only `off` modifies
the runtime's caching policy, and it is the only override JP sends.
Before starting the adapter, the launcher strips inherited cache overrides
(`DISABLE_PROMPT_CACHING*`, `CLAUDE_CODE_PROMPT_CACHE_TTL`,
`FORCE_PROMPT_CACHING_5M`, `ENABLE_PROMPT_CACHING_1H`) from the child
environment so ambient variables cannot override JP's resolved policy.
The direct HTTP flows keep their existing duration mapping, and JP-initiated
auxiliary requests apply their own resolved policy.
Cache breakpoint placement need not be byte-identical between flows as long as
supported caching controls and unchanged-prefix reuse remain effective.

Usage accounting must record uncached input, cache creation, cache reads, and
output tokens separately, distinguishing per-request usage from cumulative
`modelUsage` snapshots and background runtime helper calls.
Switching models, accounts, or flows may change the cache scope, and
configuration edits or conversation compaction can legitimately invalidate a
cached prefix.

Anthropic's [usage guidance] recommends prompt caching to conserve subscription
allowance: for equivalent work, more cache hits and fewer repeated writes reduce
input-processing cost.
However, cached context still occupies the context window, output and thinking
tokens still consume allowance, and longer histories can consume more allowance
even with a high cache hit ratio.
API cache-price multipliers and the SDK's dollar estimates do not provide a
formula for subscription allowance percentages.

### Experimental evidence

The September 2026 probes ran against `claude-agent-acp` 0.76.0 and Claude Code
2.1.257 with subscription authentication, recording protocol traffic, exact
outputs, native transcript fixtures, and failure diagnostics.
Representative run IDs are listed below; the raw artifacts under
`tmp/acp-probe/` are investigation data rather than a substitute for checked-in
integration fixtures.

| Observation                                                                                 | Run                |
| ------------------------------------------------------------------------------------------- | ------------------ |
| Template-based history replacement and edited historical tool results, with no re-execution | `HYYmzJ`, `Ttcolz` |
| Canonical Opus 5 selection and seed-free native records                                     | `ZjvkdU`           |
| Denial, host-side argument/result editing, cancellation while a tool is blocked             | `mz8fnc`           |
| Changed system prompt/schema with retained invoice data                                     | `GRQZsv`           |
| Identical calls remain distinct and correctly paired                                        | `6zuTpk`           |
| Separate processes retain separate histories while one waits on a tool                      | `cZpjor`           |
| Image input and exact color identification                                                  | `FcZXUF`           |
| Complete large text result with both size controls                                          | `K7dVmM`           |

Among the negative probe runs, the unsuccessful marker-based configuration
request was a recorded provider refusal rather than a session reload failure,
and the default-limit and environment-only large-result probes failed inline
text preservation as expected.
These probes validate the protocol mechanics, not a completed JP integration,
universal runtime-version compatibility, or a quantitative subscription-quota
conversion formula.

## Drawbacks

The ACP subscription flow adds external runtime dependencies (Node.js,
`claude-agent-acp`, and the bundled Claude Code binary), requires maintaining a
converter for Claude Code's native transcript format, and relies on runtime
behavior outside JP's direct control.
Runtime-injected context and background helper requests can increase allowance
consumption, and compatibility qualification must track the adapter and its
bundled runtime together.

Switching the default `subscription_flow` to `acp` requires existing
subscription users to either set up the external runtime or explicitly opt into
`direct`.
Retaining `direct` alongside `acp` also preserves an unsanctioned transport that
JP must document clearly and maintain separately.

## Alternatives

**Keep `direct` as the default.** Avoids external runtime dependencies, but
leaves the account and policy risk on users who have not explicitly opted into
it.

**Expose a public `acp` provider.** Fits a general-purpose external-agent
harness, but leaks an Anthropic transport choice into provider and model
selection.
Keeping the Claude-specific integration inside `anthropic` preserves model IDs
and `--auth` semantics.

**Resume the last native session or flatten JP history into a memo.** Resuming a
session ID breaks cross-provider turns and history edits, while flattening
history into a user message loses roles and tool-call structure.
Converting the projected `Thread` into a native transcript preserves full
conversation history.

**Implement the adapter in Rust immediately.** Eliminates the Node.js
dependency, but expands the scope of the initial delivery.
A Rust adapter can replace `claude-agent-acp` later using the same behavioral
tests while continuing to run the official Claude Code binary.

## Non-Goals

- Removing `direct` subscription access or changing API-key behavior.
- Adding a generic `acp` provider or changing `model.id` syntax.
- Replacing Claude Code's authentication, copying its credentials, or modifying
  its binary.
- Implementing `jp provider llm auth` delegation or native-login profile mapping
  in the initial delivery.
- Relaxing JP's tool execution policies or adding a latency benchmark
  requirement.

## Risks and Open Questions

- **Large-result and aggregate limits:** qualify runtime behavior above the
  500,000-character inline fixture and across multiple large results in a single
  turn, then finalize the handling strategy above.
- **Cache preservation:** compare warm continuation, process restart, and full
  `Thread` reconstruction within the cache TTL while holding the working
  directory, account, model, effort, tool definitions, and input content fixed.
  Measure cache reads and writes on the shared prefix, since isolated cache hits
  do not prove equal cache reuse.
  Comparing actual subscription plan deduction requires a separate measurement
  on an idle account without a quota reset during the run, as cache token
  counters alone do not measure plan consumption.
- **Runtime-added context and work:** identify what context and helper requests
  the qualified runtime adds despite disabled discovery, and suppress or account
  for them without patching the binary or recording them as part of the
  `Thread`.
- **Control and metadata fidelity:** finish end-to-end mappings for reasoning,
  request controls, historical binary attachments (beyond the single-turn image
  probe), abort/discard, and `--no-persist` against JP's execution paths.
- **Version and policy changes:** publish the qualified adapter and runtime
  version pairs.
  Anthropic can change subscription allowances and permitted usage at any time,
  and explicitly selecting `direct` does not protect an account from
  enforcement.

## Implementation Plan

1. **Flow selection and compatibility.** Add the `subscription_flow` config
   field, default and migration diagnostics, and model/runtime qualification,
   while isolating the HTTP implementations and ensuring API-key construction
   never initializes ACP.
2. **One vertical slice.** Convert a projected `Thread` into a native
   transcript, run a subscription-backed request through `claude-agent-acp`,
   service [RFD 109]'s hosted MCP tools concurrently with the in-flight ACP
   prompt, and record events through JP's standard stream and rendering path.
   Give auxiliary requests isolated native sessions.
3. **Workflow and result parity.** Extend the provider-owned routing tests from
   [PR 1152] to cover alternating providers and flows, replay, forks,
   compaction, configuration changes, tool edits, cancellation, and refusals.
   Preserve large tool results inline without weakening assertions or
   re-executing historical tool calls.
4. **Caching and release qualification.** Add controlled cache comparisons,
   per-request usage accounting, checked-in runtime fixtures, and setup
   documentation.
   Prioritize prompt correctness over cache reuse, and evaluate subscription
   consumption from observed plan usage rather than treating SDK list-price
   dollar estimates as quota units.

These phases keep the initial delivery focused on the Anthropic subscription
flow; `jp provider llm auth` integration and replacing the Node.js adapter are
follow-up work.

[Claude Code's cache documentation]: https://code.claude.com/docs/en/prompt-caching
[PR 1151]: https://github.com/dcdpr/jp/pull/1151
[PR 1152]: https://github.com/dcdpr/jp/pull/1152
[PR 1153]: https://github.com/dcdpr/jp/pull/1153
[RFD 090]: 090-anthropic-subscription-auth-with-credential-fallback.md
[RFD 109]: 109-in-process-jp-mcp-server.md
[documented size override]: https://code.claude.com/docs/en/mcp#raise-the-limit-for-a-specific-tool
[subscription clarification]: https://support.claude.com/en/articles/15036540-use-the-claude-agent-sdk-with-your-claude-plan
[usage guidance]: https://code.claude.com/docs/en/costs#why-usage-climbs-in-a-long-session
