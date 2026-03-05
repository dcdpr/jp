# Provider-Agnostic

JP works with any LLM provider. Switch between cloud and local models with a
single flag:

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
updating JP. No lock-in.

[back to README](../../README.md)
