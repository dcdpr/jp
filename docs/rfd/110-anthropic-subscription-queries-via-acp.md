# RFD 110: Anthropic Subscription Queries via ACP

- **Status**: Accepted
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-09-11
- **Extends**: [RFD 090]
- **Requires**: [RFD 109]

## Summary

The `anthropic` provider gains an ACP subscription flow through
`claude-agent-acp` and the unmodified Claude Code runtime.
It becomes the default for subscription authentication; the existing direct flow
remains available by explicit configuration.
Model IDs, the `--auth` interface, JP conversation ownership, and JP tool
policies remain unchanged.

## Motivation

[RFD 090] and [PR 1151] through [PR 1153] provide subscription credentials,
provider-owned credential selection, fallback, and the `--auth` flag.
The direct Anthropic subscription implementation sends requests from JP using
JP-stored Claude Code OAuth credentials.
It works, but is not a vendor-sanctioned third-party authentication path and
risks account restriction.

Anthropic's June 15, 2026 [subscription clarification] states that Agent SDK,
`claude -p`, and third-party application usage continue to draw from
subscription allowances.
Running the official runtime avoids extracting its credentials or reimplementing
its authentication.
This is an additional subscription flow, not another provider or another route
to mandatory per-token billing.

The probe harness in `.config/jp/experiments/` demonstrates role-bearing history
reconstruction, tool execution through MCP, and the controls needed for an
integration.
A separate public ACP provider would expose an implementation choice the user
need not make.
JP's MCP server, specified in [RFD 109], exposes JP's configured tool pipeline
to external clients; this flow consumes its hosted form.

## Design

### User interface and setup

The following commands select billing without changing the provider or model:

```sh
jp q -n --auth sub -m anthropic/claude-opus-5 "Review this change."
jp q -n --auth api -m anthropic/claude-opus-5 "Review this change."
```

For ACP subscription usage, install Node.js 22 or later and npm, then install
and authenticate the adapter release used in the experiments:

```sh
npm install --global @agentclientprotocol/claude-agent-acp@0.76.0
claude-agent-acp --cli auth login --claudeai
claude-agent-acp --cli --version
claude-agent-acp --cli auth status --json
```

Keep npm's optional dependencies enabled.
On supported platforms the SDK supplies Claude Code's native binary; a separate
Claude Code installation is not normally necessary.
`node` and `claude-agent-acp` must be on JP's `PATH`.
The measured baseline is adapter 0.76.0 with Claude Code 2.1.257, not an
unqualified promise about every later release.

Login uses Claude Code's own browser flow and credential storage.
No token is copied into JP.
Disable paid **Usage credits** in Claude's **Settings > Usage** when only the
included subscription allowance may be used.
JP verifies effective subscription authentication before inference; an API key,
helper, or cloud configuration must not silently select another billing source.
Child-process configuration must not alter the parent environment or JP's API
flow.

With the default ACP subscription flow, no TOML changes are required when the
command explicitly selects `--auth sub` and the model.
To make these the workspace defaults, merge this into `.jp/config.toml`:

```toml
[providers.llm.anthropic]
auth = ["subscription"]
subscription_flow = "acp"

[assistant.model]
id = "anthropic/claude-opus-5"
```

`subscription_flow` defaults to `acp`, so its line is optional.
After setup, ordinary queries suffice:

```sh
jp query --new "Review the changes in this workspace."
jp query "Focus on error handling."
```

JP starts the adapter and its hosted MCP server, prepares the conversation,
handles tool interaction, and records the response.
The user runs no daemon, copies no tool configuration into Claude Code, and
manages no external session IDs.
Existing assistant settings, instructions, attachments, and tools stay in JP
configuration.

### Flow selection and migration

A *subscription flow* selects the implementation used for a subscription
authentication entry.
It does not select the billing kind, model, or service tier.
`subscription_flow` accepts `acp` and `direct`, rejects unknown values, and
follows ordinary scalar config layering and conversation deltas.

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

This is explicit acceptance of that flow's policy and account risk, not a claim
that opting in makes it permitted.
JP never selects it automatically because ACP is missing, unsupported, or fails.
API-only users keep their behavior and need no Node or Claude Code installation.
The existing default auth chain stays `["api_key"]`; adding this field does not
move API users onto subscriptions.

Subscription users who omit `subscription_flow` must install and authenticate
the external runtime or explicitly select `direct`.
Existing JP credentials are neither deleted nor imported into Claude Code.
Initial ACP support uses the runtime's active subscription login.
An unmapped named JP subscription credential must fail rather than silently use
that account; existing named credentials remain usable with `direct`.
Native-login name mapping and integration with `jp provider llm auth` are
follow-up work.

The existing credential-chain and `--auth` semantics remain authoritative.
Listing an API entry explicitly authorizes the existing paid fallback policy;
this feature adds no API entry and no automatic switch between subscription
flows.
Subscription-only requests, including auxiliary requests using that
configuration, stop when their allowance is unavailable.
Paid usage credits in Claude's account are separate from JP's auth chain; the
setup requirement above is not replaceable by a cache-cost estimate.

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

There is no public `acp` provider, agent selector, or change to `model.id`.
Internal ACP transport can be reusable, but this feature supports the qualified
Claude adapter, not arbitrary ACP executables.
The implementation owns the adapter command and version compatibility policy; it
does not expose an unrestricted SDK-options bag as a substitute for JP
configuration.

Credential policy stays in the provider as in [PR 1151].
Protocol and transcript conversion belong with the Claude integration, not in
command handlers.
[RFD 109] owns tool execution and the execution-host interface.
The query runner must service that host while the ACP prompt is in flight:
waiting for prompt completion before servicing MCP calls deadlocks.
Reuse the existing tool coordination and rendering rather than duplicate them in
the provider.

An ACP prompt covers the external agent's model/tool continuation loop.
Internal orchestration must represent that explicitly rather than send its
observed tool calls through JP's API-style execution phase a second time.
How the internal provider/request interface carries this execution contract is
settled in the first vertical slice; the public provider and authentication
choice do not expose it.

### JP remains the conversation authority

The flow consumes the same Thread and provider-visible projection as the
Anthropic request builder, including the Compacted View.
It separates the pending input from prior history, constructs Claude-native
records preserving supported content, roles, order, and paired tool
calls/results, then loads them through ACP.
The pending input is submitted once, not also embedded in the loaded prefix.
Continuation without a new user request must preserve the existing provider's
continuation semantics rather than repeat an earlier request.
History is not flattened into a user-message memo.

Native records are a derived provider representation.
The demonstrated encoder needs no seed response: record bookkeeping is authored
locally, while message content comes from JP.
Native storage formats are version-specific; their encoder and decoder need
fixtures and qualified runtime versions.
Opaque reasoning metadata follows existing Anthropic conversion rules, not
invented signatures or claims that every provider's reasoning is
interchangeable.

Provider changes, replay, selected-turn forks, compaction, and attachment
changes all prepare the current Thread through this conversion.
Returning from an OpenAI turn therefore includes that turn without a manual
handoff:

```sh
jp q -n --auth sub -m anthropic/claude-opus-5 "Review the design."
jp q --auth sub -m openai/gpt-6-astra "Check the assumptions."
jp q --auth sub -m anthropic/claude-opus-5 "Continue from that review."
```

The implementation can reuse native state only when it represents the current
Thread and configuration.
A saved session ID alone is insufficient.
Otherwise, create a separate native transcript and load it.
Keep immutable JP history separate from disposable provider files, preserve
input content when remapping record identifiers, and never edit a user's
unrelated Claude Code session.

Import newly generated events once.
Load-time replay is historical observation, not new output or authorization to
execute a historical call.
Context isolation uses distinct ACP sessions for independent requests, including
auxiliary queries.
Tool callbacks and side queries must not share a mutex around one occupied
native session.
Conversation locking and durable writes remain JP's responsibility.

### Tools, output, and model support

The ACP flow supplies JP's MCP server with a stable tool namespace.
It disables Claude Code's native side-effectful tools and unconfigured MCP
servers, and suppresses optional hooks, skills, and background features through
qualified controls.
The runtime's actual tool surface is checked.
These controls are not an OS sandbox, and native runtime context must be
accounted for rather than mistaken for JP attachments.

The hosted server executes through JP's policies: enablement, approval, argument
editing, tool options, access checks, inquiries, result editing, and recording.
ACP tool updates are observations.
The tested `_meta["claudecode/toolUseId"]` on MCP call requests correlates
execution with ACP's tool-call ID; request numbers or matching arguments are not
substitutes.
Preserve the distinction between requested and edited execution arguments.

Sequential external tool dispatch is acceptable.
Correct pairing, JP's approval behavior, and isolation are not optional.
Forced tool selection retains JP's existing best-effort semantics; this flow
does not promise stronger enforcement.
Cancellation reaches pending interactions and running tools.
Disconnection after a possible side effect is not permission to repeat it
automatically.

Use `ModelDetails.subscription` and the existing capability fields for the
selected flow's supported model set and controls.
Resolve canonical IDs against qualified model information, and retain the actual
response model in metadata.
Do not require ACP discovery for API-only requests or silently substitute a
different model.
Explicit unsupported controls need the existing capability handling, not silent
removal.
HTTP-specific transport settings remain scoped to the HTTP implementations.

Apply the resolved system prompt and response schema when preparing a request.
The tested custom-prompt form uses `snapshot: false`; native prompt snapshots
must not suppress JP configuration changes.
Structured results arrive through the adapter's raw SDK result extension and
become JP structured responses.
Streamed text/thinking use JP's existing rendering.
A refusal maps to `FinishReason::Refused`, including its category when supplied;
an SDK result with `subtype: success` and `is_error: true` is not success.

### Large tool results

Claude Code can replace a large tool result with a file reference before the
next model request.
That is not acceptable merely because JP still stores the original: with native
file tools disabled, the model may not receive the data.

The measured working configuration is:

- `MAX_MCP_OUTPUT_TOKENS=100000` in the runtime environment.
- `_meta["anthropic/maxResultSizeChars"] = 500000` on each relevant JP tool's
  `tools/list` entry.

This preserves a 240,052-byte text result through the SDK-visible boundary and
lets the model answer from its footer.
The environment setting alone fails the same test.
Keep the text-preservation assertion; retrieving a substituted file through an
uncontrolled native tool is not an equivalent result.

The [documented size override] has a maximum value of 500,000 characters.
This is an inline-text threshold for the result of one tool invocation, not a
limit on JP's stored conversation, the full request, or the model's context
window.
Larger results can still be produced, but the runtime can substitute a file
reference.
Multiple large results and other runtime context-management policies can impose
additional constraints.

Handling results outside the qualified range remains an explicit compatibility
edge: establish a provider-controlled continuation/reconstruction mechanism that
preserves them, or agree a documented limitation before claiming parity.
Silent truncation, automatic `direct` fallback, and bypassing JP permissions are
not solutions.
A diagnostic avoids silent loss but is not proof that the original workflow is
supported.

### Prompt caching and subscription usage

Prompt caching is server-side reuse of an identical request prefix, not reuse of
a local session file.
[Claude Code's cache documentation] explains that matching requests can share a
cache across sessions.
Reconstructing a transcript therefore need not destroy caching, but changes in
rendered content can.

The traces already demonstrate cache reads: an ordinary follow-up reads 836
cached input tokens, and reconstructed-history requests each read 1,322.
These examples use different prompts/models and one-hour cache writes.
They prove reuse of some prefix, not equal cache efficiency or parity with
native continuation for an arbitrary JP Thread.

Preserve stable tool names, definitions and ordering, message content, and
wire-visible tool-call IDs when the Thread is unchanged.
Avoid putting transient session IDs, listener addresses, or per-request
temporary working directories into the model-visible prefix.
Native file locations can vary without changing the agent's logical working
directory.
Account for runtime-added environment and git context when deciding whether a
prefix is stable.
Do not sacrifice current instructions or correct history to preserve a cache
entry.

Honor `assistant.request.cache` through the qualified runtime controls:

| JP policy       | ACP flow mapping                                                                                         |
| --------------- | -------------------------------------------------------------------------------------------------------- |
| `off`           | `DISABLE_PROMPT_CACHING=1`.                                                                              |
| `short`         | `CLAUDE_CODE_PROMPT_CACHE_TTL=5m`.                                                                       |
| `long`          | `CLAUDE_CODE_PROMPT_CACHE_TTL=1h`.                                                                       |
| Custom duration | Existing Anthropic mapping: at least 30 minutes selects one hour; shorter durations select five minutes. |

These published runtime controls require integration tests.
Isolate conflicting ambient runtime overrides; report a managed-policy conflict
rather than claim a JP setting was honored when it was not.
JP's default remains `short`, rather than silently adopting Claude Code's
subscription default of one hour.
JP-initiated auxiliary requests use their own resolved policy.
Cache breakpoint placement need not be byte-identical between flows, but
supported caching controls and unchanged-prefix reuse must remain useful.

Record uncached input, cache creation, cache reads, and output separately.
Distinguish per-request usage from cumulative `modelUsage` snapshots and runtime
helper activity.
Switching models, accounts, or flows may change cache scope; sharing between
them is not guaranteed.
Configuration edits and compaction can legitimately invalidate a prefix.

Anthropic's [usage guidance] identifies caching as a way to conserve plan
allowance.
For otherwise equivalent work, more cache hits and fewer repeated writes reduce
input-processing expense.
Cached context still occupies the context window, output/thinking still consumes
usage, and larger histories can consume more allowance even with a high hit
ratio.
API cache-price multipliers and the SDK's dollar estimate are not a published
formula for subscription window percentages.

### Experimental evidence

The September 2026 probes use adapter 0.76.0 and Claude Code 2.1.257 with
subscription authentication.
The probe harness records protocol traffic, exact outputs, native fixtures, and
failure details.
Representative retained run IDs are listed here; the `tmp/acp-probe/` artifacts
are investigation data, not a substitute for checked-in integration fixtures.

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

The unsuccessful marker-based configuration request is a recorded provider
refusal, not evidence of a general reload failure.
The default-limit and environment-only large-result probes retain their failed
preservation checks.
No experiment establishes completed JP integration, universal runtime-version
compatibility, or a quantitative subscription-quota conversion.

## Drawbacks

ACP subscription usage adds Node and an external runtime, native transcript
format maintenance, and runtime behavior outside JP's direct control.
Hidden context and helper requests can increase allowance consumption.
Compatibility qualification must track the adapter and its bundled runtime
together.

Changing the default subscription flow requires existing subscription users to
prepare that runtime or opt into `direct`.
Keeping direct access preserves a risky alternative that JP must label honestly
and maintain separately.

## Alternatives

**Keep direct as the default.** Requires fewer dependencies, but leaves the
policy risk on users who have not chosen it explicitly.

**Expose an `acp` provider.** Useful for a generic external-agent product, but
unnecessary for selecting how this vendor serves subscription requests.
The Claude-specific implementation stays inside `anthropic`.

**Resume the last native session or flatten JP history into a memo.** Neither
preserves normal provider switching and projected history.
Native transcript conversion provides the demonstrated alternative.

**Implement the adapter in Rust immediately.** Removes Node but expands the
initial work.
A later replacement can use the same behavioral tests while continuing to run
the official Claude Code binary.

## Non-Goals

- Removing direct subscription access or changing API-key behavior.
- Adding a generic ACP provider or changing model-ID syntax.
- Replacing Claude Code's authentication, copying its credentials, or modifying
  its binary.
- Implementing `jp provider llm auth` delegation or native-login profile mapping
  in the first delivery.
- Giving tools weaker policies or adding a latency benchmark requirement.

## Risks and Open Questions

- **Large-result and aggregate limits:** qualify behavior outside the measured
  fixture and resolve the handling decision above.
- **Cache preservation:** compare warm continuation, process restart, and
  reconstruction of the same Thread within the TTL, holding directory, account,
  model, effort, tool definitions, and input content fixed.
  Measure the shared prefix's cache reads/writes; existing hits do not prove
  equal cache reuse.
  A separate comparison of actual plan usage needs a quiet account and no quota
  reset during measurement; cache counters alone do not measure that deduction.
- **Runtime-added context and work:** identify what the qualified runtime adds
  despite disabled discovery, and suppress or account for it without editing the
  binary or pretending the Thread contains it.
- **Control and metadata fidelity:** finish mappings for reasoning, request
  controls, attachments, abort/discard, and `--no-persist` against JP's actual
  paths.
  The small image probe is not historical binary-content coverage.
- **Version and policy changes:** publish the supported adapter/runtime
  combinations.
  Anthropic can change subscription allowances and permitted usage; an explicit
  direct choice does not protect an account from enforcement.

## Implementation Plan

1. **Flow selection and compatibility.** Add the typed field, default and
   migration diagnostics, isolate the retained HTTP implementations, and qualify
   model/runtime support.
   API construction must not initialize ACP.
2. **One vertical slice.** Convert a real Thread, run a subscription-backed
   request, service [RFD 109]'s hosted tools concurrently with ACP, and record
   through JP's actual stream/rendering path.
   Give auxiliary requests isolated native state.
3. **Workflow and result parity.** Extend the provider-owned route tests from
   [PR 1152].
   Cover alternating providers/flows, replay, forks, compaction, configuration
   changes, tool edits, cancellation and refusals.
   Resolve large results without weakening assertions or executing historical
   calls.
4. **Caching and release qualification.** Add controlled cache comparisons,
   usage accounting, runtime fixtures, and setup documentation.
   Keep prompt correctness ahead of cache reuse; use subscription usage
   observations rather than treating list-price dollars as quota units.

These phases keep the initial delivery focused on the Anthropic subscription
flow.
Public auth-command integration and replacing Node are subsequent work.

[Claude Code's cache documentation]: https://code.claude.com/docs/en/prompt-caching
[PR 1151]: https://github.com/dcdpr/jp/pull/1151
[PR 1152]: https://github.com/dcdpr/jp/pull/1152
[PR 1153]: https://github.com/dcdpr/jp/pull/1153
[RFD 090]: 090-anthropic-subscription-auth-with-credential-fallback.md
[RFD 109]: 109-in-process-jp-mcp-server.md
[documented size override]: https://code.claude.com/docs/en/mcp#raise-the-limit-for-a-specific-tool
[subscription clarification]: https://support.claude.com/en/articles/15036540-use-the-claude-agent-sdk-with-your-claude-plan
[usage guidance]: https://code.claude.com/docs/en/costs#why-usage-climbs-in-a-long-session
