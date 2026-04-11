# RFD 047: Editor and Path Access for Conversations

- **Status**: Implemented
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-03-16

## Summary

This RFD adds two capabilities: `jp conversation edit` without flags opens the
conversation directory in `$EDITOR`, and `jp conversation path` prints the
filesystem path to a conversation. Both commands accept `--events`,
`--metadata`, and `--base-config` flags to target specific files. These provide
direct filesystem access to conversation data — the missing complement to JP's
existing JSON-based manual editing workflow.

## Motivation

Manually editing conversation JSON files is a core JP workflow. Users tweak
context windows, remove noisy tool calls, edit responses, and trim history by
editing `events.json` directly. Today, this requires knowing the conversation's
storage path and navigating to it manually.

With conversation trees ([RFD 039], [RFD 046]), the storage path may be nested
and non-obvious. Even with flat storage, editing a local-only conversation
requires a different path than workspace conversations. A first-class command
that opens the right file in the user's editor — or prints the path for use in
shell pipelines — removes this friction.

Currently, `jp conversation edit` requires a flag (`--local`, `--title`,
`--tmp`, etc.) to determine what property to edit. Without a flag, it shows
help. This wastes the bare `jp conversation edit` invocation on a help screen
when it could do the most common thing: open the conversation in `$EDITOR` for
editing.

## Design

### `jp conversation edit` (no property flags)

When invoked without any of the existing property flags (`--local`, `--title`,
`--tmp`, etc.), `jp conversation edit` opens the conversation directory in the
user's configured editor:

```sh
# Opens $EDITOR with CWD set to .jp/conversations/<id>/
jp conversation edit

# Same, for a specific conversation
jp conversation edit jp-c1234
```

The editor is resolved using `EditorConfig` (checking `JP_EDITOR`, `VISUAL`,
`EDITOR` in order). The editor is invoked with the conversation directory as its
argument. Most editors (VS Code, Vim, Neovim, Emacs, Sublime Text) open a
directory as a file browser or project root.

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

Flags can be combined to open multiple files:

```sh
# Opens both files in $EDITOR
jp conversation edit jp-c1234 --events --metadata
# equivalent to: $EDITOR events.json metadata.json
```

When any file flag is provided, the editor is invoked with the file path(s) as
arguments (not the directory).

### Interaction with property flags

The file flags (`--events`, `--metadata`, `--base-config`) are in a separate
clap group from the existing property flags (`--local`, `--title`, `--tmp`,
`--no-tmp`, `--no-title`). The two groups conflict:

```sh
# Valid: open events.json in editor
jp conversation edit --events

# Valid: toggle local flag (existing behavior)
jp conversation edit --local

# Invalid: can't combine file opening with property mutation
jp conversation edit --events --local
```

The existing property flags continue to work as they do today. The only
behavioral change is that `jp conversation edit` with no flags now opens the
directory instead of showing help.

### `jp conversation path`

A new subcommand that prints the filesystem path to a conversation:

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

Paths are printed to stdout, one per line. This enables shell composition:

```sh
vim $(jp conversation path --id=prev --base-config)
cat $(jp conversation path --metadata) | jq .
cp $(jp conversation path --events) /tmp/backup.json
```

Without any flag, the directory path is printed.

### Path resolution

Both commands resolve the conversation's storage path through the existing
`find_conversation_dir_path` function in `jp_storage`. This returns the
workspace path for projected conversations and the user-local path for
local-only conversations. With [RFD 046]'s nested workspace projection, the
resolved path follows the nested structure automatically.

## Drawbacks

**`edit` default behavior change.** Users who relied on `jp conversation edit`
showing help (as a reminder of available flags) will now get an editor opening
instead. This is a minor surprise but the new behavior is more useful. `jp
conversation edit --help` still shows help.

## Alternatives

### Open `events.json` by default instead of the directory

Make the bare `jp conversation edit` open `events.json` directly.

Not adopted because opening the directory lets the user choose which file to
edit from their editor's file browser. This is more flexible and doesn't assume
the user always wants `events.json`.

## Non-Goals

- **In-place structured editing.** This RFD opens files in an external editor.
  It does not add interactive TUI editing or JSON-aware editing within JP.

## Implementation Plan

### Phase 1: `jp conversation path`

Add the `path` subcommand to `jp conversation`. Implement path resolution using
`find_conversation_dir_path`. Add `--events`, `--metadata`, `--base-config`
flags.

Can be merged independently.

### Phase 2: `jp conversation edit` with file/directory opening

Remove the `arg_required_else_help` attribute from the `Edit` struct. Add the
`--events`, `--metadata`, `--base-config` flag group. When no property flags are
given, resolve the conversation path and invoke the editor. Use
`EditorConfig::command()` to get the editor, invoke it with the resolved
path(s).

Depends on Phase 1 (shares path resolution logic).

## References

- [RFD 039: Conversation Trees][RFD 039] — flat storage layout where `jp
  conversation path` resolves to `.jp/conversations/<id>/`.
- [RFD 046: Nested Workspace Projection][RFD 046] — nested workspace layout
  where paths may be deeper.
- `crates/jp_cli/src/cmd/conversation/edit.rs` — current edit implementation.
- `crates/jp_editor/src/lib.rs` — editor backend abstraction.
- `crates/jp_config/src/editor.rs` — editor configuration and resolution.
- `crates/jp_storage/src/lib.rs` — `find_conversation_dir_path` for path
  resolution.

[RFD 039]: 039-conversation-trees.md
[RFD 046]: 046-nested-workspace-projection.md
