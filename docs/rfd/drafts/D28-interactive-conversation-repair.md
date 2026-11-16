# RFD D28: Interactive Conversation Repair

- **Status**: Draft
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-04-15
- **Requires**: [RFD 070]
- **Summary**: Replaces automatic trashing during workspace sanitization with
  per-file automatic reconstruction and interactive `$EDITOR` repair.

## Summary

This RFD replaces all-or-nothing conversation trashing during workspace
sanitization with per-file automatic recovery and interactive repair.
`metadata.json` and `base_config.json` are reconstructed automatically,
`init_config.json` is rebuilt from user-supplied CLI flags, and corrupt
`events.json` files open in `$EDITOR` before falling back to `.trash/`.

## Motivation

The sanitize phase ([RFD 052]) treats every validation failure identically: it
moves the conversation directory to `.trash/`.
A missing `metadata.json` trashes a conversation whose `events.json` is intact,
and a trailing comma introduced while inspecting `events.json` trashes a
conversation that takes seconds to fix.

With [RFD 070], `base_config.json` stores only the workspace config snapshot
(files + env), while initial `--cfg` overrides live in a separate
`init_config.json`.
Each file in a conversation directory has a different recovery path:

| File               | Content                                          | Reconstructible                                         |
| ------------------ | ------------------------------------------------ | ------------------------------------------------------- |
| `metadata.json`    | Title, timestamps, pinned status                 | Yes: defaults have no behavioral impact                 |
| `base_config.json` | Workspace config snapshot (files + env)          | Yes: rebuild from the current workspace config pipeline |
| `init_config.json` | Initial `--cfg` overrides as a `ConfigDelta`     | Partially: rebuild from user-supplied CLI flags         |
| `events.json`      | Conversation history + subsequent `ConfigDelta`s | No: fix manually in `$EDITOR`, otherwise trash          |

## Design

### Conversation repair during sanitize

During sanitization, JP processes each corrupt conversation in turn, applying
automatic repairs where possible and prompting the user when a TTY is available:

| File                       | Repair options                                  | Non-interactive fallback            |
| -------------------------- | ----------------------------------------------- | ----------------------------------- |
| `metadata.json` missing    | Recreate with defaults                          | Automatic                           |
| `metadata.json` corrupt    | Quarantine + recreate with defaults             | Automatic                           |
| `base_config.json` corrupt | Quarantine + rebuild from workspace config      | Automatic                           |
| `init_config.json` corrupt | Quarantine + prompt user for original CLI flags | Skip (warn, load without overrides) |
| `events.json` corrupt      | Open in `$EDITOR` to fix, or trash              | Trash                               |
| `events.json` missing      | Trash                                           | Trash                               |
| Directory name unparseable | Trash                                           | Trash                               |

#### Automatic repairs

Automatic repairs require no user input and run in both interactive and
non-interactive modes:

**`metadata.json`**: Corrupt files are renamed to
`metadata.corrupted.{timestamp}.json`, and missing or corrupt metadata is
replaced with `Conversation::default()` using the conversation ID timestamp for
`last_activated_at`.
Only the conversation title and activation timestamp are lost, neither of which
affects behavior.

**`base_config.json`**: Corrupt files are renamed to
`base_config.corrupted.{timestamp}.json` and rebuilt from the current workspace
config pipeline ([RFD 070]).
Rebuilding diverges from the original snapshot only if the workspace config has
changed since the conversation was created.
Missing `base_config.json` files continue to use the legacy fallback path.

#### Interactive repairs

Interactive repairs require a TTY; non-interactive mode uses each file's
fallback behavior.

**`init_config.json`**: Stores the `ConfigDelta` from the first invocation's
`--cfg` arguments (e.g., `-cdev -carchitect -mgoogle`).
Missing `init_config.json` is normal for conversations created without `--cfg`
arguments.
When the file is corrupt, JP renames it to
`init_config.corrupted.{timestamp}.json` and prompts for the original CLI flags:

```
Conversation 17636257526-my-chat has a corrupt init_config.json.
Enter the original flags to rebuild (or press Enter to skip):
> -cdev -carchitect -mgoogle
```

JP parses the input through clap's argument parser as if appended to `jp query`,
applies normal config resolution, and writes the resolved overrides to a new
`init_config.json`.
Pressing Enter skips recovery and loads the conversation without initial
overrides.

**`events.json`**: JSON parse errors in `events.json` often come from a trailing
comma, missing bracket, or truncated write.
A missing `events.json` cannot be repaired and is always trashed.
When the file is corrupt, JP prompts the user to choose:

1. **Open in `$EDITOR`**: Show the parse error and open `events.json` in the
   editor.
   If the saved file still fails validation, JP shows the new error and presents
   the menu again.
2. **Trash conversation**: Move the conversation directory to `.trash/`.

### Sanitize report

`SanitizeReport` records the outcome of the repair session:

```rust
pub struct SanitizeReport {
    /// Conversations that were repaired (automatically or interactively).
    pub repaired: Vec<RepairedConversation>,

    /// Conversations that were trashed (unrecoverable or user chose trash).
    pub trashed: Vec<TrashedConversation>,

    /// Conversations with corrupt init_config.json that the user skipped
    /// (non-interactive mode or user pressed Enter). These conversations
    /// load without their initial config overrides.
    pub degraded: Vec<DegradedConversation>,
}
```

The CLI logs a summary after the repair session:

```
WARN Repaired conversation 17636257526: recreated missing metadata.json
WARN Repaired conversation 17636257526-my-chat: rebuilt base_config.json from workspace config
WARN Degraded conversation 17636257526-my-chat: loaded without init_config.json overrides
WARN Trashed corrupt conversation 17636257526-my-chat: events.json: expected array
```

## Drawbacks

**Interactive sanitize blocks startup.** If multiple conversations are corrupt,
the user must work through each one before the command runs.
Non-interactive mode runs automatic repairs and fallbacks immediately, but an
interactive session across several corrupt conversations can be tedious.

**Automatic `base_config` rebuild may not match the original.** If the workspace
config has changed since the conversation was created, the rebuilt
`base_config.json` reflects the current state rather than the original snapshot.
Subsequent `ConfigDelta` events in `events.json` still apply on top, so only
fields set exclusively by the workspace config are affected.

**Clap re-parsing for `init_config` recovery is fragile.** User input is parsed
as if appended to `jp query`, so flags renamed or removed since the conversation
was created fail to parse.

## Alternatives

### Keep trash-everything behavior

Continue moving every corrupt conversation to `.trash/`.
This is the simplest option, but it discards recoverable user data.

### Repair-on-load instead of during sanitize

Make `LoadBackend` methods self-healing by repairing missing or corrupt files at
load time.

Rejected because it mixes read and write concerns in the `LoadBackend` trait and
triggers repairs at unpredictable points during command execution.
The sanitize phase exists specifically to make the store consistent before any
command touches it.

## Non-Goals

- **Full design of `init_config.json`.** Splitting `base_config.json` into a
  workspace snapshot and initial overrides is a prerequisite for this RFD, but
  its format, persistence, and migration belong in a dedicated RFD extending
  [RFD 054] and [RFD 070].

- **Config provenance storage in `metadata.json`.** Storing the original `--cfg`
  arguments in `metadata.json` to pre-populate the prompt or auto-rebuild
  `init_config.json` is a future improvement, not a requirement for the
  interactive repair flow.

## Risks and Open Questions

### Order of repairs within a conversation

When multiple files in a conversation are corrupt, the repair session runs in
dependency order: metadata first (needed for display), base config second
(needed for config resolution), init config third, and events last.
If an earlier repair fails or the user trashes the conversation, the remaining
repairs are skipped.

### Editor availability

Repairing `events.json` in `$EDITOR` requires a configured editor.
If `$EDITOR` is unset, the menu should either fall back to a default editor
(e.g. `vi`) or omit the editor option, leaving only "trash."

## Implementation Plan

### Phase 1: Automatic repairs

Extend `validate_conversations()` to distinguish repairable from non-repairable
issues.
Implement automatic repairs for metadata and base config.
Extend `SanitizeReport` with `repaired` and `degraded` fields.

**Depends on:** [RFD 070] (so `base_config.json` contains only the workspace
config snapshot) and the `init_config.json` split (not yet written).
**Mergeable:** Yes.

### Phase 2: Interactive repairs

Implement the interactive repair prompts for corrupt `init_config.json` (CLI
flag entry via clap re-parsing) and corrupt `events.json` (editor loop or
trash).

**Depends on:** Phase 1.
**Mergeable:** Yes.

## References

- [RFD 052]: Workspace Data Store Sanitization.
  Defines the sanitize and trash behavior that this RFD replaces.
- [RFD 054]: Split Conversation Config and Events.
  Establishes the three-file conversation storage layout.
- [RFD 070]: Negative Config Deltas.
  Restricts `base_config.json` to the workspace config snapshot, enabling
  automatic rebuild on corruption.

[RFD 052]: ../052-workspace-data-store-sanitization.md
[RFD 054]: ../054-split-conversation-config-and-events.md
[RFD 070]: ../070-negative-config-deltas.md
