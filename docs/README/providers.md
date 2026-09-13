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
npm install --global @agentclientprotocol/claude-agent-acp@0.76.0
claude-agent-acp --cli auth login --claudeai
```

Use Node.js 22 or later and keep npm's optional dependencies enabled.
Disable paid Usage credits in Claude's Settings > Usage if no paid overage is
permitted.
JP does not copy Claude Code's tokens.
Unnamed subscription entries select its active login; JP credential names are
not mapped to Claude Code accounts.

**ACP prompt execution is not implemented yet.** RFD 110 Phase 1 provides flow
selection and compatibility checks, not subscription-backed ACP inference.
The initial qualified model is `claude-opus-5`; other model names are rejected
rather than silently substituted.
Compatibility failures do not switch to `direct` or to paid API access.

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
