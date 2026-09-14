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

This v0.1 path requires the runtime versions above.
The launcher uses Unix process groups or Windows job objects to clean up
descendant processes.
On Windows, npm's `claude-agent-acp.cmd` must be on PATH.
Model identifiers and aliases are passed to Claude Code without a JP allowlist.
Claude Code decides availability for the active account; JP reports
model-selection and request failures with the runtime's explanation.
Canonical identifiers returned for aliases are accepted and recorded in usage
metadata.
JP does not set or change `CLAUDE_CONFIG_DIR` for a default login: that variable
also selects credentials, including the macOS Keychain entry.
A user-supplied value is inherited unchanged.

JP stores derived Claude Code conversations under
`~/.claude/projects/jp-<conversation-id>-<workspace-id>/`, or under the
configured `CLAUDE_CONFIG_DIR`.
Auxiliary requests use a separate directory derived from their working
directory.
Session filenames remain unique per request; the real working directory passed
to Claude Code is unchanged.

Restarting tool execution stops the attempt while keeping the original MCP call
open.
The MCP Host re-prepares the call before another execution attempt.
Temperature, top-p, top-k, stop words, service tiers other than `off`, and
custom model parameters have no ACP mapping.
Non-default values are ignored with a warning in tracing output; they do not
prevent the query.
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

For ACP subscriptions, `assistant.request.cache = "off"` explicitly disables
caching.
Every other value leaves caching and retention to Claude Code; JP sends no TTL
override.
There is no `auto` value, and direct HTTP flows keep their own existing
cache-policy behavior.

JP removes inherited cache environment overrides from the launched process.
Claude Code's own defaults and managed policy decide retention when caching is
not disabled.

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
