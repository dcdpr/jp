# Wire xct2cli XML fixtures into tests and anonymize them

- **Status**: Todo
- **Kind**: Chore
- **Authors**: jp
- **Date**: 2026-08-21

`crates/contrib/xct2cli/tests/fixtures/sample-toc.xml` and `time-sample.xml` have no test consumer; the parser tests use inline XML. The files also retain a device name, device UUID, username-bearing absolute path, and process IDs while the README calls the exported fixtures safe to keep.

Acceptance criteria:

- Add tests that parse both checked-in fixture files through the production entry points, or delete the files if the inline cases fully replace them.
- Assert the meaningful TOC and time-sample business outcomes exactly.
- Replace device names, UUIDs, user paths, process IDs, and addresses with stable synthetic values unless a raw value is needed by the parser case.
- Clarify the README: exported XML omits the trace bundle's environment secrets but still requires anonymization before commit.
