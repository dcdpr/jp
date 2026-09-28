# Context Decomposition for Invocation and Workspace Scope

- **Status**: Todo
- **Kind**: Feature
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-09-28
- **Implements**: 113
- **Label**: type=tracking

Tracking ticket for [RFD 113](../rfd/113-context-decomposition-for-invocation-and-workspace-scope.md).

## Implementation plan

- **Introduce the two types and migrate plugin dispatch**: Adds `CliCtx` and `WorkspaceCtx` with `Ctx` as a shell holding both. Resolves delegated-turn configuration through `ConfigPipeline` per checkout, deletes `Ctx::swap_config`, keys MCP server instances by server/configuration/spawn directory with per-turn leasing (deleting `configure_active_mcp_servers`, `set_servers`, `McpServerScope`), warns instead of silently skipping misdirected background tasks, and migrates `run_external`, `run_plugin`, `message_loop`, and plugin request handlers to the two halves. Mergeable independently; unblocks multi-workspace addressing.
- **Migrate remaining commands off the shell**: Moves every other command from the `Ctx` shell to `CliCtx`/`WorkspaceCtx`, one group at a time, then deletes the shell. Depends on Phase 1; each command group is independently mergeable.
- **Record the inventory and enact the placements it forces**: Documents the application/frontend capability split under `docs/architecture/`, consolidates query-draft handling into the storage layer (one path resolution shared by CLI and FFI, preserving both conflict policies), and removes `cfg` from `PluginToHost::Query` and `list_configs` from the protocol surface (still deserializable, but answered with a correlated error so no turn starts under a dropped config selection). Depends on Phase 1; each item is independently mergeable and is its own behavior-change review.
