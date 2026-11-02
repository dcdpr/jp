# In-Repo Project Wiki

- **Status**: Todo
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-10-31
- **Label**: tracking-rfd=127
- **Label**: type=tracking

Tracking ticket for [RFD 127].

## Implementation plan

- **Extract shared document substrate**: Moves metadata parsing/rendering,
  parameterized 7-character epoch ID allocation, reference rewriting, and
  heading/anchor extraction into a shared internal crate used by `ticket` and
  `jp-rfd`.
  Mergeable independently with no behavioral change.
- **Build jp-wiki core and CLI**: Creates `crates/plugins/command/wiki`
  (`jp-wiki`) with `init`, `add`, `show` (with `--section` and transclusion
  expansion), `list`, `edit`, `rename`, `rank`, `archive`, `pin`, `from-ticket`,
  and `check` (metadata, labels, `#section` anchors, workspace file/line/symbol
  links, permalinks).
  Depends on Phase 1.
- **Add wiki assistant tools**: Adds the `wiki_list`, `wiki_show`,
  `wiki_create`, and `wiki_edit` tools, plus `jp wiki tools init` and `jp wiki
  tools sync`.
  Depends on Phase 2.
- **Integrate wiki into jp-vite**: Extends `jp-vite` with wiki support:
  `settings.rs`, `references.mjs`, `redirects.mjs` and the `redirects` map,
  `theme/documents.mjs`, `middleware.mjs`, `<WikiIndex />`, the owned slug
  function, the transclusion markdown rule, and the auto-generated `/wiki/`
  sidebar.
  Depends on Phase 2.
- **Migrate JP's documents into the wiki**: Moves each `docs/architecture/` file
  into `docs/wiki/` as one page, keeping headings so anchors carry over.
  Rewrites references to old paths across the whole workspace (including
  `.jp/config/` and `jp_cli` doc comments), reports any left unrewritten,
  redirects old `/architecture/*` routes, and moves research-note tickets such
  as `T-0n0nwz3` with `jp wiki from-ticket`.
  Depends on Phase 4.

[RFD 127]: ../rfd/127-in-repo-project-wiki.md
