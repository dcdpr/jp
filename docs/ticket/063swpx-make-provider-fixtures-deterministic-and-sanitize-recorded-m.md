# Make provider fixtures deterministic and sanitize recorded machine data

- **Status**: Todo
- **Kind**: Chore
- **Authors**: jp
- **Date**: 2026-08-21

Several fixtures contain avoidable nondeterminism or local machine data:

- The common tool prompt asks the model to provide arbitrary arguments.
- `banana.jpg` depicts an apple and the test uses a substring assertion.
- Every generic OpenAI fixture uses the same conversation timestamp and prompt-cache key.
- llama.cpp cassettes record `/Users/jean/...`.
- Other recorded fixtures contain host names, UUIDs, process IDs, and absolute paths.

Acceptance criteria:

- Replace the arbitrary tool prompt with separate deterministic scalar, array, default, and nested-value cases.
- Rename the image fixture to match its content and compare the normalized answer exactly.
- Give each cassette a fixed distinct conversation identity while keeping all turns in one cassette on the same identity.
- Extend fixture post-processing to normalize user paths, host names, device UUIDs, process IDs, and timestamps when those values are not under test.
- Add a fixture lint test that rejects common absolute home-directory patterns and known secret header/query fields.
