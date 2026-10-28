# RFD 116: Shareable RFD, Ticket, and Site Tooling

- **Status**: Implemented
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-09-30
- **Extends**: [RFD 001], [RFD 100]
- **Summary**: Three `jp` command plugins, `ticket`, `rfd`, and `vite`, own the
  ticket workflow, the RFD workflow, and the site that renders them, configured
  through JP so any project can adopt the parts it uses.

## Summary

The RFD and ticket workflow moves into three `jp` command plugins that a project
adopts independently.
`jp ticket` manages tickets, `jp rfd` manages RFDs, and `jp vite` bootstraps and
updates the VitePress pages that render both.
Each reads its settings from JP's configuration, and the first two also serve
tools to the assistant.
A project that adopts them gets every later change by upgrading binaries rather
than by porting files.

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
Only the first two can be shared without copying.

This workflow is useful in other projects too, and right now the only solution
is to copy all of the above files and recipes into each project, having to sync
them again when the workflow changes.
Not every project wants all of it either: one tracks tickets without writing
RFDs, another has no documentation site.

The workflow keeps changing, since a new way of writing and reviewing RFDs is in
development, so the copies would fall behind faster, not slower.

## Design

### Overview

| Plugin      | Crate                           | Owns                                                   |
| ----------- | ------------------------------- | ------------------------------------------------------ |
| `jp ticket` | `crates/plugins/command/ticket` | Tickets: commands, the board, and the `ticket_*` tools |
| `jp rfd`    | `crates/plugins/command/rfd`    | RFDs: lifecycle, board, and the `rfd_*` tools          |
| `jp vite`   | `crates/plugins/command/vite`   | The site: `init`, `sync`, and its files                |

Each is a `jp-<name>` binary, built as a library with a thin `main` the way
`jp-ticket` is today, and each runs only as a plugin: `jp` resolves the
configuration and hands it over in the `init` message ([RFD 072]).
`jp-ticket` exists; `jp-rfd` and `jp-vite` are new.
The ticket format stays in `crates/internal/ticket`, and `jp_label` stays a
separate crate, because conversations use it too.

Each declares in its manifest the lowest plugin protocol it needs: 12 for `jp
ticket` and `jp rfd`, which serve tools, and 10 for `jp vite`, which reloads
configuration.
One thing is new in `jp`: protocol 13 adds `edit_files`, which opens files in
the user's editor for `jp ticket edit`.
`jp ticket` checks for it when it needs it, so a `jp` speaking 12 still runs
every other command.

The dependencies run one way:

- `jp ticket` knows nothing about RFDs.
  An RFD appears in a ticket only as an opaque id: in `Promoted to`, and in the
  labels `jp rfd` reads as links to an RFD (see [RFDs](#rfds)).
- `jp rfd` links the `ticket` library for the two things that cross over:
  tracking tickets, and seeding a draft from a ticket.
  Both plugins write through that library, so the ticket format still has one
  implementation.
- `jp vite` links neither.
  Its dev server makes every ticket write through `jp ticket`, including the
  board moves it writes itself in JavaScript today, and every RFD write through
  `jp rfd` except the priority board's (see [Site](#site)).
  Running the site no longer needs `just`.

| Entry point                                                 | Caller                              |
| ----------------------------------------------------------- | ----------------------------------- |
| `jp ticket …`, `jp rfd …`, `jp vite …`                      | A person at the terminal            |
| `plugin.command.ticket.<tool>`, `plugin.command.rfd.<tool>` | The assistant, through plugin tools |
| `jp ticket …`, `jp rfd …`                                   | The site's dev server, to write     |

### Configuration

Each plugin's settings live under its `plugins.command.<name>.options`:

```toml
[plugins.command.ticket]
run = "allow"

[plugins.command.ticket.options]
dir = "docs/ticket"
# The author written on tickets and comments filed through a tool.
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
# The label a tracking ticket names its RFD under, and the labels it is filed
# with besides.
tracking.key = "tracking-rfd"
tracking.labels = ["type=tracking"]
# The label any other ticket names an RFD its work belongs to under.
relevant.key = "relevant-rfd"

[plugins.command.vite]
run = "allow"

[plugins.command.vite.options]
# The VitePress source directory.
dir = "docs"
# Where links to repository files point.
repository = "https://github.com/dcdpr/jp"
```

Every option is optional, and the values shown are the defaults except four:
`assistant` defaults to `assistant`, `labels` to no vocabulary,
`tracking.labels` to none, and `repository` to git's `origin` remote.
A board without a vocabulary takes any `key` or `key=value` label; one with a
vocabulary takes only the labels it defines.

Each setting has one owner.
A plugin that needs another's setting reads it from the full configuration in
`init`: `jp rfd` takes the ticket directory and vocabulary from
`plugins.command.ticket.options`, and `jp vite` reads all three plugins'
options.

The label vocabulary moves out of `.labels.json` into `options.labels`, in the
same shape: a description per key, its values, and the values retired from it.
A key can also bound how many of its values one ticket carries, with `min` and
`max`, so `min = 1, max = 1` makes every ticket carry exactly one.
`values = []` makes a key take any value, such as an RFD number, and `unique`
holds a key (`"key"`) or each of its values (`"key-value"`) to one ticket on the
board.
Whether contributors may redefine it in their own config layers is the
workspace's call, as for any other setting.

`run = "allow"` admits a plugin without asking.
Every plugin defaults to `ask`, and a run with no terminal to ask on, such as a
dev-server write, refuses a plugin that is neither allowed nor approved with `jp
plugin approve`.
An approval is pinned to the binary's contents, so a project that rebuilds the
plugins often, as JP does, sets `allow` instead.
A plugin's tools are admitted once when a turn starts, where a terminal usually
is.

Both document directories must sit inside the VitePress source directory,
because VitePress serves pages from nowhere else, and `jp vite` refuses a
configuration that breaks this.
A directory's route follows its path: with `dir = "docs"`, `docs/ticket` is
served at `/ticket/`.

Layered `options` merge key by key, so a contributor who overrides `assistant`
keeps the workspace's `dir`.

### Tickets

The commands are the existing `jp ticket` commands, with these changes:

- `edit` takes `--title` and a repeatable `--label` beside `--body` and
  `--status`, so a script changes several parts in one command.
  With none of them, it opens the ticket's file in the user's editor, through
  the host's `edit_files` (protocol 13), and checks the file when the editor
  closes.
  `rename` and `label` stay, as the commands to find for changing one part.
- `label` adds and removes labels rather than replacing them, so a write never
  drops a label it doesn't name: its `add` keeps what a ticket carries, `set`
  replaces what the keys it names hold, `rm` takes a label or, for a bare key,
  every label under it, and `ls` lists them.
  The labels are bare arguments, so `--id` names the tickets, on either side of
  the verb; repeat it, or separate ids with commas, to change several tickets at
  once, all of them or none.
  Without it, the user picks one.
  A label written as a ticket id, `T-02wt0kx`, is refused, since it was meant as
  a ticket.
  `edit --label` stays the one write that sets the whole list, and names the
  labels it removed.
- `status ID STATUS` moves a ticket to any column; `close` stays as the
  shorthand for Done.
  With `--before` or `--after` naming a ticket already in that column, it lands
  next to that ticket; the neighbour is checked before anything is written.
- The `Blocked by` field is dropped, with nothing in its place: a project that
  marks blocked tickets does it with a label.
  Nothing read the field beyond displaying it.
- The `Kind` field is dropped too, with `add`'s kind argument and picker,
  `--kind` on `edit`, `list`, and `import`, the kind column of `list` and
  `board`, and the tools' `kind` parameters.
  What kind of work a ticket is becomes a label, so each board defines its own
  kinds; JP's is `type`, which every ticket carries once (`min = 1, max = 1`).
  `import` takes `--label` for the tickets it creates.
- `board` prints the board: Todo, In Progress, and Done in turn, each in board
  order, with Done capped at `board.done_limit`.
  The columns are stacked rather than side by side, which would wrap below about
  120 terminal columns.
- `rank ID (--before | --after) OTHER` moves a ticket next to another in the
  same column.
- `import` takes the repository on every call, as `--repo OWNER/NAME`.
  It is rarely used, and a project that wants a default writes a wrapper.
- `init` creates the ticket directory.
- `migrate` is dropped.
  Every ticket in JP already uses the collision-resistant identifiers of [RFD
  102], and no other project has tickets to migrate.

The board order stays in `.board.json` in the ticket directory.
`board` and `ticket_list` read it; `rank`, `status --before|--after`, and the
site's board write it.
A ticket the file doesn't name sorts below the ranked ones, so the file needs no
setup and `init` doesn't create it.

`promote ID --to RFD` keeps its meaning: it links an RFD that already exists and
closes the ticket.
Creating the RFD is `jp rfd`'s job.

### RFDs

The commands are the twelve lifecycle recipes this RFD ports, plus `show`,
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
jp rfd init
```

The recipes are the reference for what each command does to the repository,
which files change and how, not for how it does it.
They grew one at a time: ids are resolved in at least three places
(`_rfd-resolve`, a helper inside `_rfd-link`, and inline in `rfd-extend`), and
every recipe that touches the metadata header edits it with its own `sed` or
`awk`.
The port has one id resolver, one metadata-header editor, and one relationship
writer behind `extend`, `require`, and `supersede`, and it simplifies an
interface wherever the recipe's shape came from shell rather than from the
workflow.
An outcome that changes on purpose is listed in this RFD before it lands.
The ones made here:

- `draft --from-ticket` replaces `just ticket-promote`.
- A tracking ticket names its RFD in a label, `tracking-rfd=116`, and any other
  ticket whose work belongs to an RFD in another, `relevant-rfd=116`, where the
  recipes used an `Implements` field and a `type=tracking` label.
  Both keys are settings, `tracking.key` and `relevant.key`, as are the labels a
  tracking ticket is filed with besides, `tracking.labels`.
  `renumber` rewrites both labels along with the RFD's id, which the recipe
  never did for `Implements`.
- `extend` takes its arguments in the same order as `require`, source first.
  `jp rfd extend 116 100` records that 116 extends 100, and `jp rfd require 116
  072` that 116 requires 072.
  `just rfd-extend NNN MMM` reads the other way round, meaning MMM extends NNN.
  A relationship written backwards still parses and silently changes the
  promotion gates, so the two commands share one order.
- `promote` to Implemented removes the `Requires` entries that name the promoted
  RFD from every dependent, drafts included.
  The recipe finds dependents through the promoted RFD's `Required by` line,
  which never lists a draft, so a draft's `Requires` survives and fails the docs
  build.
  The port finds dependents by reading every RFD's `Requires`.
- `abandon` writes the notice block [RFD 001] specifies, where the recipe added
  a one-line `> **Abandoned**:` note.
- A field a command adds goes above `Summary`, which [RFD 001] keeps last.
  The recipes put `Superseded by` and `Supersedes` after `Date`.
- An RFD added to a relationship field that wraps goes after its last line; the
  recipe appended it to the first, inside the wrapped value.
- `supersede` adds to an RFD's `Supersedes` rather than replacing it, so an RFD
  that supersedes two keeps both.
- A title is written as given.
  The recipe built the heading with `sed`, so a `/` or `&` in a title broke it,
  and `AUTHOR` or `DATE` in one was replaced.
- A rewritten reference matches the whole slug: a longer slug that starts with
  the old one is left alone, where the recipes rewrote its start.
  `rename` rewrites a link written without `.md` too, as `renumber` and
  `promote` do.
- `jp -F json rfd list` gives each RFD's `file`, relative to the workspace,
  where the recipe gave its site `path`: `jp rfd` doesn't know where a site
  serves it.
  Neither plugin has a `--json` flag: `jp -F json` makes `list` and `show` print
  JSON.
- Ids are read leniently, so `42`, `RFD 042`, `d5`, and `D5` each name an RFD.
  The recipe found no draft by `D5`.
- A missing `drafts/` directory holds no drafts, and an unreadable
  `.priority.json` is reported, with every RFD listed as unranked.
  `just rfd-list` failed on the first and read the second as empty.
- Accepting an RFD with nobody to ask files no tracking ticket, where the recipe
  filed one: the host answers a prompt nobody saw the same way as one cancelled.
- `track` uses the first of two tracking tickets by id and warns, naming both,
  where the recipe took the first silently.

Three delegations carry over:

- `promote` reflows the promoted file with `comfort` and offers to file a
  tracking ticket at Accepted.
  `comfort` becomes a library call rather than a subprocess.
- `track` reads the Implementation Plan by asking the host for a structured
  `query` in a conversation that expires (`schema` and `expires_in`, protocol
  11), and writes the `<!-- rfd-plan: pending -->` marker when that fails.
  The recipe's `jp query` subprocess goes.
- `list` needs the board: RFD metadata, `.priority.json`, and which tracking
  tickets are In Progress.
  Today `assembleBoard` in `rfd-shared.mjs` builds it.
  `jp rfd` ports the reading side of that function and none of its validation.

`draft --from-ticket ID` seeds the draft from the ticket's title and
description, then records `Promoted to` on the ticket and closes it.
It is safe to repeat: a ticket that already names an existing RFD reports it,
and a draft seeded by an interrupted run is found by its `Promoted from ticket`
line and linked rather than seeded twice.

`rank ID --before OTHER` and `--after OTHER` put ID next to OTHER on the
priority board, in OTHER's milestone group, so a backlog RFD ranked this way
becomes planned; `rank ID --backlog` moves it back.
It refuses an RFD at a terminal status, which the board does not place.
Milestones are created, renamed, and removed on the site.

`init` creates the RFD directory with the templates, an empty `.priority.json`,
and a `drafts/` directory.
It does not write an RFD 001: the process document is each project's own.

Tracking needs tickets.
In a project without a ticket directory, `promote` skips the tracking offer,
`list` shows no RFD in development, and `track`, `start`, `stop`, and `draft
--from-ticket` fail with a message saying so.
A board that defines labels has to define the tracking key, open to any value
(`values = []`), for a ticket to carry it: `track` and `start` refuse until it
does, printing the entry to add, and `promote` skips the offer and says why.
`show` lists the tickets naming an RFD, the tracking one first, and names the
labels a missing tracking ticket takes.

### Parity

Every verb that applies to both document kinds exists for both, and every
command that makes sense for the assistant exists as a tool.
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

Tracking an RFD is ticket work, so the assistant does it with the ticket tools:
`rfd_show` names the labels a missing tracking ticket takes, `ticket_create`
files it, and `ticket_edit` moves it to start or stop the work.
Tools of its own would cost three definitions on every turn for that.

One tool, `ticket_edit`, makes every change to a ticket: it takes any of
`title`, `description`, `status`, `add_labels`, and `remove_labels`, and changes
only what it is given.
Labels are added and removed rather than replaced, so a call can't drop one it
doesn't name, and a key a ticket carries one of changes in one call: the bare
key in `remove_labels`, the new label in `add_labels`.
Each tool a model is offered costs context on every turn, and providers cap how
many it can be offered; a command costs nothing until it runs, so the CLI keeps
its separate commands.

For the same reason, `ticket_show` returns a ticket's metadata unless the call
asks for its `description` or `comments`, and ends by naming what it left out.
There is no tool listing the label vocabulary: the label parameters of
`ticket_create`, `ticket_edit`, and `ticket_list` already offer every value.
`ticket_list` returns tickets in board order, so its first Todo line is the next
ticket to work on.
It pages a long listing, and its `query` finds the tickets whose title,
description, or comments mention a piece of text.

`import`, `refresh`, `delete`, `board`, and `rank` stay CLI-only.
`import` reaches GitHub over the network, `refresh` rewrites what the current
branch introduced, and deleting and prioritizing are the user's calls.
`board` would tell the assistant nothing `ticket_list` doesn't.

A tool never opens the host's composer or picker.
A missing argument is an error naming the argument.

### Tools

`jp ticket` and `jp rfd` serve their tools as plugin tools ([RFD 072][RFD
072-2]).
JP starts the plugin for each call and hands it the call in `init`, together
with the plugin's options as the query resolved them: nested `.jp.toml` files,
conversation configuration, and `--cfg` all reach it.
`ticket.rs` leaves `jp-tools`.

```toml
[conversation.tools.ticket_create]
source = "plugin.command.ticket.create"
style.parameters = "tool"
```

The plugin names its tools without a prefix (`create`, `show`, `list`), since
the source already names the plugin.
`style.parameters = "tool"` has the plugin describe a call before it runs, which
is how `ticket_create` previews the ticket it will write today.
A question a tool asks ends the call, and JP runs the plugin again with the
answer, so the plugin keeps no state between calls.
The tools keep their current behavior, including `ticket_create` asking for a
shorter title when the title would not fit the filename, and
`options.max_title_length`.
Every tool holds to the call's access policy, refusing to read or write a
document directory the policy doesn't grant, which the `jp-tools` handlers never
checked.

The plugin owns what each tool does; each project owns its tool configuration.
A project can expose `create` as `track`, choose its own `summary` text, and set
its own `options.max_body_words`.
The `source` name is the contract between the two: `[conversation.tools.track]`
with `source = "plugin.command.ticket.create"` reaches the plugin as `create`.

`tools init PATH` writes a starting set of definitions into the directory PATH
names, one file per tool.
It refuses to overwrite a file that exists.
PATH has no default, because where a project keeps its tool configuration is its
own layout; the workspace configuration has to load that directory for the tools
to appear.

`tools sync PATH` regenerates what the definitions mirror from configuration:
the label tokens offered by the `ticket_*` tools, and the categories offered by
`rfd_draft` and `rfd_list`.
It finds the tools by their source, not their key: a tool keyed `ticket_create`
and one keyed `track`, both with `source = "plugin.command.ticket.create"`, get
the tokens, each under its own key.
It writes only into a tool whose definition declares the parameter, and names
the ones it leaves out: values alone would make a parameter without a type,
which drops the tool from every conversation.
When the board bounds a key, it describes the bounds beside the labels of
`ticket_create` and `ticket_edit`, and marks `ticket_create`'s labels required
when a key needs one, so the assistant knows before its first call.
It rewrites only the file it generated, and rewrites it whole, so edits to the
definitions survive and a tool removed from the configuration drops out of it.
It replaces `just ticket-labels-sync`.

Both commands go once a plugin can describe the tools it serves (T-0vmm72y).

### Site

`jp vite init` creates a VitePress setup in the source directory of an empty or
existing repository: `package.json`, `.vitepress/config.mts`, the theme entry,
and a homepage.
It asks for the site's title and description through the host's composer, and
writes them into `config.mts` and the homepage, so a new site does not start out
as JP's.
Like the other `init` commands, it refuses to overwrite a file that exists.

The site files split into two groups:

- **Owned by `jp vite`.** Everything under `.vitepress/jp-vite/`: the loaders
  for both document kinds, components, styles, the dev-server middleware, a
  config fragment, and the settings snapshot.
- **Owned by the project.** `config.mts`, the theme entry, the homepage, and
  every page, the RFD and ticket pages included.
  `config.mts` imports the fragment and merges it; the theme entry registers the
  components from `jp-vite/theme`.
  `init` writes both once and never touches them again.

`sync` creates the pages for each workflow the project has when they are
missing: the index, priority, and drafts index pages in the RFD directory, and
the index and board pages in the ticket directory.
A page that exists is never changed, so a project writes its own around the
list.
Each page renders its list with one tag the theme entry registers, so it reads
the same wherever the page sits:

- `<RfdIndex />` and `<RfdDraftIndex />`, the published and the draft RFDs.
- `<RfdBoard />`, the priority board.
- `<TicketIndex />` and `<TicketBoard />`, the tickets and their board.

Both loaders ship together, because the RFD board reads tracking tickets.
Without a ticket directory the ticket loader reads no tickets, as it does today.

The owned files live in the crate, under `crates/plugins/command/vite/site/`,
and are built into the binary.
JP syncs from them like any other project, so a change to a component is made in
the crate, not in `docs/.vitepress/jp-vite/`.

The loaders also run without `jp`: `vitepress build` in CI reads the documents,
filters them by category, and validates their labels, with no configuration to
resolve.
`sync` therefore writes what they need into the owned `jp-vite/settings.json`:
both directories, the categories, the label vocabulary, the Done limit, the
repository, and the revision `jp-vite` was built from.
A link to a repository file points at the default branch (`/blob/HEAD/`), since
`repository` names no branch; JP's site links to `main` today.

`jp vite sync` rewrites the owned files from the binary, the snapshot among
them, and creates the pages a workflow is missing, including one added since the
last sync.
It lists the files it will write and asks once before writing any of them.
It requires the project to be in git, and refuses when an owned file is modified
or untracked, since git cannot restore an edit it never recorded.
`--force` overwrites them anyway.
On a clean tree, `git diff` shows what the sync changed and `git checkout`
restores the last committed version.
`--yes` skips the question for scripts; it does not imply `--force`.

`sync --check` writes nothing, and fails when any owned file differs from what
`sync` would write, or a page is missing.
A CI job with `jp` installed runs it to catch a configuration change committed
without its sync.

`jp vite serve` runs the dev server.
It runs the checks in [Version Skew](#version-skew), starts `vitepress dev` as
its child, and stops it before it exits.
While it runs, it asks the host for the configuration again every two seconds
(`read_config` with `reload`, protocol 10) and rewrites the snapshot when the
settings in it change, so a changed directory or vocabulary reaches the site
without a restart.
The loaders watch the snapshot, and the middleware reads it on every request.
The snapshot holds nothing written by hand, so this rewrite skips the refusal
`sync` makes for a modified owned file.

The middleware accepts writes only under `jp vite serve`, which marks the
child's environment; a dev server started another way is read-only.
Every write passes the snapshot's directories as `--cfg` overrides, so a
configuration changed between two polls cannot move a write away from the
directory the site displays.
The RFD priority board writes `.priority.json` itself, into the RFD directory
the snapshot names: it posts the whole layout, milestones included, which no `jp
rfd` command takes.

A drag on the ticket board is one command: `jp ticket status`, with `--before`
or `--after` naming the card it landed next to, so the column and the place in
it change together or not at all.
Only the moved card gets a rank, where the board writes every column's whole
order today.

The priority board writes no ticket.
It shows which RFDs are in development, and links each RFD with tickets to the
ticket board narrowed to them, `?any-label=tracking-rfd=116` and
`&any-label=relevant-rfd=116` with the configured keys.
The ticket board filters by labels and knows nothing about RFDs: `?label=` keeps
the tickets carrying every term, `?any-label=` those carrying any.
Work starts by dragging the tracking ticket to In Progress there.
A dev-server request runs `jp` without a terminal, and one still running after
15 seconds is stopped.

Styling and component changes therefore land in `jp vite`, and every project
receives them on its next sync.

### Version Skew

The synced loaders read what `jp ticket` and `jp rfd` write, so plugins
installed at different revisions can disagree about a format.
Installing them in one command at one revision avoids this (see
[Installation](#installation)).

When they still differ, the site says so rather than misreading.
Before it starts `vitepress dev`, `jp vite serve` runs two checks:

- It compares its own revision with the one `jp ticket` and `jp rfd` report, for
  each workflow whose directory exists.
  A plugin missing for an existing directory fails the check, naming the command
  that installs it; a tickets-only project never needs `jp rfd`.
- It compares every owned file but the snapshot with what `sync` would write, so
  loaders left behind by an upgraded `jp-vite` are caught.

On a failure, the site still starts: the RFD and ticket pages show the error and
the command that fixes it in place of their content, and the dev server refuses
writes.

### Validation

The site's validation stays in JavaScript for now: duplicate ids, the
relationship graph and its status gate, board consistency, summaries, unknown
labels, checked against the vocabulary in the snapshot, and link labels that
name no RFD, or name one by an id spelled another way.
The link check runs with the RFD loaders, which read the tickets anyway, so the
ticket loaders stay free of RFDs.
The loaders that run it are owned files, so `jp vite sync` distributes them
unchanged.

A new way of writing and reviewing RFDs is in development and brings its own
checker.
Porting validation to Rust waits until that has settled, and gets its own RFD.

### Installation

```sh
cargo install --git https://github.com/dcdpr/jp --rev <sha> jp-ticket jp-rfd jp-vite
```

A project installs the plugins it uses alongside `jp`, in one command so they
share a revision.
JP installs them from the workspace, as it installs `jp-ticket` today.
Pinning a revision makes an upgrade a deliberate change, reviewed alongside the
`jp vite sync` it triggers.

## Drawbacks

- **The port is large.** The twelve recipes and their helpers span roughly 1,500
  lines of shell whose outcomes have to be reproduced, edge cases included:
  `renumber` alone handles shared ids after a branch merge, draft-only tokens,
  and references outside `docs/`.
- **JP or nothing.** A project uses these through `jp` or not at all.
  Every dev-server write starts a `jp` process, and every tool call starts the
  plugin.
- **Three versions to keep in step.** A project that upgrades one plugin and not
  the others can hit a format the older one misreads.
  The dev server's check reports it, but only after the fact.
- **Settings in two places.** The site reads a snapshot of the configuration.
  `jp vite serve` keeps it current while it runs; a configuration committed
  without its sync stays stale until `sync --check` in CI catches it.
- **Two languages remain.** Until validation moves to Rust, a rule about RFDs
  can live in a plugin or in the synced JavaScript, and both ship from here.
- **Projects follow JP's repository.** A project pins a JP revision to get a
  tooling fix, and JP's pace of change on `main` becomes theirs.

## Alternatives

**One binary.** A single `jp-todo` absorbing the ticket crate and plugin, with
`rfd`, `ticket`, and `docs` subcommands.
It has one version, so nothing can skew.
But every project installs all of it, `jp ticket` becomes `jp todo ticket`, and
one binary owns three concerns that change for different reasons.
This flips if skew causes more trouble in practice than the split saves.

**Standalone binaries.** `jp-ticket`, `jp-rfd`, and `jp-vite` running without
`jp`, configured by their own `.jp-todo.toml`.
That is a second configuration file for all three plugins and the site to find
and agree on, and without a host there is no composer or picker to prompt with.
This flips when a project wants the workflow without JP.

**The vocabulary stays in `.labels.json`.** One file the static build reads
directly.
But it is a second home for configuration, and the snapshot carries the
vocabulary to the build anyway.

**Keep copying.** The status quo, and the drift described in the Motivation is
its result.

**A shared justfile module.** `just` imports local files only, so the module
would still be copied into each project, still be shell, and still be untested.

**A separate repository.** The plugins depend on `jp_plugin`, `jp_tool`,
`jp_label`, and `comfort`.
A separate repository needs all four published and versioned, for one consumer
outside JP.
This can change once a second consumer needs these plugins outside JP's
repository.

**An npm package for the site.** It would ship the components and styles the way
a VitePress project expects, but versioned apart from the plugins, outside the
one `cargo install` that keeps all three on one revision.

**A three-way merge on sync.** Recording the hash of each written file lets `jp
vite sync` tell a local edit from an upstream change.
Refusing to overwrite modified files, and git for everything committed, cover
the same cases without a manifest to keep, so v1 does not add one.

## Non-Goals

- **Porting validation.** See [Validation](#validation).
- **The remaining recipes.** `rfd-grep`, `rfd-board-prune`, `ticket-grep`, and
  the rest stay in JP's justfile until a later change moves them.
- **A standalone mode.** See [Alternatives](#alternatives).
- **Editing milestones from the terminal.** See [RFDs](#rfds).
- **Migrating tickets.** No project has tickets left to migrate.
- **Publishing to crates.io.**

## Risks and Open Questions

- **Builds without `jp`.** A CI build of the site that has no `jp` runs neither
  the revision check nor `sync --check`.
  There, a skewed or stale loader fails the build or misreads a document with no
  hint at the cause.
- **Moving a directory.** Changing `dir` moves nothing: the documents, and every
  link to them, are the project's to move.
- **Polling.** `jp vite serve` reloads the configuration on a timer, because the
  host cannot yet say when it changed (T-0vmm72x).
  A configuration file saved halfway fails to load; the host keeps the last one
  that loaded, and so does the snapshot.
- **VitePress versions.** Synced files assume a VitePress version.
  `jp vite init` writes `package.json`, but `sync` leaves it alone, so a project
  can fall behind what the synced files need.

## Implementation Plan

### Phase 1: `jp ticket` configuration and commands

Read `dir`, `assistant`, and `board.done_limit` from the plugin's options.
Add `rename`, `status`, `block`, `board`, `rank`, and `init`; drop `migrate`;
make `import` require `--repo`, and pass it from the `ticket-import` recipe.
Serve the `ticket_*` tools as plugin tools, with `ticket_create`'s title
question and preview, add `tools init` and `tools sync`, and remove `ticket.rs`
from `jp-tools` and `just ticket-labels-sync`.
Point JP's ticket tool configuration at `plugin.command.ticket`.

The vocabulary stays in `.labels.json` until Phase 5, which gives the site a way
to read it from configuration.
JP keeps the default ticket directory, so the site's fixed paths hold until then
too.

Mergeable on its own.

### Phase 2: Reading RFDs

Create `jp-rfd`, reading `dir` and `categories` from its options.
Port the board's reading side from `rfd-shared.mjs`.
Add `list`, `show`, `tools init`, and `tools sync`, and the tools for `list` and
`show`.

Depends on Phase 1.

### Phase 3: RFD lifecycle

Write the shared id resolver, metadata-header editor, and relationship writer.
Port `draft`, `rename`, `renumber`, `extend`, `require`, `supersede`, `abandon`,
and `promote`, and their tools, and add `rank` and `init`.
Port `just ticket-promote` into `draft --from-ticket`.
JP's recipes become one-line calls into `jp rfd`, so both paths stay usable
while the port is checked against real use; `just rfd-extend NNN MMM` calls `jp
rfd extend MMM NNN`.
The `rfd_extend` tool's parameters are renamed to match the new order.

Depends on Phase 2.

### Phase 4: Tracking

Port `track`, `start`, and `stop`, and their tools, with `track` reading the
plan through the host's `query`.
Point the dev server's `/__rfd-development` endpoint at `jp rfd`.

Depends on Phase 3.

### Phase 5: Site

Create `jp-vite`.
Move JP's site files into `crates/plugins/command/vite/site/`, embed them, and
add `init`, `sync`, `sync --check`, the settings snapshot, and `serve` with its
checks, and point JP's docs dev recipe at `jp vite serve`.
Point the ticket board writer at `jp ticket status` and `jp ticket rank`, and
the priority writer at the snapshot's RFD directory.
The `/__rfd-development` endpoint stays on the recipes until Phase 4 moves it.
Move the label vocabulary from `.labels.json` into
`plugins.command.ticket.options.labels`.
JP checks its synced copy with a `jp-vite` test, `jp_site_is_synced`, which CI
runs whenever the site files, the synced copy, or `.jp/config.toml` change;
`sync --check` in the site workflow would build `jp` on every docs change.
JP's own site becomes the first project synced from the binary.

Depends on Phase 2, so the revision check finds a `jp rfd` for JP's RFD
directory.
Independent of Phases 3 and 4.

### Phase 6: Retire the recipes

Remove the ported recipes, `ticket-promote` among them, and point JP's RFD tool
configuration at `plugin.command.rfd`.
JP keeps `just rfd-promote` as a wrapper around `jp rfd promote`, because
publishing a draft also archives the review and triage conversations JP's own
workflow titles after the draft id, and its `rfd_promote` tool keeps calling the
recipe for the same reason.
The remaining `ticket-*` recipes keep calling `jp ticket`, which still works.
Update the references in [RFD 001] and [RFD 100] to `just rfd-*`, `just
ticket-promote`, `just ticket-labels-sync`, and `.labels.json`.

Depends on Phases 1 to 5.

## References

- [RFD 001: The JP RFD Process][RFD 001], the lifecycle `jp rfd` implements.
- [RFD 072: Command Plugin System][RFD 072], the protocol the plugins run under,
  the `init` message that carries their configuration, and the tool calls they
  serve.
- [RFD 077: Plugin Configuration and Trust Policy][RFD 077], which describes the
  `options` and `run` settings the plugins are configured through.
- [RFD 100: In-Repo Ticket Tracking][RFD 100], the ticket format and labels.
- [RFD 102: Collision-Resistant Ticket Identifiers][RFD 102].

[RFD 001]: 001-jp-rfd-process.md
[RFD 072]: 072-command-plugin-system.md
[RFD 072-2]: 072-command-plugin-system.md#tool-calls
[RFD 077]: 077-plugin-configuration-and-trust-policy.md
[RFD 100]: 100-in-repo-ticket-tracking.md
[RFD 102]: 102-collision-resistant-ticket-identifiers.md
