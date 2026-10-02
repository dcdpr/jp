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
the assistant's local tools.
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

| Plugin      | Crate                           | Owns                                                  |
| ----------- | ------------------------------- | ----------------------------------------------------- |
| `jp ticket` | `crates/plugins/command/ticket` | Tickets: commands, the board, and the `ticket_*` tools |
| `jp rfd`    | `crates/plugins/command/rfd`    | RFDs: lifecycle, board, and the `rfd_*` tools          |
| `jp vite`   | `crates/plugins/command/vite`   | The site: `init`, `sync`, and its files                |

Each is a `jp-<name>` binary, built as a library with a thin `main` the way
`jp-ticket` is today, and each runs only as a plugin: `jp` resolves the
workspace's configuration and hands it over in the `init` message ([RFD 072]).
They use only the part of [RFD 072] that has shipped: the `init` message with
the resolved configuration and the plugin's options, the `run` policy, and the
host's composer and picker.
`jp-ticket` exists; `jp-rfd` and `jp-vite` are new.
The ticket format stays in `crates/internal/ticket`, and `jp_label` stays a
separate crate, because conversations use it too.

The dependencies run one way:

- `jp ticket` knows nothing about RFDs.
  An RFD appears in a ticket only as an opaque id, in `Implements` and `Promoted
  to`.
- `jp rfd` links the `ticket` library for the two things that cross over:
  tracking tickets, and seeding a draft from a ticket.
  Both plugins write through that library, so the ticket format still has one
  implementation.
- `jp vite` links neither.
  Its dev server makes every write through `jp ticket` and `jp rfd`, including
  the board moves it writes itself in JavaScript today, so running the site no
  longer needs `just`.

| Entry point                                         | Caller                             |
| --------------------------------------------------- | ---------------------------------- |
| `jp ticket …`, `jp rfd …`, `jp vite …`              | A person at the terminal           |
| `jp ticket tool CONTEXT TOOL`, `jp rfd tool …`      | The assistant, through local tools |
| `jp ticket …`, `jp rfd …`                           | The site's dev server, to write    |

### Configuration

Each plugin's settings live under its `plugins.command.<name>.options`:

```toml
[plugins.command.ticket]
run = "unattended"

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
run = "unattended"

[plugins.command.rfd.options]
dir = "docs/rfd"
categories = ["design", "decision", "guide", "process"]

[plugins.command.vite]
run = "unattended"

[plugins.command.vite.options]
# The VitePress source directory.
dir = "docs"
# Where links to repository files point.
repository = "https://github.com/dcdpr/jp"
```

Every option is optional, and the values shown are the defaults except three:
`assistant` defaults to `assistant`, `labels` to no vocabulary, and
`repository` to git's `origin` remote.

Each setting has one owner.
A plugin that needs another's setting reads it from the full configuration in
`init`: `jp rfd` takes the ticket directory and vocabulary from
`plugins.command.ticket.options`, and `jp vite` reads all three plugins'
options.

The label vocabulary moves out of `.labels.json` into `options.labels`, in the
same shape: a description per key, its values, and the values retired from it.
Whether contributors may redefine it in their own config layers is the
workspace's call, as for any other setting.

`run = "unattended"` is needed wherever a plugin runs without a terminal, as it
does for a tool call or a dev-server write.
A plugin found on `$PATH` otherwise asks before running, and with no terminal
to ask on, refuses.
`tools init` writes the entry (see [Tools](#tools)).

Both document directories must sit inside the VitePress source directory,
because VitePress serves pages from nowhere else, and `jp vite` refuses a
configuration that breaks this.
A directory's route follows its path: with `dir = "docs"`, `docs/ticket` is
served at `/ticket/`.

Layered settings need `options` to merge key by key, so a contributor who
overrides `assistant` keeps the workspace's `dir`.
Today a later layer replaces the whole `options` table (T-0vkq4dh), and this
RFD depends on that fix.

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

The commands are the twelve lifecycle recipes this RFD ports, plus `show` and
`init`:

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

Three delegations carry over:

- `promote` reflows the promoted file with `comfort` and offers to file a
  tracking ticket at Accepted.
  `comfort` becomes a library call rather than a subprocess.
- `track` reads the Implementation Plan through `jp query` with a schema and
  writes the `<!-- rfd-plan: pending -->` marker when that fails.
  It stays a subprocess: an LLM call is `jp`'s job, not this crate's.
- `list` needs the board: RFD metadata, `.priority.json`, and which tracking
  tickets are In Progress.
  Today `assembleBoard` in `rfd-shared.mjs` builds it.
  `jp rfd` ports the reading side of that function and none of its validation.

`draft --from-ticket ID` seeds the draft from the ticket's title and
description, then records `Promoted to` on the ticket and closes it.
It is safe to repeat: a ticket that already names an existing RFD reports it,
and a draft seeded by an interrupted run is found by its `Promoted from ticket`
line and linked rather than seeded twice.

`init` creates the RFD directory with the templates, an empty `.priority.json`,
and a `drafts/` directory.
It does not write an RFD 001: the process document is each project's own.

Tracking needs tickets.
In a project without a ticket directory, `promote` skips the tracking offer,
`list` shows no RFD in development, and `track`, `start`, `stop`, and `draft
--from-ticket` fail with a message saying so.
Tracking tickets carry the label `type=tracking`, which the ticket vocabulary has
to define; `track` refuses when it doesn't, naming the entry to add.

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
| Rank             | the site's priority board                                                   | **`rank`** / not a tool                                     |
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

RFDs get no `rank` in v1.
`.priority.json` also holds milestones and the cut between planned and backlog,
which a single `--before` cannot express, so ranking RFDs stays on the site.

A tool call never prompts.
A missing argument is an error naming the argument.

### Tools

`jp ticket tool` and `jp rfd tool` implement the `local.<name>` protocol that
`jp-tools` implements today, each for its own prefix.
`ticket.rs` leaves `jp-tools`.
A tool runs through `jp` so it receives the plugin's configuration, which a
local tool's own context does not carry:

```toml
[conversation.tools.ticket_create]
source = "local"
command = "jp ticket tool {{context}} {{tool}}"
```

The plugin owns what each tool does; each project owns its tool configuration.
A project can expose `ticket_create` as `track`, choose its own `summary` text,
and set its own `options.max_body_words`.
The `source` name is the contract between the two: `jp` passes it as the tool
name in the context, so `[conversation.tools.track]` with `source =
"local.ticket_create"` reaches the dispatcher as `ticket_create`.

A tool call resolves configuration the way `jp` does at the workspace root,
because the tool's process starts there and runs a `jp` of its own.
The query's `--cfg` arguments, its conversation's configuration, and `.jp.toml`
files below the root do not reach it.

`tools init PATH` writes a starting set of definitions into the directory PATH
names: one file per tool, plus the plugin's `run = "unattended"` entry.
It refuses to overwrite a file that exists.
PATH has no default, because where a project keeps its tool configuration is its
own layout; the workspace configuration has to load that directory for the tools
to appear.

`tools sync PATH` regenerates what the definitions mirror from configuration:
the label tokens offered by the `ticket_*` tools, and the categories offered by
`rfd_draft`.
It finds the tools by their source name, not their advertised one: a tool keyed
`ticket_create` with `source = "local"`, and one keyed `track` with `source =
"local.ticket_create"`, both get the tokens, each under its own key.
It rewrites only the file it generated, and rewrites it whole, so edits to the
definitions survive and a tool removed from the configuration drops out of it.
It replaces `just ticket-labels-sync`.

A future self-describing local tools RFD would let `jp` ask each plugin for its
definitions, and make both commands unnecessary.

### Site

`jp vite init` creates a VitePress setup in the source directory of an empty or
existing repository: `package.json`, `.vitepress/config.mts`, the theme entry,
and a homepage.
It asks for the site's title and description through the host's composer, and
writes them into `config.mts` and the homepage, so a new site does not start
out as JP's.
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
JP syncs from them like any other project, so a change to a component is made
in the crate, not in `docs/.vitepress/jp-vite/`.

The loaders also run without `jp`: `vitepress build` in CI reads the documents,
filters them by category, and validates their labels, with no configuration to
resolve.
`sync` therefore writes what they need into the owned `jp-vite/settings.json`:
both directories, the categories, the label vocabulary, the Done limit, the
repository, and the revision `jp-vite` was built from.

`jp vite sync` rewrites the owned files from the binary, the snapshot among
them, including the pages for a workflow added since the last sync.
It lists the files it will write and asks once before writing any of them.
It requires the project to be in git, and refuses when an owned file is
modified or untracked, since git cannot restore an edit it never recorded.
`--force` overwrites them anyway.
On a clean tree, `git diff` shows what the sync changed and `git checkout`
restores the last committed version.
`--yes` skips the question for scripts; it does not imply `--force`.

`sync --check` writes nothing, and fails when any owned file differs from what
`sync` would write.
A CI job with `jp` installed runs it to catch a configuration change committed
without its sync.

Styling and component changes therefore land in `jp vite`, and every project
receives them on its next sync.

### Version Skew

The synced loaders read what `jp ticket` and `jp rfd` write, so plugins
installed at different revisions can disagree about a format.
Installing them in one command at one revision avoids this (see
[Installation](#installation)).

When they still differ, the site says so rather than misreading.
At startup the dev server runs two checks:

- It compares the revision in the snapshot with the one `jp ticket` and `jp
  rfd` report, for each workflow whose directory exists.
  A plugin missing for an existing directory fails the check, naming the
  command that installs it; a tickets-only project never needs `jp rfd`.
- It runs `jp vite sync --check`, so a snapshot left behind by a configuration
  change is caught before the site reads one directory and writes another.

On a failure, the RFD and ticket pages show the error and the command that
fixes it in place of their content, and the dev server refuses writes.

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
- **JP or nothing.** A project uses these through `jp` or not at all, and every
  tool call and dev-server write starts a `jp` process.
- **Three versions to keep in step.** A project that upgrades one plugin and not
  the others can hit a format the older one misreads.
  The dev server's check reports it, but only after the fact.
- **Settings in two places.** The site reads a snapshot of the configuration,
  which is stale from a configuration change until the next sync.
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

**A three-way merge on sync.** Recording the hash of each written file lets
`jp vite sync` tell a local edit from an upstream change.
Refusing to overwrite modified files, and git for everything committed, cover
the same cases without a manifest to keep, so v1 does not add one.

## Non-Goals

- **Porting validation.** See [Validation](#validation).
- **The remaining recipes.** `rfd-grep`, `rfd-board-prune`, `ticket-grep`, and
  the rest stay in JP's justfile until a later change moves them.
- **A standalone mode.** See [Alternatives](#alternatives).
- **Ranking RFDs from the terminal.** See [Parity](#parity).
- **Migrating tickets.** No project has tickets left to migrate.
- **Publishing to crates.io.**

## Risks and Open Questions

- **Builds without `jp`.** A CI build of the site that has no `jp` runs neither
  the revision check nor `sync --check`.
  There, a skewed or stale loader fails the build or misreads a document with no
  hint at the cause.
- **Moving a directory.** Changing `dir` moves nothing: the documents, and every
  link to them, are the project's to move.
- **Configuration below the root.** A tool call reads the configuration at the
  workspace root (see [Tools](#tools)).
  In a monorepo where `packages/foo/.jp.toml` sets a ticket `dir`, a query run
  from `packages/foo` sees that directory, and the tools it calls write to the
  root's.
  The tool result names the path it wrote, so this is visible, but the file
  stays where it landed.
  T-0vm44ks tracks the host passing a plugin's effective options to the tools
  it serves.
- **VitePress versions.** Synced files assume a VitePress version.
  `jp vite init` writes `package.json`, but `sync` leaves it alone, so a project
  can fall behind what the synced files need.

## Implementation Plan

### Phase 1: `jp ticket` configuration and commands

Read `dir`, `assistant`, and `board.done_limit` from the plugin's options.
Add `rename`, `status`, `block`, `board`, `rank`, and `init`; drop `migrate`;
make `import` require `--repo`, and pass it from the `ticket-import` recipe.
Add `tool`, `tools init`, and `tools sync`, and remove `ticket.rs` from
`jp-tools` and `just ticket-labels-sync`.
Point JP's ticket tool configuration at `jp ticket tool`.

The vocabulary stays in `.labels.json` until Phase 5, which gives the site a
way to read it from configuration.
JP keeps the default ticket directory, so the site's fixed paths hold until then
too.

Depends on T-0vkq4dh.
Mergeable on its own.

### Phase 2: Reading RFDs

Create `jp-rfd`, reading `dir` and `categories` from its options.
Port the board's reading side from `rfd-shared.mjs`.
Add `list`, `show`, `tools init`, and `tools sync`, and the tools for `list`
and `show`.

Depends on Phase 1.

### Phase 3: RFD lifecycle

Write the shared id resolver, metadata-header editor, and relationship writer.
Port `draft`, `rename`, `renumber`, `extend`, `require`, `supersede`, `abandon`,
and `promote`, and their tools, and add `init`.
Port `just ticket-promote` into `draft --from-ticket`.
JP's recipes become one-line calls into `jp rfd`, so both paths stay usable
while the port is checked against real use; `just rfd-extend NNN MMM` calls
`jp rfd extend MMM NNN`.
The `rfd_extend` tool's parameters are renamed to match the new order.

Depends on Phase 2.

### Phase 4: Tracking

Port `track`, `start`, and `stop`, and their tools.
Point the dev server's `/__rfd-development` endpoint at `jp rfd`.

Depends on Phase 3.

### Phase 5: Site

Create `jp-vite`.
Move JP's site files into `crates/plugins/command/vite/site/`, embed them, and
add `init`, `sync`, `sync --check`, the settings snapshot, and the startup
checks.
Point the board writer at `jp ticket status` and `jp ticket rank`.
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
configuration at `jp rfd tool`.
The remaining `ticket-*` recipes keep calling `jp ticket`, which still works.
Update the references in [RFD 001] and [RFD 100] to `just rfd-*`, `just
ticket-promote`, `just ticket-labels-sync`, and `.labels.json`.

Depends on Phases 1 to 5.

## References

- [RFD 001: The JP RFD Process][RFD 001], the lifecycle `jp rfd` implements.
- [RFD 072: Command Plugin System][RFD 072], the protocol the plugins run under
  and the `init` message that carries their configuration.
- [RFD 077: Plugin Configuration and Trust Policy][RFD 077], which describes
  the `options` and `run` settings the plugins are configured through.
- [RFD 100: In-Repo Ticket Tracking][RFD 100], the ticket format and labels.
- [RFD 102: Collision-Resistant Ticket Identifiers][RFD 102].

[RFD 001]: 001-jp-rfd-process.md
[RFD 072]: 072-command-plugin-system.md
[RFD 077]: 077-plugin-configuration-and-trust-policy.md
[RFD 100]: 100-in-repo-ticket-tracking.md
[RFD 102]: 102-collision-resistant-ticket-identifiers.md
