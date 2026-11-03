# RFD 128: Workspace Source Explorer in jp-vite

- **Status**: Accepted
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-10-30
- **Extends**: [RFD 116], [RFD 127]
- **Summary**: Serves syntax-highlighted workspace source files with line-range
  deep links and reverse document backlinks in `jp-vite`.

## Summary

This document extends `jp-vite` with a hosted workspace source explorer at
`/source/`.
Relative links from RFDs, tickets, and wiki pages to repository files and line
ranges (such as `../../crates/jp_llm/src/lib.rs#L12-L24`) resolve inside the
site rather than bouncing out to GitHub, highlight the linked lines with Shiki,
and display every RFD, ticket, and wiki page that references that file.

## Motivation

RFDs, tickets, and wiki pages frequently link to source files and directories in
the workspace using relative paths (for example,
`../../crates/plugins/command/rfd/src/links.rs#L88-L125`).
Today, `rewriteRepositoryLinks` in `jp-vite/config.mjs` rewrites any relative
link that climbs out of the VitePress source directory (`docs/`) into an
external GitHub URL (`${repository}/blob/HEAD/<path>#L88-L125`).

That approach has four limitations:

1. **Broken or stale links on active branches.** During local work under `jp
   vite serve`, clicking a link to a file added on the current branch returns a
   404 on GitHub `HEAD`, and clicking a line-range link on a modified file
   highlights the wrong lines because `HEAD` on `origin` does not reflect the
   local checkout.
2. **No offline or forge-free browsing.** Projects without a configured remote,
   or contributors working offline, cannot follow source links from the site.
3. **Silent dead links.** Because `rewriteRepositoryLinks` turns workspace links
   into external URLs at parse time, VitePress skips them during dead-link
   checking.
   Deleting or moving a source file never fails `vitepress build`.
4. **One-way navigation.** Clicking a link to GitHub leaves the site.
   A contributor inspecting a source file cannot see which RFDs, tickets, or
   wiki pages discuss or cite that file.

Hosting a lightweight source explorer inside `jp-vite` solves all four without
changing how links are written in markdown.

## Design

### Overview

When the source explorer is enabled (the default), `jp-vite` adds a `/source/`
route that renders workspace files and directories inside the VitePress theme.
The dev server always serves it; a static build includes it only when the
project chooses to publish its source (see [Configuration](#configuration)).

1. **Existing markdown links work unchanged.** A document still writes:

   ```markdown
   [`Links::by_rfd`](../../crates/plugins/command/rfd/src/links.rs#L88-L125)
   ```

   In `$EDITOR` and on a Git forge, that relative path resolves directly to the
   file.
   In `jp-vite`, `rewriteRepositoryLinks` rewrites it to
   `/source/?path=crates/plugins/command/rfd/src/links.rs#L88-L125`.

2. **Line-range highlighting and scrolling.** Fragments of the form `#L12`,
   `#L12-L24`, `#L12-24`, and `#12-24` highlight the specified lines and scroll
   the first highlighted line into view on load.
   Clicking a line number in the gutter updates the URL hash (`#L12`), and
   Shift-clicking selects a range (`#L12-L24`) for copying into a document.

3. **Reverse document backlinks (`Referenced by`).** Above the file content, the
   explorer renders `<RfdReferences />` showing every RFD, ticket, and wiki page
   that links to this file (or to a parent directory), with the specific line
   ranges each document cites.

4. **External forge link preserved.** When `repository` is configured, the file
   header includes a secondary `GitHub ↗` link pointing to
   `${repository}/blob/HEAD/<path>#L...`.

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

`enable` defaults to `true`, `publish` to `false`, and `exclude` to none.
JP sets `publish = true`, as above.

The two switches split along what each costs.
The dev server answers only the contributor running it, so serving source there
publishes nothing.
A static build is deployed, and git tracking a file says it belongs to the
repository, not that it may be read by everyone the documentation reaches.
A project with a private repository and public documentation turns on `publish`
deliberately or not at all.
With `publish` off, a static build rewrites source links to `repository`, as it
does today, so following one still needs access to the repository.

`jp vite sync` records the three settings in
`docs/.vitepress/jp-vite/settings.json`.

### Eligible Files

One rule decides which files the explorer can show, and the tree endpoint, the
file endpoint, the static emitter, and link rewriting all apply it.
A file is eligible when it is:

1. Tracked by git (`git ls-files --cached`), so untracked secrets, `.env` files,
   and `target/` artifacts are never candidates, whatever the ignore files say.
2. Not matched by `source.exclude`.
   The explorer takes no rules from `.ignore`, which `rg` and JP's own file
   walkers also read: hiding a fixture from the site shouldn't hide it from
   search.
3. A regular file, not a symlink, so a tracked link can't serve content from
   outside the workspace.
4. Text: a file with a null byte in its first 8,192 bytes shows a binary-file
   notice with its size instead.

On the dev server, an eligible file's content is read from the working tree, so
local edits show.
A file created on a branch appears once it is added to the index.

A directory is eligible when it holds at least one eligible file.
A link to a file or directory that exists but isn't eligible is validated like
any other source link and points at `repository`, never at an explorer page the
build didn't write.

### Architecture: Dev Server vs. Static Build

Generating a separate Vue SSR page per source file would add thousands of pages
to `vitepress build` in medium-sized repositories.
Instead, `jp-vite` uses a single `/source/` viewer component (`<SourceExplorer
/>`) backed by on-demand loading in dev and static JSON/text chunks in
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

#### In `jp vite serve` (Dev Server)

`middleware.mjs` registers `/__source`:

- `GET /__source?tree=1` returns the tree of eligible files and the map of
  workspace paths to the RFDs, tickets, and wiki pages that link to them.
- `GET /__source?path=<workspace-relative-path>` reads an eligible file from the
  working tree and highlights it with VitePress's Shiki highlighter, and refuses
  any other path.
  Edits made in `$EDITOR` appear immediately on refresh, with current line
  numbers.

#### In `vitepress build` (Static Build)

When `source.publish` is on, during `buildEnd` (alongside raw `.md` copying and
RFD redirect generation):

- `jp-vite` writes `outDir/source-index.json` (the file tree and reverse
  reference map) and copies or pre-highlights eligible files into
  `outDir/source-files/<path>.json` (containing the Shiki HTML or raw lines).
- `<SourceExplorer />` fetches `source-index.json` and the requested file's
  chunk on navigation, so `vitepress build` compiles only one Vue page
  (`source/index.md`) regardless of repository size.

### Build-Time Link and Line-Range Validation

Inside `rewriteRepositoryLinks` (which already runs on every markdown token
across the entire VitePress site, including RFDs, tickets, wiki pages, and user
docs), a link is classified by its target, not by whether it leaves the site
directory:

- A target that is a markdown page of the site, or a file in its `public/`
  directory, is the site's own and stays with VitePress.
- Any other relative target that resolves inside the workspace is a **source
  link**, whether or not it sits inside `settings.site`.
  A site at the workspace root (`dir = "."`) has source links like any other.

For every source link:

1. If the file or directory does not exist on disk, the build fails, naming the
   markdown page and the missing target.
2. If the link carries a `#L<start>[-L<end>]` fragment, the target must be a
   file and `1 <= start <= end <= line_count` must hold, or the build fails.
3. An eligible target is rewritten to the explorer; any other points at
   `repository`.

Validation runs whatever `source.enable` and `source.publish` say: whether a
file exists is a different question from whether the site may show it.

### Reverse Reference Index

The data loaders for RFDs, tickets, and wiki pages already scan document bodies
for cross-document citations (`referencedLabels` in `rfd-shared.mjs`).
We add a companion pass, `referencedFiles(content, pageDir, settings)`, that
extracts every workspace-relative link and its optional `#L<start>-L<end>`
range:

```json
{
  "crates/plugins/command/rfd/src/links.rs": [
    { "label": "RFD 116", "path": "/rfd/116-shareable-rfd-ticket-and-site-tooling", "lines": null },
    { "label": "W-0b42fs5", "path": "/wiki/0b42fs5-query-stream-pipeline", "lines": "L88-L125" }
  ]
}
```

When viewing `crates/plugins/command/rfd/src/links.rs` in `<SourceExplorer />`,
the header displays these badges using the same status-colored pill style as
`<RfdReferences />`.
Clicking a badge with `"lines": "L88-L125"` or hovering over it shows which
lines that document references.

## Drawbacks

- **Publishing publishes everything eligible.** With `source.publish` on, the
  deployed site carries every eligible file, including files no document links
  to.
  `source.exclude` is the only way to keep a tracked file off it.
- **Static build output size.** Copying or pre-highlighting every eligible file
  into `outDir/source-files/` grows the built site with the repository's tracked
  text (a few megabytes for JP).
  Projects with large tracked fixtures list them in `source.exclude`.
- **New files need `git add`.** A file created on a branch is missing from the
  dev explorer until it is added to the index, and a link to it points at
  `repository` until then.
- **Query-parameter or hash routing on static hosts.** Static hosts like GitHub
  Pages cannot serve arbitrary `/source/crates/foo/bar.rs` paths without a HTML
  stub per file.
  Using `/source/?path=crates/foo/bar.rs#L12-L24` compiles a single
  `/source/index.html` page and works on every static host without generating
  thousands of HTML files.

## Alternatives

### Generate one VitePress markdown page per source file

Using VitePress dynamic routes (`source/[...file].paths.js`) gives clean path
URLs (`/source/crates/jp_llm/src/lib.rs`) without query strings.
Rejected because VitePress runs full Vue SSR compilation and bundle chunking for
every dynamic route, turning a 1,500-file repository into a multi-minute docs
build.

### Publish by default

One `source.enable` switch covering both the dev server and the static build is
simpler, but makes publishing source the default.
A project with a private repository and public documentation would publish its
source on its first deploy after upgrading `jp-vite`, and `jp vite sync`'s
confirmation lists the files it writes into the project, not what the next
deploy publishes.
A disclosure like that can't be taken back, so publishing is opt-in.

### Never publish

Serving `/source/` only on the dev server avoids the question entirely, but the
deployed site loses in-site browsing and `Referenced by` badges.
JP wants both on its own site, so publishing stays available.

## Non-Goals

- **Editing source code in the browser.** The explorer is read-only.
- **Git blame or commit history views.** The `GitHub ↗` header link delegates
  blame and commit history to the repository's forge.
- **Semantic code navigation (go-to-definition).** Highlighting is lexical via
  Shiki, not LSP-backed.
- **Past revisions.** The explorer shows the working tree on the dev server and
  the built revision on the deployed site.
  Permalinks to a commit, which [RFD 127] `Snapshot` pages cite, are external
  links and open on the forge.

## Risks and Open Questions

- **Links to non-markdown files inside the site.** Classifying a target as the
  site's own assumes VitePress serves what sits in `public/` and resolves
  markdown pages.
  How it treats a plain link (not an image) to another file inside the source
  directory, such as `[diagram](./x.png)`, needs checking before that case is
  left to it or routed to the explorer.

## Implementation Plan

### Phase 1: Workspace link & line-range validation in `jp-vite`

Update `rewriteRepositoryLinks` in
`crates/plugins/command/vite/site/jp-vite/config.mjs` to classify links by
target and to verify that source links and their `#L<start>-L<end>` ranges exist
on disk during build and dev rendering.

Source files become inputs to the site build, so a pull request that changes,
renames, or deletes a workspace file a document links to must run that build.
JP's `docs.yml` builds a pull request only when it touches `docs/`, the
justfile, or the workflow, so a deleted source file passes review and fails the
deployment build on `main`.
The workflow's trigger grows to cover those files: each changed path outside
`docs/` is looked up in `docs/`, which finds every relative link ending in it.

Mergeable independently.

### Phase 2: `<SourceExplorer />` and dev-server `/__source` endpoint

Add `source/index.md` to `jp vite sync`, implement `<SourceExplorer />` with the
file tree, Shiki highlighting, `#L<start>-L<end>` highlighting/selection, and
reverse `Referenced by` badges, and add `/__source` to `middleware.mjs`.

Depends on Phase 1.

### Phase 3: Static build emission (`buildEnd`)

Emit `source-index.json` and `source-files/` in `withJpVite`'s `buildEnd` hook
when `source.publish` is on, and point a static build's eligible source links at
`/source/?path=<path>#L...`.
Set `source.publish = true` in JP's `.jp/config.toml`.

Depends on Phase 2.

## References

- [RFD 116: Shareable RFD, Ticket, and Site Tooling][RFD 116]
- [RFD 127: In-Repo Project Wiki][RFD 127]

[RFD 116]: 116-shareable-rfd-ticket-and-site-tooling.md
[RFD 127]: 127-in-repo-project-wiki.md

