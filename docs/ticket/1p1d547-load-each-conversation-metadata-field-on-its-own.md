# Load each conversation metadata field on its own

- **Status**: Todo
- **Authors**: jp
- **Date**: 2026-11-20
- **Label**: domain=storage
- **Label**: package=jp_conversation
- **Label**: package=jp_storage
- **Label**: type=enhancement

`metadata.json` is deserialized as one `Conversation`, so a single field that
fails to parse (a malformed `last_activated_at`, say) loses every other field
with it: the title, labels, pin, and expiry all read as absent, and callers such
as `jp conversation rm` show no title for a conversation whose title is stored
and readable.

Load what can be loaded: each field that parses keeps its value, each field that
doesn't falls back to its default, and every field that failed is still reported
as an error naming the field, so nothing is silently dropped.

- [ ] A field that fails to parse does not discard the fields that do
- [ ] Each failed field is reported, with the field name, through the same
      load-failure path as other metadata errors
- [ ] Tests with one malformed field next to a valid `title` and `labels`
