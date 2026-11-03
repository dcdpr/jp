# Workspace Source Explorer in jp-vite

- **Status**: Todo
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-10-31
- **Label**: tracking-rfd=128
- **Label**: type=tracking

Tracking ticket for [RFD 128].

## Implementation plan

- **Validate workspace links and line ranges**: Update `rewriteRepositoryLinks`
  in `jp-vite/config.mjs` to classify links by target and check that source
  links and their `#L<start>-L<end>` ranges exist on disk, in build and dev
  rendering.
  Also extends the `docs.yml` trigger so pull requests that change, rename, or
  delete a linked workspace file run the docs build.
  Mergeable independently.
- **Add SourceExplorer and dev-server /__source endpoint**: Adds
  `source/index.md` to `jp vite sync`, implements `<SourceExplorer />` (file
  tree, Shiki highlighting, line-range highlighting and selection, `Referenced
  by` badges), and registers `/__source` in `middleware.mjs`.
  Depends on Phase 1.
- **Emit source files in the static build**: Writes `source-index.json` and
  `source-files/` in `withJpVite`'s `buildEnd` hook when `source.publish` is on,
  and points eligible source links at `/source/?path=<path>#L...` in static
  builds.
  Sets `source.publish = true` in JP's `.jp/config.toml`.
  Depends on Phase 2.

[RFD 128]: ../rfd/128-workspace-source-explorer-in-jp-vite.md
