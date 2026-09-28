# Plugin Workspace Scope and Addressing

- **Status**: Todo
- **Kind**: Feature
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-09-28
- **Implements**: 114
- **Label**: type=tracking

Tracking ticket for [RFD 114](../rfd/114-plugin-workspace-scope-and-addressing.md).

## Implementation plan

- **Add the last-root record**: Introduces `metadata.local.json` in the conversation's user-local directory with unknown-key preservation, makes projection sync replace only managed files, and writes `last_root` when a turn first persists. Reports `last_root` on `list_conversations` and `read_events`, adds `expected_last_root` to `query` with a lock-time check, and has `jp-serve-web` handle a dead record by sending the confirmation.
- **Declare plugin workspace scope**: Adds `workspace_scope` to `DescribeResponse` (default `single`) and derives the plugin dispatch's `WorkspaceRequirement` from it. Admits the plugin under the discovery configuration before `describe`, and again under the selected workspace's configuration for `single`, makes `init`'s workspace/config fields conditional on scope, and rejects a plugin whose declaration disagrees with its requests.
- **Expose list_workspaces**: Projects `roots::known_workspaces` onto the wire, including per-root storage paths and the `launch` marker, giving a frontend enough to show a workspace list.
- **Wire up addressing and the workspace registry**: Adds `WorkspaceRef` to data and mutation requests (requiring `root` when a workspace has multiple live checkouts), lazily populates `WorkspaceCtx` per addressed checkout with per-checkout lock tracking, routes background tasks' destinations through the registry, implements idle-shutdown for unleased MCP server instances, and updates `jp-serve-web` to declare `multi`, address explicitly, and continue conversations via `last_root`/`expected_last_root`.
