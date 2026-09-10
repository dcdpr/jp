# Tool Access Grants

- **Status**: Todo
- **Kind**: Feature
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-09-10
- **Implements**: 076
- **Label**: type=tracking

Tracking ticket for [RFD 076].

## Implementation plan

- **Add access policy types and evaluation to jp_tool**: Introduces
  `AccessPolicy`, `FsRule`, `NetRule`, `EnvRule`, and `FsAccessError`, plus the
  path-canonicalization helper and `Context::check_*` methods.
  Implements structured net matching with host normalization and explicit-`*`
  env prefix matching, with unit tests for workspace escape, symlinks, host
  normalization, port defaulting, and env ties.
  No dependency; can merge independently.
- **Add config types in jp_config**: Adds `AccessConfig`, `FsRuleConfig`,
  `NetRuleConfig`, `EnvRuleConfig` with `MergeableVec` wrappers and standard
  partial/delta/`ToPartial` impls, plus an `access` field on `ToolConfig` and an
  accessor on `ToolConfigWithDefaults`.
  Implements the `AccessConfig` to `AccessPolicy` conversion, canonicalizing
  rule paths and normalizing hosts, and adds post-merge validation rejecting
  `access` on builtin or mcp tools.
  Depends on Phase 1.
- **Plumb access policy through jp_llm**: Includes the access policy in the JSON
  context passed to tool commands in `execute_local()` and the `FormatArguments`
  path.
  Depends on Phase 2.
- **Enforce access checks in fs_* tools**: Replaces ad-hoc path joining in the
  `fs_*` tool family with `ctx.check_read()`, `check_create()`,
  `check_update()`, `check_delete()`, and `check_execute()`, returning clear
  denial errors that name the capability and list configured grants.
  Keeps `unix_utils` out of scope for grant enforcement, only wiring its
  existing argument scanner to the shared canonicalization helper.
  Depends on Phase 3.
- **Support wildcard-scope grants and fix --mount**: Adds `access` to
  `ToolsDefaultsConfig` and resolves it under the replace rule in
  `ToolConfigWithDefaults::access()`, skipping builtin and MCP tools, and
  narrows post-merge validation to tool-declared access.
  Updates `--mount` so injected rules preserve a tool's prior reach across both
  scope and resource-type edges instead of silently widening or revoking access.
  Depends on Phase 2; independent of Phases 3 and 4.

[RFD 076]: ../rfd/076-tool-access-grants.md
