# RFD 047: Editor and Path Access for Conversations

- **Status**: Implemented
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-03-16
- **Summary**: Opens conversation files in `$EDITOR` via bare `jp conversation
  edit` and prints their paths via `jp conversation path`.

## Summary

Bare `jp conversation edit` opens a conversation's directory in `$EDITOR`, and
`jp conversation path` prints its filesystem path.
Both commands accept `--events`, `--metadata`, and `--base-config` flags to
target individual files.

## Motivation

Editing conversation JSON files by hand is a core JP workflow: users tweak
context windows, remove noisy tool calls, edit responses, and trim history in
`events.json`.
Doing so requires finding the conversation's storage path and navigating to it
manually.

Conversation trees ([RFD 039], [RFD 046]) can nest storage paths deeply, and
even flat storage places local-only and workspace conversations in different
directories.
First-class commands that open the right file in the user's editor or print its
path for shell pipelines remove this friction.

Without a property flag (`--local`, `--title`, `--tmp`, etc.), `jp conversation
edit` only prints help, wasting the bare command when it could open the
conversation in `$EDITOR`.

## Design

### `jp conversation edit` (no property flags)

Without property flags (`--local`, `--title`, `--tmp`, etc.), `jp conversation
edit` opens the conversation directory in the configured editor:

```sh
# Opens $EDITOR with CWD set to .jp/conversations/<id>/
jp conversation edit

# Same, for a specific conversation
jp conversation edit jp-c1234
```

`EditorConfig` resolves the editor by checking `JP_EDITOR`, `VISUAL`, and
`EDITOR` in order, then invokes it with the conversation directory as its
argument.
Most editors (VS Code, Vim, Neovim, Emacs, Sublime Text) open a directory as a
file browser or project root.

### File-specific flags

Three flags target specific files within the conversation directory:

```sh
# Open events.json
jp conversation edit --events
jp conversation edit jp-c1234 --events

# Open metadata.json
jp conversation edit --metadata
jp conversation edit jp-c1234 --metadata

# Open base_config.json
jp conversation edit --base-config
jp conversation edit jp-c1234 --base-config
```

Combining flags opens multiple files:

```sh
# Opens both files in $EDITOR
jp conversation edit jp-c1234 --events --metadata
# equivalent to: $EDITOR events.json metadata.json
```

With any file flag set, JP passes the selected file paths to the editor instead
of the directory.

### Interaction with property flags

The file flags (`--events`, `--metadata`, `--base-config`) form a separate clap
group that conflicts with the property flags (`--local`, `--title`, `--tmp`,
`--no-tmp`, `--no-title`):

```sh
# Valid: open events.json in editor
jp conversation edit --events

# Valid: toggle local flag (existing behavior)
jp conversation edit --local

# Invalid: can't combine file opening with property mutation
jp conversation edit --events --local
```

Property flags are unchanged; only bare `jp conversation edit` changes from
printing help to opening the conversation directory.

### `jp conversation path`

`jp conversation path` prints a conversation's directory path, or the paths to
specific files when passed `--events`, `--metadata`, or `--base-config`:

```sh
# Print the conversation directory path
$ jp conversation path
.jp/conversations/17528832001-refactor-error-handling/

# Print for a specific conversation
$ jp conversation path jp-c1234
.jp/conversations/17528832001-refactor-error-handling/

# Print path to events.json
$ jp conversation path --events
.jp/conversations/17528832001-refactor-error-handling/events.json

# Print path to metadata.json
$ jp conversation path --metadata
.jp/conversations/17528832001-refactor-error-handling/metadata.json

# Print multiple paths
$ jp conversation path --events --metadata
.jp/conversations/17528832001-refactor-error-handling/events.json
.jp/conversations/17528832001-refactor-error-handling/metadata.json
```

The command writes one path per line to stdout for shell composition:

```sh
vim $(jp conversation path --id=prev --base-config)
cat $(jp conversation path --metadata) | jq .
cp $(jp conversation path --events) /tmp/backup.json
```

### Path resolution

Both commands resolve the conversation directory via
`find_conversation_dir_path` in `jp_storage`, which returns the workspace path
for projected conversations and the user-local path for local-only
conversations.
Under [RFD 046]'s nested workspace projection, the resolved path follows the
nested structure automatically.

## Drawbacks

**`edit` default behavior change.** Running bare `jp conversation edit` opens
`$EDITOR` instead of printing the flag list as help.
`jp conversation edit --help` still shows help.

## Alternatives

### Open `events.json` by default

Bare `jp conversation edit` could open `events.json` directly instead of the
conversation directory.
Opening the directory is more flexible: the user can pick any conversation file
from their editor's file browser rather than always landing in `events.json`.

## Non-Goals

- **In-place structured editing.** This RFD opens files in an external editor;
  it does not add interactive TUI editing or JSON-aware editing within JP.

## Implementation Plan

### Phase 1: `jp conversation path`

Add the `path` subcommand to `jp conversation` with `--events`, `--metadata`,
and `--base-config` flags, resolving paths via `find_conversation_dir_path`.
This phase can merge independently.

### Phase 2: `jp conversation edit` with file/directory opening

Remove the `arg_required_else_help` attribute from the `Edit` struct and add the
`--events`, `--metadata`, and `--base-config` flag group.
When no property flags are given, resolve the target paths (shared with Phase 1)
and pass them to `EditorConfig::command()`.

## References

- [RFD 039: Conversation Trees][RFD 039]: flat storage layout where `jp
  conversation path` resolves to `.jp/conversations/<id>/`.
- [RFD 046: Nested Workspace Projection][RFD 046]: nested workspace layout where
  paths may be deeper.
- `crates/jp_cli/src/cmd/conversation/edit.rs`: `edit` implementation.
- `crates/jp_editor/src/lib.rs`: editor backend abstraction.
- `crates/jp_config/src/editor.rs`: editor configuration and resolution.
- `crates/jp_storage/src/lib.rs`: `find_conversation_dir_path` for path
  resolution.

[RFD 039]: 039-conversation-trees.md
[RFD 046]: 046-nested-workspace-projection-for-conversation-trees.md
