# RFD 127: In-Repo Project Wiki

- **Status**: Accepted
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-10-30
- **Extends**: [RFD 100], [RFD 102], [RFD 116]
- **Extended by**: [RFD 128]
- **Summary**: Adds a `jp-wiki` plugin and `jp-vite` integration for structured,
  cross-linked project knowledge and research notes in the repository.

## Summary

This document introduces the **project wiki**, a third in-repo documentation
workflow alongside RFDs and tickets, managed by the `jp-wiki` command plugin and
rendered by `jp-vite`.
Wiki pages live at `docs/wiki/<id>-<slug>.md` with collision-resistant `W-<id>`
identifiers, distinguish maintained system references (`Reference`) from
point-in-time investigations (`Snapshot`), and support section-level linking and
transclusion.
`jp wiki check` validates links to pages, heading anchors, and workspace source
lines.

## Motivation

[RFD 100] introduced in-repo tickets because RFDs had become the only place to
write anything down.
With `jp-ticket` and `jp-rfd` in place ([RFD 116]), a third kind of artifact
falls into the gap between them: project knowledge that is neither a design
proposal nor a unit of work.

In practice, contributors and assistants produce:

- Literature and prior-art notes (such as paper evaluations or comparative
  studies of other systems)
- Domain glossaries and ubiquitous language definitions
- Subsystem architecture overviews
- Operational and qualification procedures

These artifacts end up in `jp-ticket` (often under a `type=note` or `type=task`
label) because `jp-ticket` provides capabilities that plain markdown files in
`docs/` lack:

1. **Zero-friction capture** (`jp ticket add` / `ticket_create`) without
   choosing a directory path or editing `.vitepress/config.mts`.
2. **Short identifiers** (`T-0n0nwz3`) that tickets and RFDs can cite in prose
   and `jp-vite` auto-links.
3. **Structured metadata and validated labels** (`relevant-rfd=006`,
   `domain=conversation`) backed by [`jp_label`].
4. **Bidirectional backlinks** (`References` and `Referenced by` badges on the
   site, plus RFD link counts).
5. **A filterable index and dev-server editing** (`<DocIndex />` and `/__ticket`
   middleware).

Once filed as a ticket, however, a note sits on a kanban board whose columns are
`Todo`, `In Progress`, and `Done`.
Leaving a research note in `Todo` pollutes the prioritized work queue; marking
it `Done` buries active reference material in the completed-work archive.

Meanwhile, `docs/architecture/` has the opposite problem: its files sit outside
the VitePress navbar and sidebar, rely on a manually maintained `index.md` that
omits newer files, lack backlinks to citing RFDs and tickets, and do not fail
the build when `#section` anchors or relative links to source files break.

Traditional wikis solve capture but fail on maintenance: without ordering,
freshness contracts, and link checking, a wiki degrades into orphan pages, stale
claims, and broken links.
An in-repo wiki integrated into `jp`'s toolchain prevents those failure modes.

## Design

### Scope and Lifecycle

The three workflows answer distinct questions and have distinct lifecycles:

|              | RFD                            | Ticket                         | Wiki Page                                      |
| ------------ | ------------------------------ | ------------------------------ | ---------------------------------------------- |
| Question     | What should we build, and why? | What needs doing?              | What do we know, and how does X work?          |
| Contains     | A design and its rationale     | A description and a discussion | Reference documentation or research            |
| Lifetime     | Frozen once implemented        | Closed when the work is done   | Maintained (`Reference`) or dated (`Snapshot`) |
| Primary view | Priority board                 | Kanban work queue              | Hierarchical sidebar and filterable index      |

User-facing product documentation (installation, getting started, feature guides
for end users of the binary) stays in ordinary VitePress markdown pages under
`docs/`.
Contributor- and assistant-facing project knowledge (architecture, ubiquitous
language, research notes, qualification runbooks) belongs in the wiki.

### Page Format and Identity

A wiki page is a single markdown file at `docs/wiki/<id>-<slug>.md`.
Every page is plain CommonMark, readable in any file explorer, text editor, or
Git forge without `jp` installed.

The `<id>` is a 7-character collision-resistant identifier allocated from the
wiki directory's `.epoch` file using the same algorithm as [RFD 102].
The canonical reference form is `W-02wt0kx`; CLI commands and tools also accept
`W02wt0kx` or the bare 7-character ID, case-insensitively.

Keeping identity in a flat `<id>-<slug>.md` filename rather than a nested
directory path gives two properties:

1. **Zero filing friction.** Creating a page requires only a title and an
   optional parent page, not a directory path.
2. **Rename and reorganization safety.** Renaming a page updates its `<slug>`
   and rewrites relative file links, while every `W-02wt0kx` citation in RFDs,
   tickets, commits, and conversations continues to resolve.

<!-- end list -->

```markdown
# Ubiquitous Language: Conversation

- **Status**: Reference
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-10-30
- **Parent**: W-0a1b2c3
- **Label**: domain=conversation
- **Label**: relevant-rfd=064
- **Summary**: Canonical definitions for Conversation, Turn, Thread, and Event
  Stream.

## Turn

A single request-response cycle initiated by the user...
```

### Metadata

Wiki pages use the same `- **Key**: Value` metadata header as RFDs and tickets,
formatted at 80 columns by `comfort`.

| Field           | Required | Values                                        |
| --------------- | -------- | --------------------------------------------- |
| `Status`        | yes      | `Reference`, `Snapshot`, `Archived`           |
| `Authors`       | yes      |                                               |
| `Date`          | yes      | `YYYY-MM-DD`                                  |
| `Parent`        | no       | `W-<id>` of the parent wiki page              |
| `Superseded by` | no       | `W-<id>` or `RFD NNN` when `Status: Archived` |
| `Label`         | no       | One `key=value` pair, repeatable              |
| `Summary`       | yes      | One sentence, last in the metadata block      |

#### Epistemic Status (`Reference`, `Snapshot`, `Archived`)

Wikis lose trust when point-in-time notes and living system specifications sit
side by side without distinction.
`Status` declares the freshness contract of the page:

- **`Reference`**: Authoritative documentation of the current system (such as
  subsystem architecture, glossary definitions, or runbooks).
  If the code and a `Reference` page disagree, one of them is wrong.
- **`Snapshot`**: Point-in-time research, literature synthesis, benchmark
  analysis, or external prior-art study (such as paper notes or game design
  studies).
  A `Snapshot` is dated evidence; it is not expected to change when the codebase
  evolves.
- **`Archived`**: Retired material kept so historical links still resolve.
  `Archived` pages are omitted from the site sidebar and from default `jp wiki
  list` and `wiki_list` output (unless `--archived` or `--all` is passed), and
  may carry `- **Superseded by**: W-<id>` or `RFD NNN`.

#### Parent Hierarchy and Ordering

Organization is decoupled from filenames through two mechanisms:

1. **`Parent`**: A page may name another wiki page as its parent (`- **Parent**:
   W-0a1b2c3`), forming a tree of arbitrary depth without moving files into
   subdirectories.
   A page without a `Parent` is a top page that introduces a topic area (such as
   architecture or the glossary) and groups its child pages.
   `jp wiki check` rejects cycles in `Parent` links.

2. **Ordering (`.order.json`)**: As in `jp-ticket` (`.board.json`) and `jp-rfd`
   (`.priority.json`), relational ordering lives in `docs/wiki/.order.json`
   rather than inside individual files.
   The file lists the top pages in display order and maps each parent page ID to
   its ordered child pages:

   ```json
   {
     "top": [
       "W-0b42fs5",
       "W-0a1b2c3"
     ],
     "children": {
       "W-0a1b2c3": [
         "W-0a1b2c4"
       ]
     }
   }
   ```

   Pages not listed in `.order.json` sort after ranked pages by ID.

### Linking and Section Addressing

#### Cross-Document Citations

`jp-vite`'s mention linker (`references.mjs`) and document registry
(`theme/documents.mjs`) add `wiki` as a third document kind alongside `rfd` and
`ticket`:

- Mentioning `W-02wt0kx` in prose or inline code in any RFD, ticket, or wiki
  page links to `/wiki/02wt0kx-slug` and records a bidirectional edge in
  `<DocReferences />` (`References` and `Referenced by`).
- `redirects.mjs` emits short-link redirects from `/wiki/W-02wt0kx` and
  `/wiki/02wt0kx` to `/wiki/02wt0kx-slug`.
- Mention syntax supports an optional section fragment:
  `W-0a1b2c3#pinned-conversation`, `RFD 100#labels`, and `T-0n0nwz3#swot` link
  directly to that heading anchor on the target page.

#### Heading Slugs and Section Addressing

Every ATX heading (`#` through `######`) gets an anchor slug computed
identically by `jp-vite` and the Rust CLI, so any `#fragment` accepted by `jp
wiki check` matches the anchor rendered on the site.

`jp-vite` pins the slug algorithm by setting `markdown.anchor.slugify` in
`withJpVite` to its own copy of VitePress's default (`slugify` from
`@mdit-vue/shared`), preserving existing anchors across VitePress upgrades.
The Rust crate implements the same algorithm, and a shared test fixture of
headings and expected slugs keeps the JavaScript and Rust implementations in
sync.

The slug function normalizes Unicode (NFKD), strips combining marks, replaces
runs of whitespace and punctuation (including `_`) with `-`, lowercases letters,
and prefixes leading digits with `_`:

| Heading                                         | Slug                                |
| ----------------------------------------------- | ----------------------------------- |
| `## Pinned Conversation`                        | `pinned-conversation`               |
| ``### 1. Query Command (`jp_cli::cmd::query`)`` | `_1-query-command-jp-cli-cmd-query` |

Slugs are deduplicated across the entire source document (`examples`,
`examples-1`, `examples-2`), so reading or embedding a single section never
changes its suffix.
`jp wiki check` rejects custom heading anchors (`## Turn {#turn}`) because Git
forges and plain-text readers render them as literal text.

Commands and tools address sections by anchor slug or heading title.
If the argument matches a slug, that section is selected; otherwise it is
matched against heading titles.
If multiple headings share that title, the command fails and lists the candidate
slugs rather than guessing which section to target:

- `jp wiki show W-0a1b2c3` prints the metadata header, linked RFDs/tickets/wiki
  pages, and the page's table of contents (indented headings with their `#slug`
  anchors), omitting the body unless `--body` is passed.
- `jp wiki show W-0a1b2c3 --section pinned-conversation` prints only that
  section (from the matched heading up to the next heading of equal or higher
  level).
- `jp wiki edit W-0a1b2c3 --section pinned-conversation --body "..."` replaces
  only that section in place.

#### Transclusion (Reusing Snippets Across Pages)

A `Reference` page can embed a section of another document or a line range of a
workspace source file using standard CommonMark image syntax on its own line:

```markdown
![Pinned Conversation](0a1b2c3-ubiquitous-language.md#pinned-conversation)

![Links::by_rfd](../../crates/plugins/command/rfd/src/links.rs#L88-L125)
```

Using `![Title](target)` preserves plain-text readability:

- **In `$EDITOR` and Git forges**: It is valid CommonMark with a standard
  relative path that editor go-to-file commands follow directly.
- **In `jp-vite`**: A `markdown-it` rule replaces a standalone transclusion link
  pointing to `.md#section` with the rendered AST of that section (shifting
  child heading levels to sit below the enclosing heading) inside a container
  that links back to the canonical source.
  A transclusion link pointing to a workspace file with `#L<start>-L<end>`
  expands into a fenced code block with the file's extension as its language tag
  and a header linking to the source file.
- **In `jp wiki show --body` and `wiki_show` (with `expand_embeds`)**: The CLI
  and tool expand transclusions inline so callers receive the embedded content
  in a single read.
  `wiki_show` leaves embeds unexpanded by default because `wiki_edit` operates
  on the raw markdown; saving expanded output back to the page would replace the
  embed syntax with a static copy.
  When `expand_embeds` is set, `wiki_show` wraps each expanded block in `<!--
  embed W-0a1b2c3#pinned-conversation -->` and `<!-- /embed -->` markers so the
  assistant can distinguish transcluded content from the page's own text.
- **In `<DocReferences />`**: Transcluding a document counts as referencing it,
  so the source page lists every page that embeds it under `Referenced by`.

An embedded section is parsed in the context of its full source document so
`jp-vite` and the Rust reader resolve links identically:

- Reference-style links resolve against definitions at the bottom of the source
  document (where `comfort` places them, outside the transcluded section).
- Relative link targets resolve against the source document's directory and are
  rewritten relative to the embedding page.
- Intra-document `#fragment` links continue to point to the source document
  rather than to headings in the embedding page.
- Nested embeds resolve relative to the document that contains them.

In `jp-vite`, any `![...](...)` link targeting a markdown document or workspace
source file that is not a standalone section or line-range embed fails the build
rather than rendering as a broken image.

`Snapshot` pages cannot use transclusion.
Because embeds always render the target's current content, transcluding into a
`Snapshot` would mutate a point-in-time record as the source evolves.
Instead, a `Snapshot` copies the relevant excerpt inline and cites it with a
commit permalink (see [Snapshot Citations](#snapshot-citations)).

#### Deep-Linking to Workspace Source Files

Links from wiki pages, RFDs, and tickets to files in the repository use normal
relative paths, optionally carrying a 1-based line or line-range fragment:

```markdown
See [`Links::by_rfd`](../../crates/plugins/command/rfd/src/links.rs#L88-L125).
```

Line fragments accept `#L12`, `#L12-L24`, `#L12-24`, and `#12-24`, normalizing
to `#L12-L24` when rewritten by `jp-vite`.

When a source link's label is an inline code span (such as ``[`Links::by_rfd`]``
or ``[`by_rfd`]``), `jp wiki check` uses the trailing identifier (`by_rfd`) as a
symbol guard and verifies that it appears within the linked line range (or
anywhere in the file if no line range is given).
This catches silent line drift when edits above the linked range shift the
target code.

#### Snapshot Citations

Relative source links track the working tree.
That fits `Reference` pages, but in a `Snapshot` page a relative link either
breaks or silently points to newer code once the file changes.
`Snapshot` pages therefore cite source code using commit permalinks:

```markdown
[`Links::by_rfd`](https://github.com/dcdpr/jp/blob/9a43a5e2dc077909a9b8fca9e58abe541b3d8cda/crates/plugins/command/rfd/src/links.rs#L88-L125)
```

A permalink works as a standard URL in editors, on Git forges, and on the
documentation site, and its target is immutable.
`jp wiki` recognizes a URL as a permalink when it references a full commit SHA
in the repository configured for `jp vite`
(`plugins.command.vite.options.repository`, falling back to git's `origin`
remote); any other URL is treated as an external link.

The referenced commit must be reachable from the default branch (`origin/HEAD`),
because branch commits become unreachable after a squash merge and are not
fetched by fresh clones.
Code that exists only on an unmerged branch cannot be linked with a permalink
yet; the page cites the path and line range as inline code (``
`crates/foo/src/bar.rs` L12-24 ``) alongside the copied excerpt until the branch
lands and the link can be pinned.

`jp wiki pin <ID>` rewrites a page's relative source links into permalinks.
Each link resolves to the most recent default-branch commit where the file
matches the working tree; links with no matching commit on the default branch
are left unchanged and reported.
Creating a `Snapshot` page or changing a page's status to `Snapshot` (via the
CLI or assistant tools) runs the same pinning pass automatically.

### Validation (`jp wiki check`)

`jp wiki check` runs all structural and anti-rot checks over `docs/wiki/` and
exits `1` on any error:

1. **Metadata and formatting**: Every page has a `# Title`, required metadata
   fields (`Status`, `Authors`, `Date`, `Summary`), and no duplicate IDs.
2. **Label vocabulary**: Every `Label` satisfies
   `plugins.command.wiki.options.labels` (`values`, `min`, `max`, `unique`),
   using the same validator as `jp ticket check`.
3. **Hierarchy and order integrity**: The `Parent` of every non-archived page,
   and every `.order.json` entry, names an existing, non-archived wiki page,
   with no cycles in `Parent`.
   An archived page may keep its own `Parent`.
4. **Internal page and `#section` anchor links**: Every link or transclusion to
   another wiki page, RFD, or ticket resolves to an existing file, every
   `#fragment` on a markdown link matches a heading slug in the target file, and
   no heading carries a custom anchor.
5. **Workspace source file, line-range, and symbol links**: In a `Reference`
   page, every relative link or transclusion that resolves outside `docs/` into
   the workspace must point to an existing file or directory on disk.
   If a `#L<start>-L<end>` fragment is present, `1 <= start <= end <=
   line_count` must hold.
   If the link text is a code span, the identifier must appear within that line
   range; if it moved elsewhere in the file, the error reports the new line
   number.
   In a `Snapshot` page, any relative link into the workspace outside `docs/` is
   rejected with an error directing the user to run `jp wiki pin`.
   Permalinks must reference a commit that exists locally and is reachable from
   the default branch; the file path, line range, and symbol guard are validated
   against that commit's tree so later code changes never invalidate a
   `Snapshot`.
6. **Transclusions**: `![...](...#section)` embeds must not form cycles and are
   rejected in `Snapshot` pages.
7. **Advisory anti-rot warnings** (printed to stderr, promoted to errors with
   `--strict`):
   - **Orphan pages**: A non-archived page with no `Parent`, no incoming links
     from any RFD, ticket, or wiki page, and no `relevant-rfd` label.
   - **Superseded RFD citations**: A `Reference` wiki page that cites an RFD
     whose status is `Superseded` or `Abandoned`.

### Configuration

`jp-wiki` follows [RFD 116]'s plugin configuration pattern under
`plugins.command.wiki`:

```toml
[plugins.command.wiki]
run = "allow"

[plugins.command.wiki.options]
dir = "docs/wiki"
assistant = "jp"
# The label key a wiki page uses to link to an RFD.
relevant.key = "relevant-rfd"

[plugins.command.wiki.options.labels.domain]
description = "The subsystem covered by this page"
values = ["conversation", "llm", "mcp", "plugins", "storage", "tooling"]

[plugins.command.wiki.options.labels.relevant-rfd]
description = "An RFD this page relates to"
values = []
```

### CLI Commands and Assistant Tools

| Verb             | `jp wiki` CLI                                                   | Assistant tool (`plugin.command.wiki.*`) |
| ---------------- | --------------------------------------------------------------- | ---------------------------------------- |
| Create           | `add <TITLE> [--parent ...] [--status ...] [--body ...]`        | `wiki_create`                            |
| Show             | `show <ID> [--body] [--section <SLUG>] [--raw]`                 | `wiki_show`                              |
| List             | `list [--under ...] [--status ...] [--label ...] [--query ...]` | `wiki_list`                              |
| Edit             | `edit <ID> [--title ...] [--body ...] [--section ...]`          | `wiki_edit`                              |
| Rename           | `rename <ID> <TITLE>`                                           | `wiki_edit` (via `title`)                |
| Rank             | `rank <ID> (--before OTHER \| --after OTHER)`                   | not a tool                               |
| Archive          | `archive <ID> [--superseded-by <TARGET>]`                       | `wiki_edit` (via `status`)               |
| Pin              | `pin <ID>`                                                      | `wiki_edit` (via `status`)               |
| Check            | `check [--strict]`                                              | not a tool                               |
| Migrate ticket   | `from-ticket <TICKET-ID> [--parent ...] [--status ...]`         | not a tool                               |
| Tool definitions | `tools init <PATH>`, `tools sync <PATH>`                        | not a tool                               |
| Init             | `init`                                                          | not a tool                               |

With no mutation flags, `jp wiki edit <ID>` opens the markdown file in the
user's `$EDITOR` via protocol 13's `edit_files` and runs the page validator when
the editor closes.

`archive` refuses to archive a page that still has non-archived child pages and
lists the blocking children, preventing active pages from being hidden under an
archived parent in the sidebar.
Archiving also removes the page from `.order.json`.
`wiki_edit` enforces the same rules when setting `status` to `Archived`.

`jp wiki from-ticket <TICKET-ID>` migrates a note ticket into the wiki, matching
`jp rfd draft --from-ticket` ([RFD 116]).
It creates a wiki page from the ticket's title and description, copies
`Authors`, `Date`, and any labels permitted by the wiki label vocabulary
(reporting any dropped labels), and closes the ticket as `Done` with `Promoted
to` pointing to the new page.
The ticket file remains in the repository so existing `T-` citations in tickets,
RFDs, commits, and conversations still resolve and link to the new wiki page.
Ticket comment threads stay on the ticket with their original attribution; the
author migrating the ticket incorporates any relevant context from the comments
into the wiki page.

The wiki page receives a newly allocated `W-<id>` rather than reusing the
ticket's ID body.
Because the first five characters of an ID encode a timestamp relative to its
directory's `.epoch` ([RFD 102]), reusing a ticket ID under the wiki's epoch
would produce the wrong timestamp and break creation-time ordering.
Like `draft --from-ticket`, `from-ticket` is idempotent: if the ticket's
`Promoted to` field already references an existing wiki page, the command
reports that page and exits without creating a duplicate.

#### Assistant Tools

To keep per-turn schema overhead small, `jp-wiki` exposes four tools:

1. **`wiki_list`**: Lists non-archived wiki pages in `.order.json` order, with
   each top page followed by its descendants, optionally filtered by `under`
   (descendants of a given page), `status`, `labels`, or `query`.
   With `full: true`, includes each page's one-line `Summary`.
2. **`wiki_show`**: Returns a page's metadata, incoming/outgoing references, and
   its heading table of contents (`#slug` and title) by default.
   Passing `body: true` returns the full markdown body as written; passing
   `section: "<slug>"` returns only that section; `expand_embeds: true` expands
   transclusions in either mode.
3. **`wiki_create`**: Creates a new wiki page with `title`, `summary`, and
   optional `body` (omitting `body` creates a hub page), `status` (defaulting to
   `Reference`), `parent`, and `labels`.
4. **`wiki_edit`**: Single mutation tool accepting `id` and any combination of
   `title` (which also renames the file and rewrites relative links), `summary`,
   `status`, `parent`, `before` or `after` (reordering the page relative to a
   sibling, like `jp wiki rank`), `superseded_by`, `body`, `section` (scoping
   `body` replacement to one heading section), `add_labels`, and
   `remove_labels`.

Each tool enforces the invocation's file-access policy ([RFD 116]) on every file
it touches, not just the wiki directory:

- `wiki_show` checks read permission for every transcluded target, including
  nested embeds.
  Read access to a wiki page does not grant read access to files it embeds.
  If an embed is denied, `wiki_show` substitutes a marker naming the target and
  the access denial and returns the rest of the page.
- `wiki_edit` modifies only files permitted by the write policy.
  When a rename cannot update links in files outside the allowed paths (or
  cannot scan the workspace for references), it reports the skipped paths or
  restricted search scope, matching `rfd_rename`.

### Site Integration (`jp-vite`)

`jp-vite` adds `wiki` to `settings.json` alongside `rfd` and `ticket` when the
configured wiki directory exists:

- **Auto-generated hierarchical sidebar**: Unlike RFDs and tickets, which hide
  the VitePress sidebar, `withJpVite` generates `themeConfig.sidebar['/wiki/']`
  from the `Parent` hierarchy and `.order.json`, placing each top page above its
  children and using each page's `# Title` as the link label.
  Previous/next links at the bottom of each page follow the same reading order.
- **Front page and hub pages**: `jp vite sync` scaffolds `/wiki/`
  (`docs/wiki/index.md`) once if missing and never overwrites it, so
  contributors can customize it with an introduction, starting links, and
  embeds.
  The `<WikiChildren />` component renders a list of direct child pages and
  their summaries in reading order: for the current page by default, for a
  specific page when passed `of="W-0a1b2c3"`, or for the top pages when used on
  `/wiki/`.
  A hub page with only a title and metadata (no body text) renders
  `<WikiChildren />` automatically.
  `jp wiki check` counts links from `docs/wiki/index.md` as incoming links, so
  top-level hub pages linked from the front page are not flagged as orphans.
- **`<WikiIndex />` on `/wiki/all`**: Wraps `<DocIndex />` with per-top-page
  filter buttons (narrowing the table to that subtree), status badges
  (`Reference`, `Snapshot`, `Archived`), label badges, search, summary toggles,
  and dev-server `+ New page`, `Edit`, and `Archive` actions backed by the
  `/__wiki` middleware and `jp wiki`.
- **Editing in place**: When the dev server is running with editing enabled,
  every wiki page displays an `Edit page` button and per-heading section edit
  buttons.
  Opening the editor fetches the unexpanded markdown via `jp wiki show --raw`,
  and saving invokes `jp wiki edit` (with `--section` for section edits),
  validating the page before writing changes to disk.
  On the static published site, edit links point to the file on the Git forge's
  default branch (resolved from `origin/HEAD`).
- **Compact metadata rendering**: While markdown files on disk keep the `-
  **Key**: Value` metadata list, `jp-vite` renders metadata compactly across
  RFDs, tickets, and wiki pages, grouping labels by key on a single line
  (`domain: conversation, llm`).
  On a wiki page, the `Parent` chain renders as a breadcrumb trail (`Wiki ›
  Glossary › Ubiquitous Language`) above the title alongside the `Edit page`
  button, `Status` renders as a letter badge (`R`, `S`, or `A`) next to the
  title, and `Summary`, `Authors`, and `Date` render in a footer above
  `<DocReferences />`.
  On RFD and ticket pages, the header displays a compact line with the status
  badge, category, date, and authors, while the `Summary` appears only on index
  pages.
- **Cross-references**: `<DocReferences />` renders outgoing `References` and
  incoming `Referenced by` links at the bottom of each RFD, ticket, and wiki
  page, grouped by document kind, labeled by title, and collapsed when long.
  RFD and ticket pages include incoming links from wiki pages that cite them in
  prose or carry `relevant-rfd=NNN`.
- **Moved pages**: A `redirects` map in `plugins.command.vite.options` maps
  legacy routes to new targets and accepts wiki page IDs (such as
  `"architecture/ubiquitous-language" = "W-0a1b2c3"`), so renaming a target page
  never breaks the redirect.
  `redirects.mjs` generates HTML redirect stubs that preserve URL `#fragment`
  anchors.

### Shared Workspace Substrate

[RFD 100] deferred extracting shared infrastructure until a third consumer
existed.
This RFD extracts `crates/internal/doc` (or expands `site-revision`) with the
primitives shared by `ticket`, `jp-rfd`, and `jp-wiki`:

- `- **Key**: Value` metadata block parsing and `comfort`-compatible rendering
- 7-character epoch ID allocation and parsing parameterized by prefix (`T-`,
  `W-`)
- Workspace-wide reference walking and link rewriting (`references.rs`)
- Markdown ATX heading extraction and `#anchor` slug computation
- Workspace relative-path, line-range, and code-span symbol verification

For `from-ticket`, `jp-wiki` depends on the `ticket` crate directly (as `jp-rfd`
does for `draft --from-ticket`), keeping a single implementation of the ticket
format.

## Drawbacks

- **Four more tool definitions when enabled.** Offering `wiki_list`,
  `wiki_show`, `wiki_create`, and `wiki_edit` adds four schemas to the
  assistant's tool list in workspaces that enable the wiki.
  Consolidating all mutations into `wiki_edit` caps this at four rather than
  eight.
- **Flat filenames on disk.** `docs/wiki/0a1b2c3-ubiquitous-language.md` puts
  the 7-character ID at the front of every filename, so `ls docs/wiki/` orders
  by creation time rather than by topic.
  `jp wiki list` and the VitePress sidebar provide the topical hierarchy.
- **Embeds look like images on Git forges.** Git forge markdown previews treat
  `![...](...)` as an image tag and render a broken image icon instead of the
  transcluded content, though the raw markdown remains readable and links to the
  target path.
- **Verbose permalinks and delayed pinning on branches.** Commit permalinks are
  long in raw markdown, and source code that exists only on an unmerged branch
  must be cited as plain code text until the branch merges into the default
  branch.

## Alternatives

### Add a `Note` status to `jp-ticket`

Adding `Status::Note` to `crates/internal/ticket` and hiding `Note` tickets from
the kanban board takes roughly 50 lines of code.
It was rejected because a literature note or architecture reference is not a
work item: ticket affordances (append-only `From`/`Date` comment threads, flat
listing, `T-` identifiers) are the wrong model for living documentation, and it
cannot absorb `docs/architecture/` or provide hierarchical navigation and
section transclusion.

### Add a status-exempt category to `jp-rfd`

Allowing a `note` category in `jp-rfd` that skips `Draft -> Implemented` would
reuse `jp-rfd`, but consumes permanent sequential RFD numbers (`001..999`) for
informal notes and mixes historical decision records with living reference
pages.

### Directory hierarchy instead of short IDs

Storing wiki pages as `docs/wiki/<area>/<slug>.md` without IDs produces clean
filenames on disk, but reintroduces taxonomy friction on creation and breaks
prose citations whenever a page is moved to another area or renamed.

### Separate `Category` metadata field

A `Category` field drawn from a configured list (`architecture`, `glossary`,
`research`, `guide`) could group pages in the sidebar and filter the index.
However, the `Parent` hierarchy already serves both purposes: each top page acts
as a category for its descendants.
Adding a separate `Category` field duplicates the hierarchy and allows a child
page's category to conflict with its parent's.

### VitePress `@include` for transclusion

VitePress supports HTML-comment includes natively, but HTML comments are
invisible in GitHub markdown previews and plain-text reading, and do not
participate in `jp-vite`'s `<DocReferences />` graph or `jp wiki show`.

## Non-Goals

- **Prompt injection and `learn` tool.** Automatically injecting wiki topics
  into the system prompt is covered by the Knowledge Base design, which can
  point at `docs/wiki` as a topic directory once implemented.
- **Replacing user-facing product documentation.** End-user guides (`/usage`,
  `/installation`, `/features/*`) remain regular VitePress pages.
- **Hosted source code browsing on the website.** Validating relative source
  links (`../../crates/...#L12-L24`) happens in `jp wiki check`, while rendering
  workspace source files inside the VitePress site is a separate `jp-vite`
  extension.

## Risks and Open Questions

- **Heading-level shifting during transclusion.** When a `### Term` section is
  transcluded under a `## Context` heading in another page, shifting its
  headings so the outline stays valid requires care with fenced code blocks and
  nested subheadings.
- **Unchecked drift on prose-labeled source links.** Checking that the
  identifier in ``[`foo`](path.rs#L10-L20)`` appears within lines `10..=20`
  catches line shifts for code-span labels, but links with prose labels (`[the
  parser](path.rs#L10-L20)`) only receive line-count bounds checking.
- **Slug drift between `jp-vite` and Rust.** The JavaScript and Rust slug
  implementations are tested against a shared fixture.
  Any heading pattern not covered by that fixture could produce a slug that `jp
  wiki check` accepts but `jp-vite` renders differently.
- **Shallow git checkouts in CI.** Validating a permalink requires the target
  commit to be present in the local git object database.
  `docs.yml` and `rust.yml` fetch full history (`fetch-depth: 0`), whereas
  `plugins.yml` uses the default depth of `1`.
  Any CI job that runs `jp wiki check` must fetch full history; otherwise
  `check` fails because the referenced commit is missing locally.
- **Source changes bypassing CI path filters.** Because `jp wiki check`
  validates links to workspace source files, any pull request that edits,
  renames, or deletes a linked source file can break wiki links without touching
  `docs/`.
  If `jp wiki check` only runs on `docs/` path changes, a source-only pull
  request can merge broken links and fail the next documentation build.

## Implementation Plan

### Phase 1: Shared document substrate

Extract metadata parsing/rendering, parameterized 7-character epoch IDs,
reference rewriting, and heading/anchor extraction into a shared internal crate
used by `ticket` and `jp-rfd`.

Mergeable independently with no behavioral change.

### Phase 2: `jp-wiki` core and CLI

Create `crates/plugins/command/wiki` (`jp-wiki`) with `init`, `add`, `show`
(including `--section` and transclusion expansion), `list`, `edit`, `rename`,
`rank`, `archive`, `pin`, `from-ticket`, and `check` (metadata, labels,
`#section` anchors, workspace file/line/symbol links, and permalinks).

Depends on Phase 1.

### Phase 3: Assistant tools (`wiki_*`)

Add `wiki_list`, `wiki_show`, `wiki_create`, and `wiki_edit`, plus `jp wiki
tools init` and `jp wiki tools sync`.

Depends on Phase 2.

### Phase 4: `jp-vite` integration

Extend `jp-vite` (`settings.rs`, `references.mjs`, `redirects.mjs` and the
`redirects` map, `theme/documents.mjs`, `middleware.mjs`, `<WikiIndex />`, the
owned slug function, the transclusion markdown rule, and the auto-generated
`/wiki/` sidebar).

Depends on Phase 2.

### Phase 5: Migrating JP's documents

Migrate each `docs/architecture/` file into `docs/wiki/` as a single page first,
preserving existing headings so `#section` anchors remain valid; split large
pages in follow-up changes rather than during the move.

Rewrite references to `docs/architecture/` across the entire workspace,
including persona and knowledge configs under `.jp/config/` and doc comments in
`jp_cli`, and report any remaining unrewritten references.
Map legacy `/architecture/*` routes to the new wiki pages in `redirects`.

Migrate research-note tickets such as `T-0n0nwz3` with `jp wiki from-ticket`.

Depends on Phase 4.

## References

- [RFD 001: The JP RFD Process][RFD 001]
- [RFD 100: In-Repo Ticket Tracking][RFD 100]
- [RFD 102: Collision-Resistant Ticket Identifiers][RFD 102]
- [RFD 116: Shareable RFD, Ticket, and Site Tooling][RFD 116]
- [Diataxis Documentation Framework][diataxis]

[RFD 001]: 001-jp-rfd-process.md
[RFD 100]: 100-in-repo-ticket-tracking.md
[RFD 102]: 102-collision-resistant-ticket-identifiers.md
[RFD 116]: 116-shareable-rfd-ticket-and-site-tooling.md
[RFD 128]: 128-workspace-source-explorer-in-jp-vite.md
[`jp_label`]: https://github.com/dcdpr/jp/tree/main/crates/jp_label
[diataxis]: https://diataxis.fr/


