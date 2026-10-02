# Command Plugin System

- **Status**: Done
- **Kind**: Feature
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-09-28
- **Implements**: 072
- **Label**: type=tracking

Tracking ticket for [RFD 072].

## Implementation plan

- **Build protocol core and dispatcher**: Defines the JSON-lines protocol
  message types in a new `jp_plugin` crate, implements the host-side message
  loop (spawn child, send `init`, relay requests to `Workspace` methods, capture
  stderr to tracing), and implements unknown-subcommand dispatch via `$PATH`
  search for `jp-<name>`.
  Validated with a minimal shell script plugin.
- **Extract web server as external plugin**: Moves the web server into a
  standalone `jp-serve` binary crate with its own plugin-side protocol client,
  removes `jp serve` as a built-in command in favor of plugin dispatch, and
  drops the `jp_web` dependency from `jp_cli`.
- **Add plugin registry and auto-install**: Defines the registry JSON format and
  implements fetching, caching, and checksum-validated binary downloads, plus a
  silent/prompted install flow and new `jp plugin list`/`install`/`update`
  subcommands.
- **Ship write operations**: Delivers task-level write operations
  (`ArchiveConversation`, `SetTitle`, `WriteDraft`, `Query` with a `Created`
  response) that each perform their own locking on the host, so a plugin never
  holds a lock or appends events directly.
- **Implement command routing and plugin dependencies**: Wires the `command`
  field from `Describe` and registry keys into actual routing (replacing
  binary-name-only derivation), applies the longest-path-wins and
  conflict-diagnostic rules, caches `describe` responses, updates `jp plugin
  install` to resolve `requires` dependencies, and implements `command_group`
  help aggregation with merged sub-plugin listings.
- **Add admission, containment, and output routing**: Enforces admission checks
  before every plugin spawn (including `describe`), terminates the plugin's
  process group/job object after the shutdown grace period, and routes `print`
  messages through the host printer by channel and format instead of writing raw
  text to stdout.

## Comments

-----

- **From**: jp
- **Date**: 2026-09-28T21:04:06Z

Phases 5 and 6 are implemented; every phase of RFD 072 is now checked.
Along the way, RFD 077 Phase 4 (`unattended` → `allow`) and Phase 5 (explicit
installs) landed too.

Before `just rfd-promote 072` to Implemented: RFD 077 is still in Discussion,
and 072 is `Required by` it, not the reverse, so nothing gates the promotion.
RFD 077 Phase 3 still has one open bullet (show the plugin kind in `jp plugin
list`).

-----

- **From**: jp
- **Date**: 2026-09-30T08:35:40Z
- **Re**: #1

RFD 077 Phase 3's last bullet is done too: `jp plugin list` shows each registry
entry's type (`command` or `command group`), and the registry reads entries one
at a time, so an entry of an unknown type is skipped instead of failing the
whole registry.

Running an installed plugin no longer reaches the network: official plugins
update through `jp plugin update`, and the manifest cache is gone.
RFD 072's registry section describes when JP fetches.

[RFD 072]: ../rfd/072-command-plugin-system.md
