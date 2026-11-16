# RFD 128: Workspace Source Explorer in jp-vite

- **Status**: Accepted
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-10-30
- **Extends**: [RFD 116], [RFD 127]
- **Summary**: Serves syntax-highlighted workspace source files with line-range
  deep links and reverse document backlinks in `jp-vite`.

## Summary

This RFD adds a hosted workspace source explorer to `jp-vite` at `/source/`.
Relative links from RFDs, tickets, and wiki pages to repository files and line
ranges (such as `../../crates/jp_llm/src/lib.rs#L12-L24`) resolve inside the
site instead of opening on GitHub.
The explorer highlights linked line ranges with Shiki and lists every RFD,
ticket, and wiki page that references the file.

## Motivation

RFDs, tickets, and wiki pages frequently link to workspace source files and
directories using relative paths (for example,
`../../crates/plugins/command/rfd/src/links.rs#L88-L125`).
`rewriteRepositoryLinks` in `jp-vite/config.mjs` rewrites any relative link that
resolves outside the VitePress source directory (`docs/`) into an external
GitHub URL (`${repository}/blob/HEAD/<path>#L88-L125`).

Rewriting workspace links to external GitHub URLs has four limitations:

1. **Broken or stale links on active branches.** Under `jp vite serve`, links to
   files added on the current branch return 404 against GitHub `HEAD`, and
   line-range links into modified files highlight the wrong lines because `HEAD`
   on `origin` does not match the local working tree.
2. **No offline or forge-free browsing.** Contributors working offline and
   projects without a configured remote cannot follow source links from the
   site.
3. **Silent dead links.** Because `rewriteRepositoryLinks` converts workspace
   links into external URLs at parse time, VitePress skips them during dead-link
   checking.
   Deleting or moving a referenced source file never fails `vitepress build`.
4. **One-way navigation.** Following a GitHub link leaves the documentation
   site.
   A contributor reading a source file cannot see which RFDs, tickets, or wiki
   pages reference it.

Hosting a source explorer in `jp-vite` solves all four problems without changing
how links are written in markdown.

## Design

### Overview

When the source explorer is enabled (the default), `jp-vite` serves a `/source/`
route that renders workspace files and directories inside the VitePress theme.
The dev server always serves `/source/` when enabled; static builds include it
only when source publishing is enabled (see [Configuration](#configuration)).

1. **Existing markdown links work unchanged.** Documents continue to use
   standard relative links:

   ```markdown
   [`Links::by_rfd`](../../crates/plugins/command/rfd/src/links.rs#L88-L125)
   ```

   In `$EDITOR` and on a Git forge, that relative path resolves directly to the
   file.
   In `jp-vite`, `rewriteRepositoryLinks` rewrites it to
   `/source/?path=crates/plugins/command/rfd/src/links.rs#L88-L125`.

2. **Line-range highlighting and scrolling.** URL fragments of the form `#L12`,
   `#L12-L24`, `#L12-24`, and `#12-24` highlight the target lines and scroll the
   first highlighted line into view on load.
   Clicking a line number in the gutter updates the URL hash (`#L12`), and
   Shift-clicking selects a range (`#L12-L24`) that can be copied into a
   document.

3. **Document backlinks (`Referenced by`).** Above the file content,
   `<RfdReferences />` lists every RFD, ticket, and wiki page that links to the
   file or one of its parent directories, along with the line ranges each
   document cites.

4. **External forge link.** When `repository` is configured, the file header
   includes a `GitHub ↗` link to `${repository}/blob/HEAD/<path>#L...`.

### Configuration

`plugins.command.vite.options` gains a `source` table:

```toml
[plugins.command.vite.options]
dir = "docs"
repository = "https://github.com/dcdpr/jp"

# Serve eligible workspace files at `/source/` on the dev server, and route
# source links there. When `false`, source links point at `repository`.
source.enable = true

# Also write eligible files into the static build, publishing them with the
# site. Off by default.
source.publish = true

# Files the explorer never serves or publishes, as globs relative to the
# workspace root.
source.exclude = ["crates/**/fixtures/**"]
```

`enable` defaults to `true`, `publish` defaults to `false`, and `exclude`
defaults to `[]`.
JP's own configuration sets `publish = true`.

`enable` and `publish` are separate settings because local development and
deployed static builds have different visibility guarantees.
The dev server runs locally, so serving tracked files there does not expose
repository source code externally.
A static build is deployed to everyone who can access the documentation site,
which may be public even when the repository is private.
Keeping `publish` off by default requires projects to opt in before bundling
tracked source files into the static site.
When `publish` is `false`, static builds rewrite source links to `repository`,
preserving the repository's access controls.

`jp vite sync` writes all three settings to
`docs/.vitepress/jp-vite/settings.json`.

### Eligible Files

The tree endpoint, file endpoint, static build emitter, and link rewriter all
apply the same eligibility rules.
A file is eligible when it satisfies all of the following:

1. **Tracked by Git** (`git ls-files --cached`).
   Untracked secrets, `.env` files, and `target/` artifacts are never served or
   published, regardless of ignore rules.
2. **Not matched by `source.exclude`.** The explorer ignores `.ignore` files,
   which `rg` and JP's file walkers read, so excluding a fixture from the
   documentation site does not hide it from local search.
3. **A regular file, not a symlink.** Rejecting symlinks prevents a tracked link
   from serving files outside the workspace.
4. **Text content.** If a file contains a null byte in its first 8,192 bytes,
   the explorer renders a binary-file notice with the file size instead of
   source lines.

On the dev server, the explorer reads eligible files from the working tree, so
uncommitted edits appear immediately.
Newly created files become eligible once staged with `git add`.

A directory is eligible if it contains at least one eligible file.
Links to existing but ineligible files or directories are still validated on
disk, but rewrite to `repository` instead of `/source/`.

### Architecture: Dev Server vs. Static Build

Generating a separate Vue SSR page per source file would add thousands of pages
to `vitepress build` in medium-sized repositories.
Instead, `jp-vite` uses a single `/source/` viewer component (`<SourceExplorer
/>`) backed by on-demand loading in the dev server and static JSON chunks in
production builds:

```
Markdown link: ../../crates/jp_llm/src/lib.rs#L12-L24
                     │
                     ▼
        rewriteRepositoryLinks (config.mjs)
        ├── Verify target path & line range on disk (fails build if missing)
        └── Rewrite href -> /source/?path=crates/jp_llm/src/lib.rs#L12-L24
                     │
                     ▼
             <SourceExplorer />
        ├── Directory tree sidebar (collapsible workspace tree)
        ├── Header: breadcrumb path + "Referenced by" badges + "GitHub ↗"
        └── Code view: line numbers + Shiki syntax highlighting + active #L range
```

#### Dev Server (`jp vite serve`)

`middleware.mjs` registers `/__source`:

- `GET /__source?tree=1` returns the tree of eligible files and the reverse
  reference map linking workspace paths to the RFDs, tickets, and wiki pages
  that cite them.
- `GET /__source?path=<workspace-relative-path>` reads an eligible file from the
  working tree, highlights it with VitePress's Shiki instance, and rejects
  ineligible or out-of-workspace paths.
  Edits saved in `$EDITOR` appear immediately on page refresh with updated line
  numbers.

#### Static Build (`vitepress build`)

When `source.publish` is enabled, the `buildEnd` hook (which also copies raw
`.md` files and generates RFD redirects) emits static assets for the explorer:

- `jp-vite` writes `outDir/source-index.json` (the eligible file tree and
  reverse reference map) and writes each eligible file to
  `outDir/source-files/<path>.json` (containing the Shiki HTML or raw lines).
- `<SourceExplorer />` fetches `source-index.json` and the requested file's JSON
  chunk on navigation, so `vitepress build` compiles only a single Vue page
  (`source/index.md`) regardless of repository size.

### Build-Time Link and Line-Range Validation

`rewriteRepositoryLinks` runs on every markdown token across the VitePress site,
including RFDs, tickets, wiki pages, and user documentation.
It classifies each relative link by its resolved target rather than by whether
the path resolves outside the site directory:

- A target that resolves to a markdown page in the site or a file under
  `public/` belongs to the documentation site and is left to VitePress.
- Any other relative target that resolves inside the workspace is a **source
  link**, even if it sits inside `settings.site` (for example, when the site
  root is the workspace root, `dir = "."`).

Every source link is validated and rewritten as follows:

1. **Target existence**: If the file or directory does not exist on disk, the
   build fails with an error naming the markdown page and the missing target.
2. **Line-range bounds**: If the link includes a `#L<start>[-L<end>]` fragment,
   the target must be a regular file and satisfy `1 <= start <= end <=
   line_count`, or the build fails.
3. **Link target**: If the target is eligible and the explorer is active for the
   current mode, the link rewrites to `/source/?path=...`; otherwise it rewrites
   to `repository`.

Validation runs regardless of `source.enable` and `source.publish`, so broken
paths and out-of-bounds line ranges fail the build even when the explorer is
disabled or unpublished.

### Reverse Reference Index

The data loaders for RFDs, tickets, and wiki pages scan document bodies for
cross-document citations via `referencedLabels` in `rfd-shared.mjs`.
A companion function, `referencedFiles(content, pageDir, settings)`, extracts
every workspace-relative link and its optional `#L<start>-L<end>` range:

```json
{
  "crates/plugins/command/rfd/src/links.rs": [
    { "label": "RFD 116", "path": "/rfd/116-shareable-rfd-ticket-and-site-tooling", "lines": null },
    { "label": "W-0b42fs5", "path": "/wiki/0b42fs5-query-stream-pipeline", "lines": "L88-L125" }
  ]
}
```

When `<SourceExplorer />` displays `crates/plugins/command/rfd/src/links.rs`,
the header renders these entries using the same status-colored pill badges as
`<RfdReferences />`.
Clicking or hovering over a badge with `"lines": "L88-L125"` shows the line
range cited by that document.

## Drawbacks

- **Publishing includes all eligible files.** Enabling `source.publish` bundles
  every eligible tracked file into the deployed site, including files that no
  document links to.
  `source.exclude` is the only mechanism to omit tracked files from published
  output.
- **Larger static build output.** Writing every eligible file into
  `outDir/source-files/` increases the built site size proportionally to the
  repository's tracked text (a few megabytes for JP).
  Projects with large tracked fixtures must exclude them via `source.exclude`.
- **Untracked files require `git add`.** Because only Git-tracked files are
  eligible, newly created files on a branch do not appear in the dev explorer
  (and links to them rewrite to `repository`) until staged with `git add`.
- **Query-parameter URLs on static hosts.** Static hosts such as GitHub Pages
  cannot serve arbitrary `/source/crates/foo/bar.rs` paths without generating an
  HTML stub per file.
  Routing via `/source/?path=crates/foo/bar.rs#L12-L24` works on every static
  host from a single `/source/index.html` page, at the cost of using a query
  parameter instead of a clean path.

## Alternatives

### Generate one VitePress markdown page per source file

Using VitePress dynamic routes (`source/[...file].paths.js`) produces clean path
URLs (`/source/crates/jp_llm/src/lib.rs`) without query strings.
Rejected because VitePress runs full Vue SSR compilation and bundle chunking for
every dynamic route, turning a 1,500-file repository into a multi-minute docs
build.

### Publish by default

Using a single `source.enable` setting for both the dev server and static builds
is simpler, but publishes repository source code by default.
A project with a private repository and public documentation would publish its
source tree on the first deploy after upgrading `jp-vite`, because `jp vite
sync` only lists the template files it writes to the workspace, not the source
files emitted during `vitepress build`.
Because accidental source disclosure cannot be undone, static publishing
requires an explicit opt-in via `source.publish`.

### Dev-server only

Serving `/source/` only in `jp vite serve` avoids static publishing concerns
entirely, but removes in-site source browsing and `Referenced by` backlinks from
deployed sites.
JP wants both features on its public documentation site, so static publishing is
supported via `source.publish`.

## Non-Goals

- **Editing source code in the browser.** The explorer is read-only.
- **Git blame or commit history views.** The `GitHub ↗` header link delegates
  blame and commit history to the repository's forge.
- **Semantic code navigation (go-to-definition).** Syntax highlighting is
  lexical via Shiki, not backed by an LSP server.
- **Historical revisions.** The explorer serves the working tree on the dev
  server and the built commit on the deployed site.
  Commit-pinned permalinks (such as those cited by [RFD 127] `Snapshot` wiki
  pages) remain external URLs and open on the forge.

## Risks and Open Questions

- **Links to non-markdown assets inside the site directory.** Target
  classification leaves markdown pages and files under `public/` to VitePress.
  We need to verify how VitePress handles plain links (as opposed to image
  embeds) to other files inside the docs source directory, such as
  `[diagram](./x.png)`, before deciding whether to leave them to VitePress or
  route them to `/source/`.

## Implementation Plan

### Phase 1: Workspace link & line-range validation in `jp-vite`

Update `rewriteRepositoryLinks` in
`crates/plugins/command/vite/site/jp-vite/config.mjs` to classify links by
resolved target and verify that source links and `#L<start>-L<end>` line ranges
exist on disk during both builds and dev rendering.

Because link validation makes referenced source files inputs to the
documentation build, pull requests that modify, rename, or delete a linked
workspace file must run the docs check.
JP's `docs.yml` workflow only runs on pull requests that touch `docs/`, the
`justfile`, or the workflow file itself, so deleting or editing a linked source
file would pass PR checks and fail only during deployment on `main`.
Update `docs.yml` to search `docs/` for relative links matching any file changed
outside `docs/`, and trigger the docs build when a match is found.

Mergeable independently.

### Phase 2: `<SourceExplorer />` and dev-server `/__source` endpoint

Add `source/index.md` to `jp vite sync`, implement `<SourceExplorer />` with the
file tree sidebar, Shiki highlighting, `#L<start>-L<end>` highlighting and range
selection, and `Referenced by` backlink badges, and register `/__source` in
`middleware.mjs`.

Depends on Phase 1.

### Phase 3: Static build emission (`buildEnd`)

Update `withJpVite`'s `buildEnd` hook to emit `source-index.json` and
`source-files/` when `source.publish` is enabled, and rewrite eligible source
links in static builds to `/source/?path=<path>#L...`.
Set `source.publish = true` in JP's `.jp/config.toml`.

Depends on Phase 2.

## References

- [RFD 116: Shareable RFD, Ticket, and Site Tooling][RFD 116]
- [RFD 127: In-Repo Project Wiki][RFD 127]

[RFD 116]: 116-shareable-rfd-ticket-and-site-tooling.md
[RFD 127]: 127-in-repo-project-wiki.md

