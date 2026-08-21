# Replace duplicated full provider catalogs with focused mapping fixtures

- **Status**: Todo
- **Kind**: Chore
- **Authors**: jp
- **Date**: 2026-08-21

Provider model-list fixtures are too large to review and several model-details cassettes duplicate the full catalog byte for byte. OpenRouter has two identical 29,699-line cassettes plus a 10,111-line model snapshot.

Google, llama.cpp, and Ollama also duplicate their model-list cassette for `models` and `model_details` tests.

Acceptance criteria:

- Keep one recorded catalog smoke cassette per provider where live format coverage is useful.
- Test model mapping, sorting, deduplication, deprecation, capability inference, and lookup with small curated static responses.
- Reuse one cassette response for `models` and `model_details` when both production methods call the same endpoint.
- Replace full-catalog snapshots with exact assertions over selected representative records and list invariants.
- Keep a test proving an unknown requested model returns the intended fallback.
