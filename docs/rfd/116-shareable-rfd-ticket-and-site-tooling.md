# RFD 116: Shareable RFD, Ticket, and Site Tooling

- **Status**: Implemented
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-09-30
- **Extends**: [RFD 001], [RFD 100]
- **Extended by**: [RFD 127], [RFD 128]
- **Summary**: Moves ticket, RFD, and documentation-site workflows into reusable
  `jp-ticket`, `jp-rfd`, and `jp-vite` command plugins.

## Summary

This RFD moves the ticket, RFD, and documentation-site workflows into three
independently adoptable command plugins: `jp ticket`, `jp rfd`, and `jp vite`.
Each plugin reads its settings from JP's configuration, and `jp ticket` and `jp
rfd` also expose their operations as assistant tools so adopting projects
receive workflow updates by upgrading plugin binaries.

## Motivation

The workflow lives in four places today:

- The ticket format and store in `crates/internal/ticket`, and the `jp ticket`
  command plugin in `crates/plugins/command/ticket`.
- The `ticket_*` tool handlers in `.config/jp/tools/src/ticket.rs`, inside
  `jp-tools` next to unrelated file, git, and web tools.
- About 25 `rfd-*` recipes and their helpers in the justfile, written in shell
  with `jq`, `sed`, `comfort`, and `jp` as dependencies.
- The loaders, components, and styles under `docs/.vitepress/`.

The recipes have no tests, and the site's tests cover only its metadata reader.
Only the `ticket` library and `jp-ticket` binary can be shared without copying
files.

Other projects that want this workflow must copy these files and recipes and
manually sync them whenever the workflow changes.
Projects also adopt different subsets: one project may track tickets without
writing RFDs, while another has no documentation site.

Because RFD authoring and review tooling is under active development, copied
files quickly fall out of sync.

## Design

### Overview

| Plugin      | Crate                           | Owns                                                   |
| ----------- | ------------------------------- | ------------------------------------------------------ |
| `jp ticket` | `crates/plugins/command/ticket` | Tickets: commands, the board, and the `ticket_*` tools |
| `jp rfd`    | `crates/plugins/command/rfd`    | RFDs: lifecycle, board, and the `rfd_*` tools          |
| `jp vite`   | `crates/plugins/command/vite`   | Documentation site: `init`, `sync`, and site assets    |

Each plugin is a `jp-<name>` binary built as a library with a thin `main`
function, matching `jp-ticket`.
All three run only as JP command plugins: `jp` resolves the configuration and
passes it in the `init` message ([RFD 072]).
`jp-ticket` already exists; `jp-rfd` and `jp-vite` are new.
The ticket format remains in `crates/internal/ticket`, and `jp_label` remains a
separate crate shared with conversations.

Each plugin manifest declares its minimum protocol version: 12 for `jp ticket`
and `jp rfd` (which serve tools), and 10 for `jp vite` (which reloads
configuration).
Protocol 13 adds `edit_files` to `jp` so `jp ticket edit` can open files in the
user's editor.
`jp ticket` checks for protocol 13 only when running `edit`, so a host on
protocol 12 still runs every other command.

Dependencies between the plugins are unidirectional:

- `jp ticket` has no dependency on RFDs.
  An RFD appears in a ticket only as an opaque ID in the `Promoted to` field and
  in the labels that `jp rfd` reads as RFD links (see [RFDs](#rfds)).
- `jp rfd` links the `ticket` crate for the two operations that cross domains:
  managing tracking tickets and seeding a draft from a ticket.
  Both plugins write tickets through that crate, keeping a single implementation
  of the ticket format.
- `jp vite` links neither crate.
  Its dev server delegates all ticket writes to `jp ticket` (replacing the
  JavaScript board-move writer) and delegates all RFD writes to `jp rfd`, except
  for priority-board layout updates (see [Site](#site)).
  Running the site no longer requires `just`.

| Entry point                                                 | Caller                              |
| ----------------------------------------------------------- | ----------------------------------- |
| `jp ticket …`, `jp rfd …`, `jp vite …`                      | A person at the terminal            |
| `plugin.command.ticket.<tool>`, `plugin.command.rfd.<tool>` | The assistant, through plugin tools |
| `jp ticket …`, `jp rfd …`                                   | The site's dev server, for writes   |

### Configuration

Each plugin's settings live under its `plugins.command.<name>.options`:

```toml
[plugins.command.ticket]
run = "allow"

[plugins.command.ticket.options]
dir = "docs/ticket"
# Author recorded on tickets and comments filed via a tool.
assistant = "jp"
# How many Done tickets a board shows.
board.done_limit = 8

[plugins.command.ticket.options.labels.type]
description = "The type of ticket"
values = ["bug", "feature", "tracking"]

[plugins.command.rfd]
run = "allow"

[plugins.command.rfd.options]
dir = "docs/rfd"
categories = ["design", "decision", "guide", "process"]
# Author recorded on drafts created via a tool.
assistant = "jp"
# Label key linking a tracking ticket to its RFD, and extra labels added when
# filing a tracking ticket.
tracking.key = "tracking-rfd"
tracking.labels = ["type=tracking"]
# Label key linking any other ticket to a related RFD.
relevant.key = "relevant-rfd"

[plugins.command.vite]
run = "allow"

[plugins.command.vite.options]
# The VitePress source directory.
dir = "docs"
# Where links to repository files point.
repository = "https://github.com/dcdpr/jp"
```

All options are optional.
The example above shows the default values, except for four settings:
`assistant` defaults to `"assistant"`, `labels` defaults to an empty vocabulary,
`tracking.labels` defaults to `[]`, and `repository` defaults to git's `origin`
remote.
Without a label vocabulary, the ticket board accepts any `key` or `key=value`
label; with a vocabulary, it accepts only defined labels.

Each setting has a single owner.
When a plugin needs another plugin's settings, it reads them from the full
configuration in `init`: `jp rfd` reads the ticket directory and label
vocabulary from `plugins.command.ticket.options`, and `jp vite` reads the
options of all three plugins.

The label vocabulary moves from `.labels.json` into `options.labels` with the
same structure: a description, allowed values, and retired values for each key.
A key can also constrain how many of its values a single ticket may carry using
`min` and `max` (for example, `min = 1, max = 1` requires every ticket to carry
the key exactly once).
Setting `values = []` accepts any value, such as an RFD number, and `unique`
restricts a key (`"key"`) or each key-value pair (`"key-value"`) to at most one
ticket on the board.
As with any other setting, the workspace controls whether contributors may
override `options.labels` in their local configuration layers.

`run = "allow"` runs a plugin without prompting.
Plugins default to `ask`, and non-interactive runs (such as dev-server writes)
reject any plugin that is neither set to `allow` nor pre-approved with `jp
plugin approve`.
Because `jp plugin approve` pins the binary hash, projects that rebuild plugins
frequently, such as JP, set `run = "allow"` instead.
Plugin tools are authorized once at the start of a turn, when a terminal is
usually available.

The ticket and RFD directories must reside inside the VitePress source directory
(`plugins.command.vite.options.dir`), because VitePress serves pages only from
its source root; `jp vite` rejects configurations that violate this constraint.
Each directory's URL route mirrors its relative path: with `dir = "docs"`,
`docs/ticket` is served at `/ticket/`.

Layered `options` tables merge key by key, so a contributor who overrides
`assistant` inherits the workspace's `dir`.

### Tickets

The commands are the existing `jp ticket` commands, with the following changes:

- `edit` accepts `--title` and a repeatable `--label` alongside `--body` and
  `--status`, so scripts can update multiple fields in one command.
  Called with no flags, `edit` opens the ticket file in the user's editor via
  the host's `edit_files` operation (protocol 13) and validates the file when
  the editor closes.
  `rename` and `label` remain as dedicated commands for single-field updates.
- `label` adds and removes labels rather than replacing the full set, so an
  update never drops unmentioned labels: `add` appends to existing labels, `set`
  replaces all labels under the specified keys, `rm` removes a specific label
  (or all labels under a bare key), and `ls` lists the ticket's labels.
  Because labels are positional arguments, `--id` specifies the target tickets
  on either side of the subcommand.
  Repeat `--id` or pass comma-separated IDs to update multiple tickets
  atomically; when `--id` is omitted, an interactive picker prompts the user to
  select a ticket.
  A label formatted like a ticket ID (such as `T-02wt0kx`) is rejected as a
  mistyped ticket argument.
  `edit --label` remains the only command that replaces the entire label list,
  and it prints any labels it removed.
- `status ID STATUS` moves a ticket to any column; `close` remains as a
  shorthand for moving to Done.
  Passing `--before` or `--after` with a ticket already in the target column
  places the moved ticket adjacent to that neighbor, validating the neighbor
  before writing.
- The `Blocked by` field is removed without replacement; projects that track
  blocked tickets can use a label instead.
  Nothing in the tooling read `Blocked by` beyond displaying it.
- The `Kind` field is also removed, along with `add`'s kind argument and picker,
  `--kind` on `edit`, `list`, and `import`, the kind column in `list` and
  `board`, and the `kind` parameter on tools.
  Ticket kind becomes a label so each board can define its own taxonomy; JP uses
  `type`, required once on every ticket (`min = 1, max = 1`).
  `import` accepts `--label` to apply labels to imported tickets.
- `board` prints the Todo, In Progress, and Done columns in board order, capping
  Done at `board.done_limit`.
  The columns render stacked vertically rather than side by side, which would
  wrap on terminals narrower than roughly 120 columns.
- `rank ID (--before | --after) OTHER` moves a ticket adjacent to `OTHER` within
  the same column.
- `import` requires `--repo OWNER/NAME` on every call.
  Because `import` runs infrequently, projects that want a default repository
  can wrap the command.
- `init` creates the ticket directory.
- `migrate` is removed.
  Every ticket in JP already uses the collision-resistant identifiers from [RFD
  102], and no other project has legacy tickets to migrate.

Board ordering remains in `.board.json` inside the ticket directory.
`board` and `ticket_list` read `.board.json`; `rank`, `status --before|--after`,
and the site's ticket board write it.
Tickets absent from `.board.json` sort below ranked tickets, so the file
requires no initial setup and `init` does not create it.

`promote ID --to RFD` retains its behavior: it links an existing RFD and closes
the ticket.
Creating the RFD belongs to `jp rfd`.

### RFDs

The plugin ports the twelve lifecycle recipes from the justfile and adds `show`,
`rank`, and `init`:

```sh
jp rfd draft CATEGORY TITLE [--from-ticket ID]
jp rfd promote ID
jp rfd extend ID TARGET
jp rfd require ID TARGET
jp rfd supersede ID BY
jp rfd abandon ID REASON
jp rfd renumber ID [NEW]
jp rfd rename ID TITLE
jp rfd track ID
jp rfd start ID
jp rfd stop ID
jp rfd list [CATEGORY]
jp rfd show ID [--body]
jp rfd rank ID (--before OTHER | --after OTHER | --backlog)
jp rfd init [--force]
```

The justfile recipes define the expected repository changes for each command,
not the implementation structure.
Those recipes grew piecemeal: IDs are resolved in at least three places
(`_rfd-resolve`, a helper inside `_rfd-link`, and inline in `rfd-extend`), and
every recipe that modifies the metadata header runs its own `sed` or `awk`
script.
`jp rfd` consolidates these into a single ID resolver, a single metadata-header
editor, and a shared relationship writer for `extend`, `require`, and
`supersede`, while simplifying command interfaces that were shaped by shell
limitations.
Intentional behavioral changes from the recipes are:

- `draft --from-ticket` replaces `just ticket-promote`.
- Tracking tickets reference their RFD with a `tracking-rfd=116` label, and
  other related tickets use `relevant-rfd=116`, replacing the `Implements`
  metadata field and hardcoded `type=tracking` label used by the recipes.
  Both label keys (`tracking.key` and `relevant.key`) and the extra labels
  applied to new tracking tickets (`tracking.labels`) are configurable.
  `renumber` updates both labels when an RFD's ID changes, whereas the recipe
  never updated `Implements`.
- `extend` takes its arguments in the same order as `require`, with the source
  RFD first: `jp rfd extend 116 100` records that 116 extends 100, and `jp rfd
  require 116 072` records that 116 requires 072.
  `just rfd-extend NNN MMM` used the reverse order (MMM extends NNN).
  Because a reversed relationship still parses and silently alters promotion
  gates, both commands now use the same argument order.
- Promoting an RFD to Implemented removes `Requires` entries pointing to it from
  all dependent RFDs, including drafts.
  The recipe located dependents via the promoted RFD's `Required by` field,
  which omits drafts, leaving stale `Requires` entries in drafts that then
  failed the docs build.
  `jp rfd` scans the `Requires` field of every RFD directly.
- `abandon` writes the multi-line notice block specified by [RFD 001], whereas
  the recipe wrote a single-line `> **Abandoned**:` note.
- Commands insert new metadata fields above `Summary`, preserving [RFD 001]'s
  requirement that `Summary` come last.
  The recipes inserted `Superseded by` and `Supersedes` immediately after
  `Date`.
- Appending an RFD to a line-wrapped relationship field places it after the
  final wrapped line; the recipe appended it to the first line, mid-value.
- `supersede` appends to an RFD's `Supersedes` field rather than overwriting it,
  so an RFD that supersedes multiple RFDs retains all of them.
- Titles are written verbatim.
  Because the recipe constructed headings with `sed`, `/` or `&` characters
  broke the substitution and literal `AUTHOR` or `DATE` substrings were
  expanded.
- Reference rewriting matches complete slugs: a longer slug that shares the old
  slug as a prefix is left untouched, whereas the recipes rewrote the prefix.
  `rename` also updates links that omit the `.md` extension, matching `renumber`
  and `promote`.
- `jp -F json rfd list` outputs each RFD's workspace-relative `file` path
  instead of the recipe's site `path`, because `jp rfd` does not know the site's
  URL layout.
  Neither `jp rfd` nor `jp ticket` defines a `--json` flag; passing global `jp
  -F json` formats `list` and `show` output as JSON.
- ID parsing is case-insensitive and flexible: `42`, `RFD 042`, `d5`, and `D5`
  all resolve, whereas the recipe rejected `D5`.
- A missing `drafts/` directory is treated as having no drafts, and an
  unreadable `.priority.json` produces a warning and lists all RFDs as unranked.
  `just rfd-list` failed when `drafts/` was missing and silently treated an
  unreadable `.priority.json` as empty.
- Promoting an RFD to Accepted in a non-interactive session skips creating a
  tracking ticket, whereas the recipe created one unconditionally; the host
  treats an unprompted confirmation the same as a declined one.
- When multiple tracking tickets match an RFD, `track` selects the first by ID
  and emits a warning naming both, whereas the recipe took the first silently.

Three integrations replace shell subprocesses:

- `promote` formats the promoted file with `comfort` as a Rust library call
  rather than a subprocess, and offers to create a tracking ticket when
  promoting to Accepted.
- `track` extracts the Implementation Plan by requesting a structured `query`
  from the host in an expiring conversation (`schema` and `expires_in`, protocol
  11), writing the `<!-- rfd-plan: pending -->` marker if the query fails.
  This replaces the recipe's `jp query` subprocess.
- `list` assembles the priority board from RFD metadata, `.priority.json`, and
  In Progress tracking tickets, porting the read path of `assembleBoard` from
  `rfd-shared.mjs` without its validation checks.

`draft --from-ticket ID` seeds a draft from the ticket's title and description,
sets `Promoted to` on the ticket, and closes the ticket.
The command is idempotent: if the ticket already references an existing RFD,
`draft --from-ticket` reports that RFD, and if an earlier run was interrupted
after creating the draft, it locates the draft by its `Promoted from ticket`
field and links it instead of creating a duplicate.

`rank ID --before OTHER` and `--after OTHER` position `ID` adjacent to `OTHER`
on the priority board within `OTHER`'s milestone group, promoting a backlog RFD
to planned; `rank ID --backlog` moves it back to the backlog.
`rank` rejects RFDs in terminal statuses, which do not appear on the priority
board.
Milestones are created, renamed, and deleted in the web UI.

`init` creates the RFD directory, category templates, an empty `.priority.json`,
and the `drafts/` subdirectory.
When run again, it overwrites the templates as described in [Overwriting
Files](#overwriting-files); an existing `.priority.json` holds the project's
ranking and is never modified.
It does not generate an RFD 001, because each project defines its own RFD
process.

RFD tracking requires the ticket workflow.
In a repository without a ticket directory, `promote` skips the tracking-ticket
prompt, `list` reports no RFDs in development, and `track`, `start`, `stop`, and
`draft --from-ticket` fail with an error explaining that tickets are not
configured.
When the ticket board defines a label vocabulary, that vocabulary must include
the tracking key with `values = []`; until it does, `track` and `start` fail and
print the required configuration snippet, and `promote` skips the tracking
prompt with an explanation.
`show` lists all tickets linked to the RFD (tracking ticket first) and, when no
tracking ticket exists, prints the labels required to create one.

### Parity

Every operation that applies to both document types is available on both
plugins, and every command useful to the assistant is exposed as a tool.
Bold entries are new.

| Verb             | `jp rfd` CLI / tool                                       | `jp ticket` CLI / tool                        |
| ---------------- | --------------------------------------------------------- | --------------------------------------------- |
| Create           | `draft` / `rfd_draft`                                     | `add` / `ticket_create`                       |
| Show             | **`show`** / **`rfd_show`**                               | `show` / `ticket_show`                        |
| List             | `list` / **`rfd_list`**                                   | `list` / `ticket_list`                        |
| Board            | `list`                                                    | **`board`** / not a tool                      |
| Rank             | **`rank`** / not a tool                                   | **`rank`** / not a tool                       |
| Rename           | `rename` / `rfd_rename`                                   | **`rename`**, `edit` / **`ticket_edit`**      |
| Edit body        | not applicable                                            | `edit` / **`ticket_edit`**                    |
| Change status    | `promote` / `rfd_promote`                                 | **`status`**, `close` / **`ticket_edit`**     |
| Relate           | `extend`, `require`, `supersede` / same                   | not applicable                                |
| Retire           | `abandon` / `rfd_abandon`                                 | `delete` / not a tool                         |
| Renumber         | `renumber` / `rfd_renumber`                               | not applicable                                |
| Track            | `track`, `start`, `stop` / `ticket_create`, `ticket_edit` | not applicable                                |
| Discuss          | not applicable                                            | `comment` / `ticket_comment`                  |
| Label            | not applicable                                            | `label`, `labels`, `edit` / **`ticket_edit`** |
| Ticket to RFD    | **`draft --from-ticket`** / `rfd_draft` with **`ticket`** | `promote --to` / not a tool                   |
| Tool definitions | **`tools init`**, **`tools sync`**                        | **`tools init`**, **`tools sync`**            |

Because RFD tracking operates on tickets, the assistant manages tracking tickets
through the ticket tools: `rfd_show` reports the labels required for a missing
tracking ticket, `ticket_create` creates it, and `ticket_edit` updates its
status to start or stop development.
Dedicated RFD tracking tools would add three extra tool definitions to every
turn.

A single tool, `ticket_edit`, handles all ticket modifications: it accepts
`title`, `description`, `status`, `add_labels`, and `remove_labels`, updating
only the specified fields.
Labels are added and removed incrementally so a call never drops unmentioned
labels, and a single-valued label key can be replaced in one call by passing the
bare key in `remove_labels` and the new `key=value` pair in `add_labels`.
Because every tool definition consumes context on every turn and providers cap
total tool counts, consolidating mutations into `ticket_edit` saves context; CLI
subcommands incur no per-turn cost and remain separate.

For the same reason, `ticket_show` returns only a ticket's metadata unless
`description` or `comments` are requested, and notes any omitted sections at the
end of its output.
No separate tool lists the label vocabulary, because the label parameter schemas
on `ticket_create`, `ticket_edit`, and `ticket_list` already enumerate the
allowed values.
`ticket_list` returns tickets in board order (making the first Todo entry the
highest-priority item), paginates long results, and supports a `query` parameter
that searches ticket titles, descriptions, and comments.

`import`, `refresh`, `delete`, `board`, and `rank` remain CLI-only.
`import` fetches from GitHub over the network, `refresh` rewrites tickets
introduced on the current branch, and deleting or reprioritizing tickets is left
to the user.
`board` provides no information beyond what `ticket_list` already returns.

Tools never open the host's interactive composer or picker; a missing required
argument returns an error naming that argument.

### Tools

`jp ticket` and `jp rfd` serve their tools as command-plugin tools ([RFD
072][RFD 072-2]).
For each tool call, JP spawns the plugin and passes the tool invocation and
resolved plugin options in `init`, including overrides from nested `.jp.toml`
files, conversation configuration, and `--cfg`.
`ticket.rs` is removed from `jp-tools`.

```toml
[conversation.tools.ticket_create]
source = "plugin.command.ticket.create"
style.parameters = "tool"
```

Within the plugin, tool names omit the domain prefix (`create`, `show`, `list`),
because the `source` path already identifies the plugin.
Setting `style.parameters = "tool"` delegates call-preview formatting to the
plugin, preserving `ticket_create`'s existing ticket preview.
When a tool prompts the user with a question, the plugin process exits and JP
re-invokes it with the user's answer, so plugins maintain no state across calls.
The tools preserve their existing behavior, including `ticket_create`'s prompt
for a shorter title when the generated filename would exceed
`options.max_title_length`.
Every tool enforces the call's filesystem access policy, refusing to read or
write a document directory that the policy does not grant (a check the
`jp-tools` handlers omitted).

The plugin defines tool behavior, while each project controls its tool
configuration.
A project can expose `create` under the name `track`, customize its `summary`
text, and set `options.max_body_words`.
The `source` field connects the two: `[conversation.tools.track]` with `source =
"plugin.command.ticket.create"` invokes the plugin's `create` handler.

`tools init PATH` writes default tool definitions into the directory at `PATH`,
one file per tool, overwriting any existing files that differ as described in
[Overwriting Files](#overwriting-files).
`PATH` has no default because each project chooses where to store its tool
configurations; the workspace configuration must load `PATH` for the tools to
take effect.

`tools sync PATH` updates the generated parameter schemas to match the project's
configuration: the allowed label tokens on `ticket_*` tools, and the allowed
categories on `rfd_draft` and `rfd_list`.
It matches tools by `source` rather than by their table key, so both
`[conversation.tools.ticket_create]` and `[conversation.tools.track]` with
`source = "plugin.command.ticket.create"` receive updated label tokens under
their respective keys.
`tools sync` writes only to tools whose base definition already declares the
parameter, warning about any skipped tools (writing enum values without a base
parameter type would produce an untyped parameter and cause JP to drop the tool
from conversations).
When a label key defines `min` or `max` bounds, `tools sync` documents those
bounds in the `ticket_create` and `ticket_edit` parameter descriptions and marks
`labels` as required on `ticket_create` whenever a key sets `min >= 1`, so the
model sees the constraint before its first call.
`tools sync` overwrites only its own generated file in full, preserving
hand-edited tool definitions while removing entries for deleted tools.
The file is overwritten as described in [Overwriting Files](#overwriting-files).
A file at that path written by another plugin's `tools sync` or by hand is
overwritten only with `--force`, even when git could restore it, so plugins
pointed at the same directory do not overwrite each other's file on every sync.
It replaces `just ticket-labels-sync`.

Both `tools init` and `tools sync` will be retired once plugins can provide
their own tool definitions directly (T-0vmm72y).

### Site

`jp vite init` initializes a VitePress setup in the source directory of an empty
or existing repository, creating `package.json`, `.vitepress/config.mts`, the
theme entry point, and a homepage.
It prompts for the site's title and description via the host's composer and
writes them into `config.mts` and the homepage, so a new site does not start
with JP's branding.
When run again, it prompts for the title and description again and overwrites
the files as described in [Overwriting Files](#overwriting-files), because it
needs both values to determine whether `config.mts` and the homepage already
match the expected content.

Site files divide into two categories:

- **Owned by `jp vite`.** Everything under `.vitepress/jp-vite/`: the RFD and
  ticket data loaders, Vue components, styles, dev-server middleware, a config
  fragment, and the settings snapshot (`jp-vite/settings.json`).
- **Owned by the project.** `config.mts`, the theme entry point, the homepage,
  and all content pages (including the RFD and ticket landing pages).
  `config.mts` imports and merges the `jp-vite` config fragment, and the theme
  entry point registers the components from `jp-vite/theme`.
  `sync` never modifies these files; only re-running `init` updates them.

`sync` creates any missing workflow landing pages: the index, priority board,
and drafts index pages in the RFD directory, and the index and board pages in
the ticket directory.
Existing pages are never modified, so projects can add custom prose around the
embedded component.
Each page renders its view using a globally registered Vue component, regardless
of where the page sits in the directory tree:

- `<RfdIndex />` and `<RfdDraftIndex />` for published and draft RFDs.
- `<RfdBoard />` for the RFD priority board.
- `<TicketIndex />` and `<TicketBoard />` for the ticket list and kanban board.

The RFD and ticket data loaders always ship together because the RFD priority
board reads tracking tickets.
When no ticket directory exists, the ticket loader returns an empty list,
matching its existing behavior.

The `jp-vite`-owned files live under `crates/plugins/command/vite/site/` and are
embedded into the `jp-vite` binary at compile time.
JP syncs its own site from the binary like any external project, so component
edits happen in the crate source rather than under `docs/.vitepress/jp-vite/`.

Because static site builds (`vitepress build` in CI) run without `jp`, the data
loaders must read documents, filter by category, and validate labels without
querying JP's configuration at build time.
`sync` therefore writes the resolved configuration into `jp-vite/settings.json`:
the RFD and ticket directory paths, RFD categories, label vocabulary,
`board.done_limit`, repository URL, and the git revision from which `jp-vite`
was built.
Repository file links point to `/blob/HEAD/` because `repository` does not
specify a branch (replacing the hardcoded `main` branch in JP's existing site).

`jp vite sync` writes all `jp-vite`-owned files (including `settings.json`) from
the binary and creates any missing workflow pages, including pages for workflows
enabled since the previous sync.
Before writing, `sync` lists the target files and prompts once for confirmation.
It overwrites or removes owned files as described in [Overwriting
Files](#overwriting-files).
On a clean working tree, `git diff` shows the synced changes and `git checkout`
restores the previous version.
Passing `--yes` skips the confirmation prompt for scripts without implying
`--force`.

`sync --check` performs no writes and exits non-zero if any owned file differs
from what `sync` would write or if any workflow page is missing.
CI jobs with `jp` installed can run `jp vite sync --check` to catch
configuration changes committed without a corresponding `sync`.

`jp vite serve` runs the development server.
It executes the [Version Skew](#version-skew) checks, spawns `vitepress dev` as
a child process, and terminates the child process on exit.
While running, `jp vite serve` polls the host for configuration changes every
two seconds (`read_config` with `reload`, protocol 10) and updates
`jp-vite/settings.json` whenever the relevant settings change, propagating
directory or label-vocabulary edits to the live site without a restart.
The VitePress loaders watch `settings.json`, and the dev-server middleware reads
it on every request.
Because `settings.json` is never edited by hand, live updates overwrite it
without triggering `sync`'s dirty-file check.

The dev-server middleware accepts write requests only when started by `jp vite
serve`, which sets an environment variable on the child process; a `vitepress
dev` server started directly is read-only.
Every write command receives the directory paths from `settings.json` as `--cfg`
overrides, ensuring that a configuration change between polls cannot redirect
writes away from the directory currently displayed in the browser.
The RFD priority board writes `.priority.json` directly to the RFD directory
specified in `settings.json`, because it posts the complete board layout
(including milestones), which no `jp rfd` CLI subcommand accepts.

Dragging a card on the ticket board runs a single command: `jp ticket status`
with `--before` or `--after` naming the adjacent card, updating the ticket's
column and rank atomically.
Only the moved card's rank is updated, whereas the existing JavaScript handler
rewrites the entire ordering of every column.

The RFD priority board never writes to tickets.
It displays which RFDs are in development and links each RFD that has associated
tickets to a filtered view of the ticket board (`?any-label=tracking-rfd=116`
and `&any-label=relevant-rfd=116`, using the configured label keys).
The ticket board filters by label without any RFD-specific logic: `?label=`
matches tickets carrying all specified labels, and `?any-label=` matches tickets
carrying at least one.
Contributors start work on an RFD by dragging its tracking ticket into the In
Progress column on the ticket board.
Dev-server write requests invoke `jp` without a terminal and kill any subprocess
that runs longer than 15 seconds.

All styling and component updates land in `jp vite` and reach adopting projects
on their next `jp vite sync`.

### Overwriting Files

All commands that write configuration, tool definitions, templates, or site
files into a project share a single overwrite policy (`jp ticket tools init` and
`tools sync`, the corresponding commands in `jp rfd` and `jp wiki`, `jp rfd
init`, and `jp vite init` and `sync`).
When a target file already exists with different contents:

- In a git work tree, tracked files with no staged or unstaged changes since
  `HEAD` can be restored by git and are overwritten.
- Modified (staged or unstaged), untracked, and ignored files are refused.
- Files marked `assume-unchanged` or `skip-worktree` are refused, because `git
  status` ignores local edits to them.
- Files reached through a symlink (the file itself or an ancestor directory) are
  refused, because git tracks only the symlink target path, not the target's
  contents, and writing follows the symlink.
- Outside a git work tree, existing files cannot be restored by git and are
  refused.
- `--force` overwrites refused files anyway, following symlinks to their
  targets.

Missing files and files that already match the target contents (ignoring `CRLF`
vs. `LF` line endings) are never refused and are not rewritten when unchanged.
When any file is refused, the command lists each file and the reason git cannot
restore it, and exits without writing any files.
The shared `overwrite` crate in `crates/internal/overwrite` implements these
checks.

Files that hold project state rather than shipped templates or assets, such as
`.priority.json` and directory epoch files, are created when missing and never
overwritten.

### Version Skew

Because the synced VitePress loaders parse files written by `jp ticket` and `jp
rfd`, running plugins built from different git revisions can cause format
mismatches.
Installing all plugins in a single command at the same revision avoids skew (see
[Installation](#installation)).

When a mismatch occurs, `jp vite serve` surfaces the error instead of misreading
files.
Before spawning `vitepress dev`, `jp vite serve` runs two checks:

- For each workflow whose document directory exists, it compares its own build
  revision against the revision reported by `jp ticket` or `jp rfd`.
  If the corresponding plugin binary is missing for an existing directory, the
  check fails and prints the installation command; a tickets-only repository
  does not require `jp rfd`.
- It compares every `jp-vite`-owned file (except `settings.json`) against the
  embedded version in the binary, catching stale loaders left behind after
  upgrading `jp-vite`.

If either check fails, `vitepress dev` still starts, but the RFD and ticket
pages render the error message and remediation command in place of their normal
content, and the dev-server middleware rejects all write requests.

### Validation

Site validation remains in JavaScript for now: duplicate IDs, relationship-graph
integrity and status gates, board consistency, required `Summary` fields,
unknown labels (checked against the vocabulary in `settings.json`), and RFD link
labels on tickets that reference a nonexistent RFD or use a non-canonical ID
format.
The ticket-to-RFD link check runs inside the RFD loaders, which already read
tickets, keeping the ticket loaders independent of RFDs.
Because the loaders are `jp-vite`-owned files, `jp vite sync` distributes the
validation logic unchanged.

New RFD authoring and review tooling is under development and includes its own
validator.
Porting RFD validation to Rust is deferred until that tooling settles and will
be covered in a separate RFD.

Ticket validation is the exception because its rules already live in Rust: every
label mutation validates against the configured vocabulary, `min`/`max` bounds,
and `unique` constraints.
`jp ticket check` runs the same validation across all existing tickets, allowing
projects to catch invalid hand-edits or tickets created before a constraint was
added without running a full site build.
The ticket loaders retain their JavaScript validation for static `vitepress
build` runs that execute without `jp`.

### Installation

```sh
cargo install --git https://github.com/dcdpr/jp --rev <sha> jp-ticket jp-rfd jp-vite
```

Adopting projects install the plugins they need alongside `jp` in a single
command so all binaries share the same git revision.
JP installs them from the local workspace, matching how it installs `jp-ticket`.
Pinning `--rev <sha>` makes plugin upgrades deliberate changes reviewed in the
same commit as the `jp vite sync` output they produce.

## Drawbacks

- **The port is large.** The twelve recipes and their helpers span roughly 1,500
  lines of shell whose behavior and edge cases must be reproduced: `renumber`
  alone handles duplicate IDs after a branch merge, draft-only tokens, and
  references outside `docs/`.
- **Requires JP.** Projects must run these workflows through `jp`.
  Every dev-server write spawns a `jp` subprocess, and every tool call spawns
  the plugin binary.
- **Three binaries to keep in sync.** Upgrading one plugin without the others
  can introduce format mismatches.
  `jp vite serve` detects version skew, but only when starting the dev server.
- **Configuration is duplicated in `settings.json`.** The site reads a committed
  snapshot of the configuration.
  `jp vite serve` updates the snapshot while running, but a configuration change
  committed without running `jp vite sync` remains stale until `sync --check`
  catches it in CI.
- **Validation spans two languages.** Until RFD validation moves to Rust, RFD
  rules live across both the Rust plugin and the synced JavaScript loaders.
- **Adopting projects track JP's repository.** Projects pin a JP commit to pick
  up tooling fixes, coupling their upgrades to JP's `main` branch.

## Alternatives

**Single binary.** A single `jp-todo` binary combining the ticket, RFD, and site
workflows under `rfd`, `ticket`, and `docs` subcommands.
A single binary eliminates version skew across plugins, but forces every project
to install all three workflows, changes `jp ticket` to `jp todo ticket`, and
couples three distinct concerns into one binary.
We can revisit this if version skew proves burdensome in practice.

**Standalone binaries.** Running `jp-ticket`, `jp-rfd`, and `jp-vite` without
`jp`, configured via a dedicated `.jp-todo.toml` file.
That approach introduces a second configuration file that all three binaries and
the site must locate and parse, and loses access to the host's interactive
composer and picker.
We can revisit this if a project needs the workflows without JP.

**Keep the label vocabulary in `.labels.json`.** A standalone file that the
static site build can read directly, but it splits configuration across two
files when `jp-vite/settings.json` already passes the resolved vocabulary to the
static build.

**Continue copying files.** The status quo, which causes the maintenance drift
described in [Motivation](#motivation).

**A shared justfile module.** `just` only imports local files, so each project
would still have to copy and sync an untested shell file.

**A separate repository.** The plugins depend on `jp_plugin`, `jp_tool`,
`jp_label`, and `comfort`.
Hosting the plugins in a separate repository would require publishing and
versioning all four internal crates for a single external consumer.
We can revisit this once a second external consumer needs the plugins.

**An npm package for site assets.** Packaging the Vue components and styles on
npm matches standard VitePress conventions, but versions the site assets
separately from the Rust plugins, outside the single `cargo install` command
that keeps all three in sync.

**Three-way merge during `sync`.** Recording a manifest of file hashes would let
`jp vite sync` distinguish local edits from upstream changes.
Refusing to overwrite dirty files and relying on git for committed files covers
the same safety guarantees without maintaining a hash manifest, so v1 omits it.

## Non-Goals

- **Porting RFD validation to Rust.** See [Validation](#validation).
- **Porting the remaining justfile recipes.** `rfd-board-prune` and other
  maintenance recipes stay in JP's justfile for a future change.
- **Standalone execution without `jp`.** See [Alternatives](#alternatives).
- **Managing milestones from the CLI.** See [RFDs](#rfds).
- **Ticket ID migration.** No project has legacy tickets left to migrate.
- **Publishing crates to crates.io.**

## Risks and Open Questions

- **CI builds without `jp`.** A static site build in CI that does not install
  `jp` runs neither the version-skew check nor `sync --check`.
  A stale or mismatched loader can fail the build or misparse documents without
  an actionable error message.
- **Changing document directories.** Changing `dir` in configuration does not
  move existing files or update links; the project must move the files and
  update links manually.
- **Configuration polling.** `jp vite serve` polls for configuration changes on
  a timer because the host protocol cannot yet push configuration change events
  (T-0vmm72x).
  If a configuration file is read mid-save and fails to parse, both the host and
  `settings.json` retain the last valid configuration.
- **VitePress version drift.** The synced site files target a specific VitePress
  version.
  `jp vite init` writes `package.json` once, and `sync` never updates it, so a
  project's installed VitePress version can fall behind what the synced files
  expect.

## Implementation Plan

### Phase 1: `jp ticket` configuration and commands

Read `dir`, `assistant`, and `board.done_limit` from
`plugins.command.ticket.options`.
Add `rename`, `status`, `block`, `board`, `rank`, and `init`; remove `migrate`;
require `--repo` on `import` and pass it from the `ticket-import` recipe.
Serve the `ticket_*` tools as plugin tools (preserving `ticket_create`'s title
prompt and preview), add `tools init` and `tools sync`, and remove `ticket.rs`
from `jp-tools` along with `just ticket-labels-sync`.
Point JP's ticket tool configuration at `plugin.command.ticket`.

The label vocabulary remains in `.labels.json` until Phase 5 provides
`settings.json` for the site.
JP keeps the default ticket directory path so the site's hardcoded paths
continue to work until Phase 5.

This phase can merge independently.

### Phase 2: Reading RFDs

Create `jp-rfd`, reading `dir` and `categories` from
`plugins.command.rfd.options`.
Port the read path of `assembleBoard` from `rfd-shared.mjs`.
Add the `list`, `show`, `tools init`, and `tools sync` subcommands, along with
the `rfd_list` and `rfd_show` tools.

Depends on Phase 1.

### Phase 3: RFD lifecycle

Implement the shared ID resolver, metadata-header editor, and relationship
writer.
Port `draft`, `rename`, `renumber`, `extend`, `require`, `supersede`, `abandon`,
and `promote` (along with their corresponding tools), and add `rank` and `init`.
Port `just ticket-promote` into `draft --from-ticket`.
Replace JP's justfile recipes with one-line wrappers around `jp rfd` so both
entry points work while the port is validated (`just rfd-extend NNN MMM` calls
`jp rfd extend MMM NNN`).
Rename the `rfd_extend` tool parameters to match the new argument order.

Depends on Phase 2.

### Phase 4: Tracking

Port `track`, `start`, and `stop` (and their tools), with `track` extracting the
Implementation Plan via the host's structured `query` request.
Update the dev server's `/__rfd-development` endpoint to call `jp rfd`.

Depends on Phase 3.

### Phase 5: Site

Create `jp-vite`.
Move JP's VitePress files into `crates/plugins/command/vite/site/`, embed them
in the binary, implement `init`, `sync`, `sync --check`, `settings.json`, and
`serve` (including version-skew checks), and point JP's docs dev recipe at `jp
vite serve`.
Update the ticket board middleware to call `jp ticket status` and `jp ticket
rank`, and point the priority board writer at the RFD directory from
`settings.json`.
The `/__rfd-development` endpoint continues using the justfile recipes until
Phase 4 lands.
Move the label vocabulary from `.labels.json` into
`plugins.command.ticket.options.labels`.
Verify JP's synced site files with a `jp-vite` integration test,
`jp_site_is_synced`, triggered in CI whenever the embedded site files, the
synced copy under `docs/`, or `.jp/config.toml` change (running `sync --check`
in the docs CI workflow would require compiling `jp` on every documentation
edit).
JP's own documentation site becomes the first consumer synced from `jp-vite`.

Depends on Phase 2 so the version-skew check finds `jp rfd` for JP's RFD
directory.
Independent of Phases 3 and 4.

### Phase 6: Retire the recipes

Remove the ported justfile recipes (including `ticket-promote`) and point JP's
RFD tool definitions at `plugin.command.rfd`.
Keep `just rfd-promote` as a wrapper around `jp rfd promote`, because promoting
a draft in JP also archives the review and triage conversations named after the
draft ID; JP's `rfd_promote` tool continues calling `just rfd-promote` for the
same reason.
The remaining `ticket-*` recipes continue calling `jp ticket`.
Update [RFD 001] and [RFD 100] to replace references to `just rfd-*`, `just
ticket-promote`, `just ticket-labels-sync`, and `.labels.json`.

Depends on Phases 1 through 5.

## References

- [RFD 001: The JP RFD Process][RFD 001], the RFD lifecycle implemented by `jp
  rfd`.
- [RFD 072: Command Plugin System][RFD 072], the plugin protocol, the `init`
  message that delivers resolved configuration, and plugin tool execution.
- [RFD 077: Plugin Configuration and Trust Policy][RFD 077], the `options` and
  `run` settings used to configure and authorize command plugins.
- [RFD 100: In-Repo Ticket Tracking][RFD 100], the ticket file format and label
  system.
- [RFD 102: Collision-Resistant Ticket Identifiers][RFD 102].

[RFD 001]: 001-jp-rfd-process.md
[RFD 072]: 072-command-plugin-system.md
[RFD 072-2]: 072-command-plugin-system.md#tool-calls
[RFD 077]: 077-plugin-configuration-and-trust-policy.md
[RFD 100]: 100-in-repo-ticket-tracking.md
[RFD 102]: 102-collision-resistant-ticket-identifiers.md
[RFD 127]: 127-in-repo-project-wiki.md
[RFD 128]: 128-workspace-source-explorer-in-jp-vite.md
