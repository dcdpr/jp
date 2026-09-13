# Provider-Agnostic

JP works with any LLM provider.
Switch between cloud and local models with a single flag:

```sh
# Long arguments and flags
jp query --model anthropic/claude-sonnet-4-6 "Explain this function"

# Short arguments and flags
jp q -m ollama/qwen3:8b "What is the purpose of this module?"

# Custom model aliases
jp q -m gpt "How do I paginate?"
```

You can switch models at any time, use different defaults in different
situations, add model aliases, and start using newly released models without
updating JP.
No lock-in.

## Anthropic subscription flows

`providers.llm.anthropic.subscription_flow` selects `acp` (the default) or
`direct` for subscription entries in the authentication chain.
API-key entries always use the existing HTTP implementation and require no
external runtime.
Model IDs and `--auth` syntax are unchanged.

ACP compatibility checks require `claude-agent-acp` 0.76.0 with Claude Code
2.1.257 and an active Pro or Max login:

```sh
npm install --global --prefix "$HOME/.local" --include=optional @agentclientprotocol/claude-agent-acp@0.76.0
export PATH="$HOME/.local/bin:$PATH"
claude-agent-acp --version
claude-agent-acp --cli --version
claude-agent-acp --cli auth login --claudeai
claude-agent-acp --cli auth status --json
```

Use Node.js 22 or later and keep npm's optional dependencies enabled.
In fish, use `fish_add_path "$HOME/.local/bin"` instead of the `export` line.
The adapter bundles Claude Code; a separate installation is unnecessary.
The auth status must report a first-party Claude account with a Pro or Max plan,
not an API-key source.
Disable paid Usage credits in Claude's Settings > Usage if no paid overage is
permitted.
JP does not copy Claude Code's tokens.
Unnamed subscription entries select its active login; JP credential names are
not mapped to Claude Code accounts.

The initial ACP implementation supports queries through JP's tool execution
service, including approvals, tool questions, result editing, and recording.
JP derives a separate Claude-native transcript from the current conversation for
each request; auxiliary queries do not share the main query's native session.

```sh
jp query --new --auth sub --model anthropic/claude-opus-5 "Review this change."
```

This v0.1 path currently requires Unix and the runtime versions above.
The initial qualified model is `claude-opus-5`; other model names are rejected
rather than substituted.
Restarting an external tool batch is not implemented yet, and native history
preparation rejects working directories whose encoded names exceed 200 bytes.
Temperature, top-p, top-k, service tiers other than `off`, and custom model
parameters have no ACP mapping and are rejected when set.
Compatibility failures do not switch to `direct` or to paid API access.
If Claude Code substitutes a `<persisted-output>` file reference for a tool
result, JP reports the loss instead of accepting it silently.
There is no automatic file read-back or reconstruction for that case in v0.1;
request a smaller tool result.
The size hints preserve the measured large-result case, not arbitrarily large
results or batches.

The reconstructed-history path has tests for provider switching, selected-turn
forks, replay, compaction, and changed instructions, schemas, and tool results.
Live qualification is opt-in; the [qualification procedure] describes the
production-provider cache comparison and how to interpret its usage reports.
Fixture tests alone do not establish live cache efficiency.

### Prompt caching

`assistant.request.cache` maps to the Claude runtime's cache controls:

| Policy            | Behavior                                                   |
| ----------------- | ---------------------------------------------------------- |
| `short` (default) | Request five-minute cache retention.                       |
| `long`            | Request one-hour cache retention.                          |
| `off`             | Disable prompt caching.                                    |
| Custom duration   | Below 30 minutes selects five minutes; otherwise one hour. |

JP overrides conflicting inherited cache environment settings.
Runtime-managed policy can still affect requests; live qualification checks the
reported cache writes rather than assuming a requested setting took effect.

Provider events carry usage snapshots with uncached input, cache writes, cache
reads, and output counted separately.
Runtime aggregate totals are separate from main-request usage and must not be
added to it.
SDK dollar estimates are not subscription charges or quota percentages.
See the [qualification procedure] for the metadata format and deduplication
rules.

### Direct subscription access

Existing direct subscription users must opt in explicitly:

```toml
[providers.llm.anthropic]
auth = ["subscription"]
subscription_flow = "direct"
```

This keeps JP-stored credentials and the existing direct HTTP behavior.
It carries Anthropic account-policy risk; explicit configuration does not make
it a vendor-sanctioned route.
Existing credentials are not deleted or imported into Claude Code.
Named subscriptions remain available with this flow.

[back to README]

[back to README]: ../../README.md
[qualification procedure]: ../architecture/anthropic-acp-qualification.md
