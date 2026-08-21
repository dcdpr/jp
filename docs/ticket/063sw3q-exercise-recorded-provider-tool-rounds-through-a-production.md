# Exercise recorded provider tool rounds through a production-shaped loop

- **Status**: Todo
- **Kind**: Chore
- **Authors**: jp
- **Date**: 2026-08-21

`TestRequest::tool_call_response` creates a fresh `ChatQuery` with no tools. Production keeps the full tool list on every streaming cycle and resets a forced choice to `Auto` after execution.

The mismatch is visible in the fixtures: Ollama says no tool is defined after JP sends a result, and some Google forced-tool follow-ups contain no assistant message. These tests do not cover repeated tool calls or continued tool availability.

Acceptance criteria:

- Add a recorded-provider test path that retains tool definitions across the post-result request, matching `run_turn_loop`.
- Use a fake executor so the recorded response is produced by the same request, execute, append-result, request cycle used in production.
- Cover a second tool call after the first result.
- Cover parallel calls where the provider supports them.
- Assert that forced choice becomes `Auto` while tools remain declared.
- Keep lower-level provider serialization tests where useful, but name them as such.
