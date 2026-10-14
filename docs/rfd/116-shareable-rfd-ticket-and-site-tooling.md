# RFD 116: Shareable RFD, Ticket, and Site Tooling

- **Status**: Discussion
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
Everything this RFD needs from `jp` is on `main`.

The dependencies run one way:

- `jp ticket` knows nothing about RFDs.
  An RFD appears in a ticket only as an opaque id, in `Implements` and `Promoted
  to`.
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

[plugins.command.vite]
run = "allow"

[plugins.command.vite.options]
# The VitePress source directory.
dir = "docs"
# Where links to repository files point.
repository = "https://github.com/dcdpr/jp"
```

Every option is optional, and the values shown are the defaults except three:
`assistant` defaults to `assistant`, `labels` to no vocabulary, and `repository`
to git's `origin` remote.

Each setting has one owner.
A plugin that needs another's setting reads it from the full configuration in
`init`: `jp rfd` takes the ticket directory and vocabulary from
`plugins.command.ticket.options`, and `jp vite` reads all three plugins'
options.

The label vocabulary moves out of `.labels.json` into `options.labels`, in the
same shape: a description per key, its values, and the values retired from it.
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

- `rename` replaces `edit --title`.
- `status ID STATUS` moves a ticket to any column; `close` stays as the
  shorthand for Done.
- `block ID BLOCKER` writes the `Blocked by` field, and `--clear` removes it.
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
`board` reads it, and `rank` and the site's board write it.
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
jp rfd show ID
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
- The tracking label comes from configuration.
- `extend` takes its arguments in the same order as `require`, source first.
  `jp rfd extend 116 100` records that 116 extends 100, and `jp rfd require 116
  072` that 116 requires 072.
  `just rfd-extend NNN MMM` reads the other way round, meaning MMM extends NNN.
  A relationship written backwards still parses and silently changes the
  promotion gates, so the two commands share one order.
- `promote` to Implemented removes the `Requires` entries that name the
  promoted RFD from every dependent, drafts included.
  The recipe finds dependents through the promoted RFD's `Required by` line,
  which never lists a draft, so a draft's `Requires` survives and fails the docs
  build.
  The port finds dependents by reading every RFD's `Requires`.

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
Tracking tickets carry the label `type=tracking`, which the ticket vocabulary
has to define; `track` refuses when it doesn't, naming the entry to add.

### Parity

Every verb that applies to both document kinds exists for both, and every
command that makes sense for the assistant exists as a tool.
Bold entries are new.

| Verb             | `jp rfd` CLI / tool                                                         | `jp ticket` CLI / tool                                      |
| ---------------- | --------------------------------------------------------------------------- | ----------------------------------------------------------- |
| Create           | `draft` / `rfd_draft`                                                       | `add` / `ticket_create`                                     |
| Show             | **`show`** / **`rfd_show`**                                                 | `show` / `ticket_show`                                      |
| List             | `list` / **`rfd_list`**                                                     | `list` / `ticket_list`                                      |
| Board            | `list`                                                                      | **`board`** / not a tool                                    |
| Rank             | **`rank`** / not a tool                                                     | **`rank`** / not a tool                                     |
| Rename           | `rename` / `rfd_rename`                                                     | **`rename`** / `ticket_rename`                              |
| Edit body        | not applicable                                                              | `edit` / **`ticket_edit`**                                  |
| Change status    | `promote` / `rfd_promote`                                                   | **`status`**, `close` / **`ticket_status`**, `ticket_close` |
| Relate           | `extend`, `require`, `supersede` / same                                     | **`block`** / **`ticket_block`**                            |
| Retire           | `abandon` / `rfd_abandon`                                                   | `delete` / not a tool                                       |
| Renumber         | `renumber` / `rfd_renumber`                                                 | not applicable                                              |
| Track            | `track`, `start`, `stop` / **`rfd_track`**, **`rfd_start`**, **`rfd_stop`** | not applicable                                              |
| Discuss          | not applicable                                                              | `comment` / `ticket_comment`                                |
| Label            | not applicable                                                              | `label`, `labels` / `ticket_label`, **`ticket_labels`**     |
| Ticket to RFD    | **`draft --from-ticket`** / `rfd_draft` with **`ticket`**                   | `promote --to` / not a tool                                 |
| Tool definitions | **`tools init`**, **`tools sync`**                                          | **`tools init`**, **`tools sync`**                          |

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
`rfd_draft`.
It finds the tools by their source, not their key: a tool keyed `ticket_create`
and one keyed `track`, both with `source = "plugin.command.ticket.create"`, get
the tokens, each under its own key.
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
  Also the pages for each workflow the project has: the index, priority, and
  drafts index pages in the RFD directory, and the index and board pages in the
  ticket directory, each when that directory exists.
- **Owned by the project.** `config.mts`, the theme entry, the homepage, and
  every other page.
  `config.mts` imports the fragment and merges it; the theme entry registers the
  components from `jp-vite/theme`.
  `init` writes both once and never touches them again.

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

`jp vite sync` rewrites the owned files from the binary, the snapshot among
them, including the pages for a workflow added since the last sync.
It lists the files it will write and asks once before writing any of them.
It requires the project to be in git, and refuses when an owned file is modified
or untracked, since git cannot restore an edit it never recorded.
`--force` overwrites them anyway.
On a clean tree, `git diff` shows what the sync changed and `git checkout`
restores the last committed version.
`--yes` skips the question for scripts; it does not imply `--force`.

`sync --check` writes nothing, and fails when any owned file differs from what
`sync` would write.
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
relationship graph and its status gate, board consistency, summaries, and
unknown labels, the last checked against the vocabulary in the snapshot.
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
`plugins.command.ticket.options.labels`, and run `sync --check` in JP's site
workflow.
JP's own site becomes the first project synced from the binary.

Depends on Phase 2, so the revision check finds a `jp rfd` for JP's RFD
directory.
Independent of Phases 3 and 4.

### Phase 6: Retire the recipes

Remove the ported recipes, `ticket-promote` among them, and point JP's RFD tool
configuration at `plugin.command.rfd`.
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
