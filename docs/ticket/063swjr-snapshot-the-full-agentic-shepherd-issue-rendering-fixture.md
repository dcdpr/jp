# Snapshot the full Agentic Shepherd issue rendering fixture

- **Status**: Todo
- **Kind**: Chore
- **Authors**: jp
- **Date**: 2026-08-21

`renders_full_issue_fixture` renders a complete user-visible Markdown document but checks selected substrings. Duplicated sections, broken ordering, leaked content, and formatting changes can pass.

Acceptance criteria:

- Replace the substring checks with one exact static output assertion or an accepted snapshot.
- Keep focused unit tests for individual rendering rules.
- Normalize only genuinely unstable fields before comparison.
- Verify the test fails when a section is duplicated, reordered, or rendered with incorrect indentation.
