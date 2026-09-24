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

ACP compatibility checks require `claude-agent-acp` 0.81.0 with Claude Code
2.1.280 and an active Pro or Max login:

```sh
npm install --global --prefix "$HOME/.local" --include=optional @agentclientprotocol/claude-agent-acp@0.81.0
export PATH="$HOME/.local/bin:$PATH"
claude-agent-acp --version
claude-agent-acp --cli --version
jp provider llm auth login anthropic --name sub
jp provider llm auth list
```

Use Node.js 22 or later and keep npm's optional dependencies enabled.
In fish, use `fish_add_path "$HOME/.local/bin"` instead of the `export` line.
The adapter bundles Claude Code; a separate installation is unnecessary.
The auth status must report a first-party Claude account with a Pro or Max plan,
not an API-key source.
Disable paid Usage credits in Claude's Settings > Usage if no paid overage is
permitted.
JP does not copy Claude Code's tokens.
Unnamed subscription entries select its inherited active login.
Named entries select logins registered through `jp provider llm auth login`.

The initial ACP implementation supports queries through JP's tool execution
service, including approvals, tool questions, result editing, and recording.
JP derives a separate Claude-native transcript from the current conversation for
each request; auxiliary queries do not share the main query's native session.

```sh
jp query --new --auth sub:sub --model anthropic/claude-opus-5 "Review this change."
```

This v0.1 path requires the runtime versions above.
The launcher uses Unix process groups or Windows job objects to clean up
descendant processes.
On Windows, npm's `claude-agent-acp.cmd` must be on PATH.
Model identifiers and aliases are passed to Claude Code without a JP allowlist.
Claude Code decides availability for the active account; JP reports
model-selection and request failures with the runtime's explanation.
Canonical identifiers returned for aliases are accepted and reported in usage
traces.
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
Claude Code chooses the output-token limit unless
`assistant.model.parameters.max_tokens` is explicitly set.
That override applies to each underlying model request, including its reasoning
tokens, not to the sum of every request in a Turn.

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

### Named subscriptions

Anthropic login defaults to Claude Code authentication through ACP:

```sh
jp provider llm auth login anthropic --name sub
jp provider llm auth login anthropic --name sub2
jp provider llm auth list
```

JP creates a separate directory for each account at `<JP user data
dir>/data/claude/<name>` and supplies `CLAUDE_CONFIG_DIR` to the bundled
runtime.
The user-data root honors `JP_USER_DATA_DIR`, then `$XDG_DATA_HOME/jp`, then the
platform default.
Names may contain ASCII letters, digits, hyphens, and underscores.

JP stores the absolute directory and account identity, not Claude Code's tokens.
Claude Code owns credential storage and refresh.
Repeating login for a registered name reuses its original directory.
No profile-manager dependency is required.

Select an account with an explicit subscription name:

```sh
jp query --new --model anthropic/claude-sonnet-5 --auth sub:sub2 "Reply with exactly OK."
```

`--auth sub` is shorthand for an unnamed `subscription`, not the account named
`sub`.
Use `--auth sub:sub` to select that named account.

To register an existing login directory without relocating it, pass its exact
absolute path when signing in:

```sh
jp provider llm auth login anthropic --name sub2 --config-dir "$HOME/.local/share/jp/claude/sub2"
```

The selected directory applies to authentication checks, the adapter and its SDK
subprocess, and derived conversation history.
JP sets `CLAUDE_SECURESTORAGE_CONFIG_DIR` to the same directory so an inherited
override cannot select another account.
An unnamed `subscription` preserves the inherited login environment, including
whether `CLAUDE_CONFIG_DIR` is unset.

`auth list` checks each registered runtime login and reports it as a
subscription.
A failed status check reports `unavailable`, not `valid`.
Logout clears that account's runtime login before removing its registration, and
retains the registration if the runtime fails:

```sh
jp provider llm auth logout anthropic --name sub2
```

Logout does not delete configuration directories or conversation history.
Concurrent login/logout operations are rejected; listing and queries do not wait
for an interactive login to complete.

Manual `providers.llm.anthropic.acp_config_dirs` mappings remain supported for
unregistered directories.
Their values must be absolute paths, without `~` or `$HOME` expansion, and each
layer replaces the whole map.
A mapping sharing a registered name must match its registered directory; remove
a stale mapping rather than silently querying another account.
Manual mappings are not themselves registrations for `auth list` or `auth
logout`.

### Subscription quota fallback

Registered subscriptions are tried in the configured order when Claude Code
reports an exhausted subscription window:

```toml
[providers.llm.anthropic]
subscription_flow = "acp"
auth = ["sub:sub", "sub:sub2", "api_key"]
```

JP shares the direct flow's scoped cooldowns and chain advancement.
A spent account is skipped until its reported reset, or for 30 minutes when no
reset is available.
Model-specific windows do not block other model families.
The cooldown is stored across invocations and appears in `auth list`.

Including `api_key` authorizes paid API access after the subscriptions are
spent.
Omit it to stop when the subscription chain is exhausted.
ACP never switches to direct subscription-token access.
Runtime setup failures, model errors, and ordinary rate limits without
subscription-quota evidence do not change accounts.
Warnings and rejected extra-usage allowances are not themselves subscription
exhaustion.

Switching rebuilds the request from JP's committed history, preserving completed
tool results.
An agent request with unrecorded tool results stops rather than risking repeated
side effects.
Credential changes do not consume the transient retry budget.

Automatic fallback requires registered names from `jp provider llm auth login`;
JP does not attribute cooldowns to an unnamed inherited login or a
directory-only manual mapping.
Without a recorded cooldown, including when the credential store cannot be
written, an exhausted subscription stops the request instead of switching.
A single-entry `--auth sub:sub2` replaces the configured chain and therefore
disables fallback to other entries for that selection.

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

Debug tracing reports uncached input, cache writes, cache reads, and output
separately.
These diagnostics are not stored in conversation metadata.
Runtime aggregate totals are separate from main-request usage and must not be
added to it.
SDK dollar estimates are not subscription charges or quota percentages.
See the [qualification procedure] for the diagnostic format.

### Direct subscription access

Direct token login requires explicit opt-in:

```sh
jp provider llm auth login anthropic --name personal --direct --setup-token
```

Queries using those tokens must also opt in:

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
