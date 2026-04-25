# RFD D28: Interactive Conversation Repair

- **Status**: Draft
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-04-15

## Summary

This RFD replaces the current all-or-nothing sanitize behavior (trash any
conversation with a corrupt file) with an interactive repair session. Each
file in a conversation directory has a different recoverability profile:
`metadata.json` and `base_config.json` can be rebuilt automatically,
`init_config.json` can be rebuilt from user-provided CLI flags, and corrupt
`events.json` can be opened in `$EDITOR` for manual fixes. Only truly
unrecoverable cases (missing `events.json`, unparseable directory names) are
trashed.

## Motivation

The current sanitize phase ([RFD 052]) treats all validation failures
identically: the conversation directory is moved to `.trash/`. This is
disproportionate for failures that are repairable.

The files in a conversation directory have different recoverability:

| File | Content | Reconstructible |
|---|---|---|
| `metadata.json` | Title, timestamps, pinned status | Yes — defaults are fine, no behavioral impact |
| `base_config.json` | Workspace config snapshot (files + env) | Yes — rebuild from current workspace config pipeline |
| `init_config.json` | Initial `--cfg` overrides as a `ConfigDelta` | Partially — user-prompted rebuild from original CLI flags |
| `events.json` | Conversation history + subsequent `ConfigDelta`s | Not automatically — user can manually fix in `$EDITOR`, otherwise trashed |

A conversation with a missing `metadata.json` but perfectly valid `events.json`
is trashed today — the user loses access to a conversation whose content is
intact. A conversation where the user introduced a trailing comma in
`events.json` while inspecting it is also trashed, despite being fixable in
seconds.

With [RFD 070] in place, `base_config.json` contains only the workspace config
snapshot (files + env) and is fully reconstructible. The initial `--cfg`
overrides live in a separate `init_config.json` and can be rebuilt by prompting
the user for the original CLI flags. Corrupt `events.json` cannot be rebuilt
automatically, but the user can be offered a chance to fix it in `$EDITOR`
before it is trashed.

## Design

### Conversation repair during sanitize

The sanitize phase becomes an interactive repair session for conversations
with corrupt files. Each file type has a different set of recovery options
available. The sanitize phase processes each corrupt conversation in turn,
applying automatic repairs where possible and prompting the user where not.

The repair options per file are:

| File | Repair options | Non-interactive fallback |
|---|---|---|
| `metadata.json` missing | Recreate with defaults | Automatic |
| `metadata.json` corrupt | Quarantine + recreate with defaults | Automatic |
| `base_config.json` corrupt | Quarantine + rebuild from workspace config | Automatic |
| `init_config.json` corrupt | Quarantine + prompt user for original CLI flags | Skip (warn, load without overrides) |
| `events.json` corrupt | Open in `$EDITOR` to fix, or trash | Trash |
| `events.json` missing | Trash | Trash |
| Directory name unparseable | Trash | Trash |

#### Automatic repairs

These require no user input and run in both interactive and non-interactive
mode:

**`metadata.json`** — Missing or corrupt metadata is replaced with
`Conversation::default()`, with `last_activated_at` set from the conversation
ID timestamp. The only loss is the conversation title and activation
timestamp, neither of which affects behavior. Corrupt files are renamed to
`metadata.corrupted.{timestamp}.json` before replacement.

**`base_config.json`** — With [RFD 070] in place, this file contains only
the workspace config snapshot (files + env). A corrupt file is renamed to
`base_config.corrupted.{timestamp}.json` and rebuilt from the current
workspace config pipeline. The only loss is if the workspace config has
changed since the conversation was created. Missing `base_config.json` is
already handled by the existing legacy fallback path.

#### Interactive repairs

These require user input and only run when a TTY is available. In
non-interactive mode, each has a defined fallback.

**`init_config.json`** — Contains the `ConfigDelta` from the first
invocation's `--cfg` arguments (e.g., `-cdev -carchitect -mgoogle`).
Corrupt files are renamed to `init_config.corrupted.{timestamp}.json`.
The user is prompted to enter the original CLI flags:

```
Conversation 17636257526-my-chat has a corrupt init_config.json.
Enter the original flags to rebuild (or press Enter to skip):
> -cdev -carchitect -mgoogle
```

The input is parsed through clap's actual argument parser (as if appended
to `jp query`), producing a real command struct with all normal config
resolution applied. The resolved overrides are written to a new
`init_config.json`. If the user presses Enter, the conversation loads
without initial overrides. Missing `init_config.json` is normal for
conversations created without `--cfg` arguments.

A future improvement could store the original config arguments in
`metadata.json` to pre-populate the prompt or auto-rebuild.

**`events.json`** — Corrupt `events.json` may be fixable: a trailing
comma, missing bracket, or truncated write are common causes of JSON parse
errors. The user is presented with a choice:

1. **Open in `$EDITOR`** — The parse error is shown, then the file opens
   in the editor. If the saved file still fails validation, the error is
   shown and the menu is presented again.
2. **Trash conversation** — Move to `.trash/` as today.

Missing `events.json` is non-repairable and always trashed.

### Sanitize report

The `SanitizeReport` captures what happened during the repair session:

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
the user must work through each one before the command runs. For non-interactive
mode this is a non-issue (automatic repairs + fallbacks run instantly), but an
interactive session with several corrupt conversations could be tedious.

**Automatic base_config rebuild may not match original.** If the workspace
config has changed since the conversation was created, the rebuilt
`base_config.json` reflects the current state, not the original. The
`ConfigDelta` events in `events.json` still apply on top, so the impact is
limited to fields that only the workspace config set.

**Clap re-parsing for init_config recovery is fragile.** The user's input is
parsed as if appended to `jp query`, but the clap argument structure may have
changed between the version that created the conversation and the current
version. Flags may have been renamed or removed. This is an edge case but
worth noting.

## Alternatives

### Keep trash-everything behavior

Do nothing. Corrupt conversations are trashed as today.

This is the simplest approach but loses recoverable data unnecessarily.
The repair session adds complexity but preserves user work.

### Repair-on-load instead of during sanitize

Make loading methods self-healing: if a file is missing or corrupt, repair
it at load time.

Rejected because it mixes read and write concerns in the `LoadBackend` trait
and makes repair happen at unpredictable times during command execution.
Sanitize is the right place — it's explicitly about making the store
consistent before any command touches it.

## Non-Goals

- **Full design of `init_config.json`.** The file split from `base_config.json`
  into workspace snapshot + initial overrides is a prerequisite for this RFD's
  repair story, but the detailed format, persistence, and migration belong in
  a dedicated RFD extending [RFD 054] and [RFD 070].

- **Config provenance storage in `metadata.json`.** Storing the original
  `--cfg` arguments for auto-rebuild is a future improvement, not a
  requirement for the interactive repair flow.

## Risks and Open Questions

### Order of repairs within a conversation

If multiple files in the same conversation are corrupt, the repair session
must decide the order. A natural order is: metadata first (needed for display),
base config second (needed for config resolution), init config third, events
last. If an earlier repair fails or the user trashes the conversation, later
repairs are skipped.

### Editor availability

The `$EDITOR` repair for `events.json` assumes an editor is configured. If
`$EDITOR` is unset, this option should either fall back to a sensible default
(e.g. `vi`) or be omitted from the menu, leaving only "trash."

## Implementation Plan

### Phase 1: Automatic repairs

Extend `validate_conversations()` to distinguish repairable from
non-repairable issues. Implement automatic repairs for metadata and base
config. Extend `SanitizeReport` with `repaired` and `degraded` fields.

**Depends on:** [RFD 070] (base config must contain only the workspace config
snapshot for the rebuild to be a valid recovery path). Also depends on the
`init_config.json` split (not yet written).
**Mergeable:** Yes.

### Phase 2: Interactive repairs

Implement the interactive repair prompts for corrupt `init_config.json`
(CLI flag entry via clap re-parsing) and corrupt `events.json` (editor loop
or trash).

**Depends on:** Phase 1.
**Mergeable:** Yes.

## References

- [RFD 052] — Workspace Data Store Sanitization. Defines the current sanitize
  and trash behavior that this RFD replaces.
- [RFD 054] — Split Conversation Config and Events. Established the three-file
  conversation storage layout.
- [RFD 070] — Negative Config Deltas. Changes `base_config.json` to contain
  only the workspace config snapshot, enabling automatic rebuild on corruption.

[RFD 052]: 052-workspace-data-store-sanitization.md
[RFD 054]: 054-split-conversation-config-and-events.md
[RFD 070]: 070-negative-config-deltas.md
