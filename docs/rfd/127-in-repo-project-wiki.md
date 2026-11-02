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

This document establishes the **project wiki**: a third in-repo documentation
workflow alongside RFDs and tickets, managed by a `jp-wiki` command plugin and
rendered by `jp-vite`.
Wiki pages live in `docs/wiki/<id>-<slug>.md` with collision-resistant `W-<id>`
identifiers, distinguish maintained system references (`Reference`) from
point-in-time investigations (`Snapshot`), support section-level linking and
transclusion, and validate links to pages, headings, and workspace source lines
in `jp wiki check`.

## Motivation

[RFD 100] introduced in-repo tickets because RFDs had become the only place to
write anything down.
With `jp-ticket` and `jp-rfd` in place ([RFD 116]), a third kind of artifact now
falls into the gap between them: project knowledge that is neither a design
proposal nor a unit of work.

In practice, contributors and assistants produce:

- Literature and prior-art notes (such as paper evaluations or comparative
  studies of other systems)
- Domain glossaries and ubiquitous language definitions
- Subsystem architecture overviews that describe how the code works today
- Operational and qualification procedures

These keep landing in `jp-ticket` (often under a `type=note` or `type=task`
label) because `jp-ticket` offers five ergonomics that plain markdown files in
`docs/` lack:

1. **Zero-friction capture** (`jp ticket add` / `ticket_create`) without
   choosing a directory path or editing `.vitepress/config.mts`.
2. **A short handle** (`T-0n0nwz3`) that prose in other tickets and RFDs cites
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
it `Done` buries active reference material in the completed-work archive and
misstates its nature.

Meanwhile, the existing `docs/architecture/` files have the opposite problem:
they sit outside the VitePress navbar and sidebar, have a manually maintained
`index.md` that already omits newer files, carry no backlinks to the RFDs and
tickets that cite them, and do not fail the build when `#section` anchors or
relative links to source files break.

Traditional wikis solve capture but fail on maintenance: without ordering,
freshness contracts, and link checking, a wiki degrades into orphan pages, stale
claims, and broken links.
A repo-native wiki integrated into `jp`'s toolchain can prevent those failure
modes structurally.

## Design

### What a Wiki Page Is

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
The canonical reference form is `W-02wt0kx`; CLI commands and tools accept
`W02wt0kx`, the bare 7-character body, and any case.

Keeping identity in a flat `<id>-<slug>.md` filename rather than a nested
directory path gives two properties:

1. **Zero filing friction.** Creating a page requires a title, and optionally
   the page it sits under, not a decision about directory taxonomy.
2. **Rename and reorganization safety.** Renaming a page renames its `<slug>`
   and rewrites file links, while every `W-02wt0kx` citation in RFDs, tickets,
   commits, and conversations continues to resolve.

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
  `Archived` pages are omitted from the site's sidebar, omitted from default `jp
  wiki list` and `wiki_list` output unless `--archived` or `--all` is passed,
  and may carry `- **Superseded by**: W-<id>` or `RFD NNN`.

#### Parent Hierarchy and Ordering

Organization is decoupled from filenames through two mechanisms:

1. **`Parent`**: A page may name another wiki page as its parent (`- **Parent**:
   W-0a1b2c3`), forming a tree of arbitrary depth without moving files into
   subdirectories.
   A page without one is a top page, which introduces an area, such as the
   architecture or the glossary, and leads to the pages under it.
   Cycles in `Parent` links are rejected by `jp wiki check`.

2. **Ordering (`.order.json`)**: As in `jp-ticket` (`.board.json`) and `jp-rfd`
   (`.priority.json`), relational ordering lives in `docs/wiki/.order.json`
   rather than inside individual files.
   The file holds the order of the top pages, and maps each parent page ID to
   the order of the pages under it:

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
- `redirects.mjs` emits short-link redirects so `/wiki/W-02wt0kx` and
  `/wiki/02wt0kx` bounce to `/wiki/02wt0kx-slug`.
- Mention syntax supports an optional section fragment:
  `W-0a1b2c3#pinned-conversation`, `RFD 100#labels`, and `T-0n0nwz3#swot` link
  directly to that heading anchor on the target page.

#### Heading Slugs and Section Addressing

Every ATX heading (`#` through `######`) has an anchor slug computed the way the
site computes it, so a `#fragment` that `jp wiki check` accepts is one the site
renders.

`jp-vite` owns the slug function: `withJpVite` sets `markdown.anchor.slugify` to
its own copy of VitePress's default (`slugify` from `@mdit-vue/shared`), so no
existing anchor changes and a VitePress upgrade can't change one underneath.
The Rust side implements the same function, and a fixture of headings and their
slugs, shared by the JavaScript and Rust tests, holds the two in step.

The function normalizes Unicode (NFKD) and drops combining accents, replaces
runs of whitespace and punctuation (`_` included) with `-`, keeps other letters,
lowercases, and prefixes a leading digit with `_`:

| Heading                                         | Slug                                |
| ----------------------------------------------- | ----------------------------------- |
| `## Pinned Conversation`                        | `pinned-conversation`               |
| ``### 1. Query Command (`jp_cli::cmd::query`)`` | `_1-query-command-jp-cli-cmd-query` |

Slugs are unique within a document: the second `## Examples` is `examples-1`,
the third `examples-2`.
Suffixes are counted over the whole source document, so reading or embedding one
section never changes them.
Custom heading anchors (`## Turn {#turn}`) are rejected by `jp wiki check`: Git
forges and plain-text readers show them as literal text.

Commands and tools address sections by anchor slug or heading title.
An argument that matches a slug selects that section; otherwise it is matched
against heading titles, and a title shared by several headings is refused with
their slugs, so a section edit never guesses which section to replace:

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
- **In `jp wiki show --body`, and `wiki_show` with `expand_embeds`**: The Rust
  reader expands transclusions inline so the reader receives the embedded text
  in a single read.
  `wiki_show` returns the text as written unless asked, since that text is what
  `wiki_edit` replaces, and writing expanded text back would turn each embed
  into a copy.
- **In `<DocReferences />`**: Transcluding a document counts as referencing it,
  so the source page lists every page that embeds it under `Referenced by`.

An embedded section is parsed as part of its whole source document, so the site
and the Rust reader give it the same meaning wherever it appears:

- Reference-style links resolve against the source document's definitions, which
  `comfort` puts at the end of the file, outside any section.
- Relative targets resolve against the source document's directory and are
  rewritten to be valid from the embedding page.
- A `#fragment` pointing into the embedded section keeps pointing at the source
  page, not at a heading of the embedding page.
- A nested embed resolves against the document that contains it.

`wiki_show` wraps each expanded embed in markers naming its source (`<!-- embed
W-0a1b2c3#pinned-conversation -->` and `<!-- /embed -->`), so the assistant can
tell embedded text from the page's own.
On the site, a `![...](...)` pointing at a markdown document or a workspace file
that isn't a standalone embed of a section or line range fails the build rather
than rendering as a broken image.

`Snapshot` pages don't embed.
An embed shows the source's current text, so a page that must not change as the
code evolves would change on every build.
A `Snapshot` copies the excerpt it relies on and cites its source as described
in [Snapshot Citations](#snapshot-citations).

#### Deep-Linking to Workspace Source Files

Links from wiki pages, RFDs, and tickets to files in the repository use normal
relative paths, optionally carrying a 1-based line or line-range fragment:

```markdown
See [`Links::by_rfd`](../../crates/plugins/command/rfd/src/links.rs#L88-L125).
```

Line fragments accept `#L12`, `#L12-L24`, `#L12-24`, and `#12-24`, normalizing
to `#L12-L24` when rewritten by `jp-vite`.

To catch silent line drift when a source file is edited above the linked range,
when a link's label is an inline code span (such as ``[`Links::by_rfd`]`` or
``[`by_rfd`]``), `jp wiki check` takes the trailing identifier (`by_rfd`) as an
expected symbol guard and verifies that the substring still appears within the
linked line range (or within the file when no line range is given).

#### Snapshot Citations

A relative source link follows the working tree.
That is what a `Reference` page wants and what a `Snapshot` must not do: once
the code moves, the link breaks or points at code the snapshot never saw.
`Snapshot` pages therefore cite source code with a permalink to a commit:

```markdown
[`Links::by_rfd`](https://github.com/dcdpr/jp/blob/9a43a5e2dc077909a9b8fca9e58abe541b3d8cda/crates/plugins/command/rfd/src/links.rs#L88-L125)
```

A permalink is a link in every editor, on every forge, and on the site, and what
it points at never changes.
A link counts as a permalink when it names a full commit hash under the
repository `jp vite` links to (`plugins.command.vite.options.repository`, or
git's `origin` remote); any other URL is an ordinary external link.

The commit has to be reachable from the default branch (`origin/HEAD`).
A branch commit stops being reachable once its pull request is squash-merged,
and a fresh clone never fetches it.
Code that exists only on the current branch can't be pinned yet, so the page
names its path and lines as code text (`` `crates/foo/src/bar.rs` L12-24 ``)
beside the copied excerpt, and is pinned once the code lands.

`jp wiki pin <ID>` rewrites a page's relative source links into permalinks.
Each link is pinned to the newest default-branch commit whose copy of the file
matches the working tree; a link with no such commit is left alone and reported.
Creating a `Snapshot` page, or changing a page's status to `Snapshot`, pins it
the same way, through the CLI or the tools.

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
   In a `Snapshot` page, a relative link into the workspace outside `docs/` is
   an error naming `jp wiki pin`.
   A permalink's commit must exist locally and be reachable from the default
   branch, and the path, line range, and symbol guard are checked against that
   commit's copy of the file, which never changes: these checks fail on a
   mistake, never on later code changes.
6. **Transclusions**: Rejects any cycle of `![...](...#section)` embeds, and any
   embed in a `Snapshot` page.
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

`archive` refuses a page that still has non-archived children, naming them, so
no active page is left under a parent the sidebar hides.
Archiving removes the page's entries from `.order.json`.
Both hold when `wiki_edit` sets the status to `Archived`.

`jp wiki from-ticket <TICKET-ID>` moves a note ticket into the wiki the way `jp
rfd draft --from-ticket` moves one into an RFD ([RFD 116]).
It creates a page from the ticket's title and description, carrying over
`Authors`, `Date`, and the labels the wiki vocabulary accepts (naming the rest),
then closes the ticket as `Done` with `Promoted to` naming the page.
The ticket stays, so every `T-` citation in tickets, RFDs, commits, and
conversations still resolves and leads on to the page.
Its comments stay with it, with their attribution; whoever migrates the ticket
folds what matters into the page.

The page gets a new id rather than the ticket's.
An id's first five characters count time from its own directory's `.epoch` ([RFD
102]), so the same characters under the wiki's epoch would name a different time
and break creation order.
Like `draft --from-ticket`, the command is safe to repeat: a ticket whose
`Promoted to` already names an existing page reports it.

#### Token-Conscious Assistant Tools

To keep per-turn schema overhead small, `jp-wiki` exposes four tools:

1. **`wiki_list`**: Lists non-archived wiki pages in `.order.json` order, each
   top page above the pages under it, optionally filtered by `under` (the pages
   below one page), `status`, `labels`, or `query`.
   With `full: true`, includes each page's one-line `Summary`.
2. **`wiki_show`**: Returns a page's metadata, incoming/outgoing references, and
   its heading table of contents (`#slug` and title) by default.
   Passing `body: true` returns the full markdown body as written; passing
   `section: "<slug>"` returns only that section; `expand_embeds: true` expands
   the transclusions in either.
3. **`wiki_create`**: Creates a new wiki page with `title`, `summary`, and
   optional `body` (a page without one is a hub), `status` (defaulting to
   `Reference`), `parent`, and `labels`.
4. **`wiki_edit`**: Single mutation tool accepting `id` and any combination of
   `title` (which also renames the file and rewrites relative links), `summary`,
   `status`, `parent`, `before` or `after` (a sibling to place it directly above
   or below, as `jp wiki rank` does), `superseded_by`, `body`, `section`
   (scoping `body` replacement to one heading section), `add_labels`, and
   `remove_labels`.

Every tool holds to the call's access policy ([RFD 116]) file by file, not only
at the wiki directory:

- `wiki_show` checks the policy for every file an embed reads, nested embeds
  included.
  Being allowed to read a page grants nothing about what it embeds.
  A denied embed is replaced by a marker naming the target and the refusal, and
  the rest of the page is returned.
- `wiki_edit` updates only files the policy lets it update.
  A rename that leaves references in files it can't reach names them, and says
  so when the policy doesn't let it search the workspace, as `rfd_rename` does.

### Site Integration (`jp-vite`)

`jp-vite` adds `wiki` to `settings.json` alongside `rfd` and `ticket` when the
configured wiki directory exists:

- **Auto-generated hierarchical sidebar**: Unlike RFDs and tickets, which hide
  the VitePress sidebar, `withJpVite` generates `themeConfig.sidebar['/wiki/']`
  from the wiki's `Parent` tree and `.order.json`, each top page above the pages
  under it, using each page's `# Title` as the link text.
  Prev/Next navigation at the bottom of each wiki page follows this reading
  order.
- **A front page and hub pages**: `/wiki/` is the wiki's own front page, which
  `jp vite sync` creates once and never rewrites: an introduction, links to the
  pages to read first, and embeds.
  Each area opens with a top page introducing it, with the detailed pages under
  it through `Parent`.
  `<WikiChildren />` lists the pages one level down, with their summaries, in
  reading order: under the page it is on, under `of="W-0a1b2c3"`, or with no
  page around it, the top pages.
  A page with no text of its own, only a title and metadata, shows this list.
  A hub can be just that.
  `jp wiki check` counts a link from the front page as reaching a page, so a hub
  is no orphan.
- **`<WikiIndex />` on `/wiki/all`**: Wraps `<DocIndex />` with a filter button
  per top page, narrowing the table to the pages under it, status badges
  (`Reference`, `Snapshot`, `Archived`), label badges, search, summary toggles,
  and dev-server `+ New page`, `Edit`, and `Archive` actions backed by `/__wiki`
  middleware calling `jp wiki`.
- **Editing in place**: On a dev server that accepts edits, each section heading
  of a wiki page has an edit button, and the page an `Edit page` button.
  Either opens the text as written, through `jp wiki show --raw`.
  Saving runs `jp wiki edit --section`, which checks the page before keeping it.
  On the published site, every page links to the forge's editor for its file on
  the default branch, which `jp vite` reads from git's `origin/HEAD`.
- **Compact metadata**: On RFD, ticket, and wiki pages, the metadata block
  renders as one line of labels grouped by key (`domain: conversation, llm`),
  and each other field on a short line.
  A wiki page's status is a letter after its title (`R`, `S`, or `A`), its
  summary, authors, and date sit at the end of the page in small print, above
  the references, and its parent is a trail above the title (`Wiki › Glossary
  › Ubiquitous Language`), on the line with the `Edit page` button.
  An RFD or ticket keeps a line of status badge, category, date, and authors,
  and leaves its summary to the index pages.
  The file keeps the list.
- **Cross-references**: `<DocReferences />` lists, below each RFD, ticket, and
  wiki page, what it references and what references it, grouped by kind and
  named by title, folded when long.
  RFD and ticket pages show wiki pages that cite them or carry
  `relevant-rfd=NNN`.
- **Moved pages**: A `redirects` map in `plugins.command.vite.options` sends an
  old route to a new one, and may name a wiki page by id
  (`"architecture/ubiquitous-language" = "W-0a1b2c3"`), so a later rename
  doesn't break the redirect.
  `redirects.mjs` writes the stubs, which keep the `#fragment`.

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

`jp-wiki` links the `ticket` library for `from-ticket`, as `jp-rfd` does for
`draft --from-ticket`, so the ticket format keeps one implementation.

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
- **Embeds look like images on Git forges.** A forge preview treats
  `![...](...)` as an image, so where the site shows an embedded section, the
  forge shows a broken image.
  The source file stays readable and names the target.
- **Snapshots cite longer links, and wait for merges.** A permalink is long, and
  code that exists only on a branch is cited as text until it lands on the
  default branch.

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

### Configured categories beside the tree

A `Category` field naming one of a configured list (`architecture`, `glossary`,
`research`, `guide`) would group the sidebar and filter the index.
It was rejected because the `Parent` tree already does both, and a page's
category could disagree with its parent's, an error state nothing about the page
could resolve.
A top page is the category, and the pages under it are its members.

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
- **Symbol guard false positives on refactored code.** Checking that the
  identifier in ``[`foo`](path.rs#L10-L20)`` appears in lines `10..=20` catches
  line shifts, but a link whose label is prose (`[the parser](path.rs#L10-L20)`)
  only gets bounds checking, not content checking.
- **Slug drift between the site and Rust.** The two implementations of the slug
  function agree as far as the shared fixture covers.
  A heading shape it misses can yield an anchor `check` accepts and the site
  renders differently.
- **Shallow checkouts.** Checking a permalink needs its commit locally.
  `docs.yml` and `rust.yml` check out full history (`fetch-depth: 0`), but
  `plugins.yml` uses the default depth of 1, so whichever CI job runs `jp wiki
  check` needs full history.
  Without it, `check` reports the commit as missing rather than passing it.
- **Source changes that skip the check.** `jp wiki check` validates links into
  workspace source, so a pull request that changes, renames, or deletes a linked
  file has to run it, though it touches nothing under `docs/`.
  A job triggered only by documentation changes lets such a pull request pass
  and fails the next one that does touch `docs/`.

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

Move each `docs/architecture/` file into `docs/wiki/` as one page, keeping its
headings so anchors carry over; split pages afterwards, not during the move.

Rewrite references to the old paths across the whole workspace, not only
`docs/`.
Persona and knowledge configuration under `.jp/config/` and doc comments in
`jp_cli` name `docs/architecture/` files, and a missed one points the assistant
at a file that no longer exists.
Report every reference left unrewritten.
Redirect old `/architecture/*` routes to their pages through the `redirects`
map.

Move research-note tickets such as `T-0n0nwz3` with `jp wiki from-ticket`.

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


