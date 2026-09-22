# Exercise recorded provider tool rounds through a production-shaped loop

- **Status**: Todo
- **Kind**: Chore
- **Authors**: jp
- **Date**: 2026-08-21
- **Label**: domain=llm
- **Label**: package=jp_llm
- **Label**: type=task

`TestRequest::tool_call_response` creates a fresh `ChatQuery` with no tools.
Production keeps the full tool list on every streaming cycle and resets a forced
choice to `Auto` after execution.

The mismatch is visible in the fixtures: Ollama says no tool is defined after JP
sends a result, and some Google forced-tool follow-ups contain no assistant
message.
These tests do not cover repeated tool calls or continued tool availability.

Acceptance criteria:

- Add a recorded-provider test path that retains tool definitions across the
  post-result request, matching `run_turn_loop`.
- Use a fake executor so the recorded response is produced by the same request,
  execute, append-result, request cycle used in production.
- Cover a second tool call after the first result.
- Cover parallel calls where the provider supports them.
- Assert that forced choice becomes `Auto` while tools remain declared.
- Keep lower-level provider serialization tests where useful, but name them as
  such.

## Comments

-----

- **From**: jp
- **Date**: 2026-09-21T19:55:44Z

The same construction drops reasoning, not just tools.

`test.rs:591` builds the follow-up as a fresh `TestRequest::chat(provider_id)`,
which seeds `reasoning = Off` alongside the empty tool list.
So `tool_call_reasoning` and `tool_call_required_reasoning` enable reasoning on
the first request and silently disable it on the second.

The vLLM fixture shows what that produces.
The follow-up replays the prior reasoning in the history while telling the
server not to think:

```yaml
{
  "role": "assistant",
  "reasoning_content": "The user wants me to run the tool with whatever arguments...",
  "tool_calls": [ ... ]
},
...
"chat_template_kwargs": { "enable_thinking": false }
```

llama.cpp's fixture carries the same `true` then `false` pair, so this is every
provider, not a vLLM quirk.

The consequence is that no test covers sending prior reasoning back to a
provider with reasoning still on.
That is the fragile path: Anthropic requires thinking blocks replayed with their
signatures, and `openai_tests.rs` carries a dedicated recorded test for replayed
native reasoning items because the provider rejects a malformed replay.
The shared suite names two tests after reasoning and exercises neither
round-trip.

Worth folding into this ticket rather than filing separately: it is the same
line, the same fix shape, and the same re-recording cost across seven live
endpoints.
Splitting them means recording every provider twice.

Suggested additional acceptance criteria:

- Carry the first request's reasoning setting into the post-result request, so a
  reasoning test stays a reasoning test for the whole exchange.
- Assert the replayed assistant turn keeps whatever the provider needs to accept
  it (Anthropic's thinking signature, OpenAI's native reasoning item id).
