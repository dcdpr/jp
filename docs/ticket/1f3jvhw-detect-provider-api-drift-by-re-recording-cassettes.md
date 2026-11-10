# Detect provider API drift by re-recording cassettes

- **Status**: Todo
- **Authors**: jp
- **Date**: 2026-11-07
- **Label**: domain=llm
- **Label**: domain=tooling
- **Label**: package=jp_llm
- **Label**: package=jp_test
- **Label**: type=task

Provider tests replay recorded cassettes, so they check our parsing against what
a provider answered on the day someone last ran `RECORD=1`.
When a provider changes a response, such as a renamed field, a new SSE event
type, or a reworded error, nothing fails until someone happens to re-record that
provider.
Some classification depends on exact wording.
PR #1298 recognizes unknown-model refusals by message text for Anthropic
(`model:`), OpenRouter (`is not a valid model ID`), Ollama (`model '…' not
found`), and Claude Code over ACP (`Model '…' not found`); when that wording
changes, the error falls back to `StreamErrorKind::Other` and nothing reports
it.
Gemini's wording already drifted once, from `Call ListModels` to `Call
ModelService.ListModels`, and was only noticed by re-recording.
The ACP recordings (`tests/fixtures/acp/`) belong in the same run: the models
Claude Code offers and the names it gives them change with Claude Code releases.

Run a scheduled job (monthly, or weekly if the cost allows) that re-records
every provider suite against the live APIs, normalizes away what is expected to
differ between runs, and reports any remaining difference against the committed
cassettes and snapshots.

Two kinds of signal:

- **Semantic assertions fail.** A case such as `unknown_model` asserts the
  refusal is classified as `UnknownModel`, so a re-record against a reworded
  error fails outright.
  This needs nothing beyond running the suite with `RECORD=1`.
- **Response shape differs.** The status code, content type, JSON keys and value
  types, SSE event names, and error envelopes are compared, not the values.
  Generated text, token counts, IDs, timestamps, signatures, cache keys, and
  rate-limit headers are dropped before comparison.

Open questions:

- Where it runs.
  Cloud providers need API keys as CI secrets, and the subscription routes need
  a signed-in login (Claude Code for ACP). llama.cpp, Ollama, and vLLM need a
  server (`JP_TEST_UPSTREAM`): a small model on the runner, a self-hosted
  machine, or skipping them in the scheduled run.
- What a detected change produces: a draft PR with the re-recorded fixtures, an
  issue or ticket with the normalized diff, or both.
  Nothing should be committed to `main` automatically.
- Token cost per run, which decides how often it runs.

Depends on T-063swpx: snapshots holding nondeterministic values would report
drift on every run.

- [ ] A recipe that re-records all provider suites (both billing routes) into a
      scratch directory, leaving the committed fixtures untouched
- [ ] Normalization that reduces a cassette response to its shape, with the
      dropped fields listed in one place
- [ ] A comparison step that reports semantic test failures and shape
      differences per provider and per case
- [ ] A scheduled workflow that runs the recipe and opens a PR or issue when
      anything differs
- [ ] Documentation for running it locally against a single provider
