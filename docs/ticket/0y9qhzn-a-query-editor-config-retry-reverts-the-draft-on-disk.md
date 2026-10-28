# A query editor config retry reverts the draft on disk

- **Status**: Todo
- **Kind**: Bug
- **Authors**: jp
- **Date**: 2026-10-06
- **Label**: client=cli
- **Label**: package=jp_cli

When the "Active Configuration" block in `QUERY_MESSAGE.md` fails to parse, `jp
query` reopens the editor on the user's text with the error annotated.
If the user fixes it and the retry succeeds, `QUERY_MESSAGE.md` on disk is then
reverted to what it held before the first editor session, or deleted if it did
not exist yet.

The query is still sent correctly, because `edit_query` returns the text from
the retry session.
The damage shows up only when the turn then fails: the draft is the recovery
copy of the request, and it no longer holds what the user wrote.

## Reproduce

1. Run a bare `jp q` on a conversation with no draft.
2. Write a message, and break the TOML in the config block (for example, delete
   a closing quote).
   Save.
3. The editor reopens with the parse error.
   Fix the TOML and save.
4. Let the turn fail (for example, a model the provider rejects).
5. `QUERY_MESSAGE.md` is gone, and the next `jp q` opens an empty draft.

With an existing draft, step 5 instead shows the draft as it was before step 2.

## Cause

The retry is a recursive call made while the outer frame's `RevertFileGuard` is
still armed (`crates/jp_cli/src/editor.rs`, the `Err(error)` arm of the config
parse in `edit_query`):

```rust
let (outcome, content, mut guard) = open(query_file_path.clone(), options, editor)?;
// ...
Err(error) => {
    let error = error.to_string();
    return edit_query(config, conversation_root, stream, "", editor, Some(&error));
}
```

The inner call disarms its own guard, leaving the repaired text on disk.
When it returns, the outer frame's `guard` drops and restores the file content
from before the outer session, or removes the file if it did not exist.

Nothing writes the text back afterwards.
`Query::run_locked` skips `preserve_query_message_file` for
`QuerySource::Editor`, on the assumption that an editor-composed draft is
already on disk (`crates/jp_cli/src/cmd/query.rs`, the comment above that call).

## Fix direction

Keep one guard for the whole edit, across retries, and disarm it only when the
final session parses.
A cancel during a retry should still restore the file from before the first
session, which is what the outer guard does today and what a plain "disarm
before recursing" would lose.

Turning the recursion into a loop around `open` gives this naturally: the first
guard is kept, each retry reopens the file without taking a new one, and success
disarms it.
[RFD 080] already asks for this for `ParseOutcome::Retry`: "the protocol must
preserve the edited file contents for the next editor invocation".

Tests should drive `edit_query` with a scripted editor that breaks the TOML in
the first session and repairs it in the second, then assert the exact file
content on disk after the call returns.
Add a second case that cancels in the retry session and asserts the original
content is restored.

[RFD 080]: ../rfd/080-editor-as-a-config-source.md
