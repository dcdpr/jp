# RFD 077: Plugin Configuration and Trust Policy

- **Status**: Implemented
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-04-07
- **Summary**: Adds a `[plugins]` configuration section to `AppConfig` for trust
  policies, binary checksum pinning, and per-plugin options.

## Summary

This RFD introduces a `[plugins]` section in `AppConfig` that configures plugin
execution trust policies, binary checksum pinning, and per-plugin options across
JP's layered configuration chain.
Under the `ask` policy, a user-local approval store records approved binary
checksums so each binary prompts only once.
Official plugins install automatically on first use, while third-party plugins
require an explicit `jp plugin install`.

## Motivation

[RFD 072] defines the command plugin system: standalone binaries that
communicate with JP over a JSON-lines protocol, backed by a plugin registry and
auto-install flow (Phase 3).
It leaves four gaps in how users configure and trust plugins:

- **Trust decisions are ad-hoc.** Tracking plugin approvals only in a standalone
  JSON file bypasses config inheritance.
  A user who trusts a plugin globally cannot deny it in a specific workspace, or
  vice versa.

- **No checksum pinning.** The registry provides checksums to verify downloads,
  but users cannot pin a specific binary digest and refuse to run a modified
  binary.
  If a registry-hosted binary is compromised and republished with a new
  checksum, unpinned installations are silently exposed.

- **No per-plugin options.** Plugins like `jp-serve` need plugin-specific
  settings (bind address, port).
  Without a config path, users must pass these as CLI arguments on every
  invocation, or the plugin must manage its own config file.

- **Plugin types aren't distinguished.** The registry assumes all plugins are
  command plugins.
  Supporting wasm plugins ([RFD 016]) requires distinguishing plugin kinds in
  both the registry and the configuration.

## Design

### Plugin Kind Taxonomy

Each registry plugin entry carries a `type` field:

```json
{
  "version": 1,
  "plugins": {
    "serve": {
      "type": "command",
      "description": "Read-only web UI for conversations",
      "official": true,
      "binaries": { ... }
    }
  }
}
```

`type` defaults to `"command"` when omitted, keeping existing registry entries
valid.
The dispatch pipeline filters on `type` and ignores unrecognized values, so
future plugin types (such as `wasm`) can be added to the registry without
breaking older JP versions.

`jp_plugin::registry` represents this field with `PluginKind`:

```rust
pub enum PluginKind {
    Command,  // standalone binary, RFD 072 protocol
    // Wasm,  // future: RFD 016
}
```

### Configuration Schema

The `[plugins]` section in `AppConfig` holds one section per kind of plugin.
`[plugins.command]` configures command plugins: defaults for all of them under
`'*'`, and per-plugin policies by name:

```toml
# Defaults for every command plugin.
[plugins.command.'*']
run = "ask"                       # default: "ask"
# Grace period (seconds) before SIGKILL after sending Shutdown.
shutdown_timeout_secs = 5         # default: 5

# Per-plugin configuration, keyed by plugin name.
[plugins.command.serve-web]
run = "allow"                     # execution policy
```

The defaults mirror `[conversation.tools.'*']`: a plugin that sets the same key
under its own name uses its own value.
A setting lives under the kind of plugin it applies to, so a kind without a
process to stop does not carry a shutdown timeout.

`run` and `shutdown_timeout_secs` have a default.
`checksum` and `options` do not: a checksum is the digest of one binary, so a
shared one would refuse every other plugin, and options are the plugin's own.

#### `plugins.command.'*'.run`

The execution policy of every command plugin that does not set its own `run`.
Defaults to `ask`; see [Per-Plugin Configuration](#per-plugin-configuration) for
the values.

Setting it to `deny` runs only the plugins allowed by name:

```toml
[plugins.command.'*']
run = "deny"

[plugins.command.serve-web]
run = "allow"
```

Setting it to `allow` trusts every `jp-*` binary on `$PATH` by name, without a
prompt or an approval.

#### `plugins.command.'*'.shutdown_timeout_secs`

Grace period in seconds between sending `Shutdown` over the protocol and
forcibly terminating (`SIGKILL`) the plugin and its child processes.
Applies to every command plugin that does not set its own
`shutdown_timeout_secs`.

#### Per-Plugin Configuration

Each entry under `plugins.command.<name>` configures a specific command plugin:

```toml
[plugins.command.serve-web]
run = "allow"

[plugins.command.serve-web.checksum]
algorithm = "sha256"
value = "e3b0c44298fc1c149afbf4c8996fb924..."

[plugins.command.serve-web.options]
bind = "127.0.0.1"
port = 3141
```

**`run`** (`RunPolicy`): Execution policy.

| Value   | Behavior                                                     |
| ------- | ------------------------------------------------------------ |
| `ask`   | Run verified official binaries and approved binaries; prompt |
|         | for any other.                                               |
| `allow` | Run without prompting.                                       |
| `deny`  | Never run. JP exits with an error if the plugin is invoked.  |

A plugin that sets no `run` uses `plugins.command.'*'.run`, which is `ask`
unless configured.
The `run` policy applies at every execution point, regardless of how the binary
was discovered.
Under `ask`, two kinds of binary run without prompting: an official binary whose
SHA-256 matches its registry entry, and a binary recorded in the [approval
store](#approval-store).
An unmodified official plugin therefore runs without prompting.
`allow` trusts a plugin by name regardless of its binary digest, which avoids
re-prompting after every build during local plugin development.

`allow` governs only the admission decision; an allowed plugin can still prompt
the user interactively during execution.
`unattended` is accepted as a deprecated alias for `allow` and logs a warning.

**`shutdown_timeout_secs`**: Overrides
`plugins.command.'*'.shutdown_timeout_secs` for this plugin.
Unset by default, in which case the default applies.
A plugin with work to finish before it exits, such as one waiting for a
delegated turn to stop, gets a longer grace period this way without lengthening
it for every other plugin:

```toml
[plugins.command.serve-chat]
shutdown_timeout_secs = 90
```

**`checksum`**: Pins the binary to a specific digest.
When set, JP computes the binary's checksum before execution and refuses to run
it on a mismatch.
This catches two scenarios:

1. A registry-hosted binary is republished with different contents (supply-chain
   compromise).
2. A `$PATH`-discovered binary is replaced or modified.

The setting reuses `ChecksumConfig` from MCP server configuration:

```rust
pub struct ChecksumConfig {
    pub algorithm: AlgorithmConfig,  // sha256 (default) | sha1
    pub value: String,               // hex-encoded digest
}
```

On a checksum mismatch, JP prints the expected and actual digests and the config
key to update, so the user can accept a legitimate update after reviewing it.

**`options`**: A table of opaque key-value pairs passed to the plugin in the
`options` field of the `init` message.
JP does not validate the table; the plugin parses and validates its own options.
Options merge key by key across config layers, recursing into nested tables so
later layers override only the keys they specify, matching tool options ([RFD
042]).

For example, the `serve-web` plugin reads `options.bind` and `options.port` from
`init` to configure its HTTP listener.

### Plugin Installation

Plugin installation is not controlled by configuration:

- **Official plugins install on first use.** Invoking an official command whose
  binary is missing downloads the binary, verifies it against the registry
  checksum, and runs it ([RFD 072]).
  `jp plugin update` uses the same flow when the registry publishes a new
  binary.
- **Third-party plugins require explicit installation.** `jp plugin install
  <name>` displays the binary source URL and prompts for confirmation before
  downloading.
  Confirming the install also records execution approval, so the first run does
  not prompt again.
  A binary placed directly on `$PATH` requires no install step and prompts for
  approval on first run.

Setting `JP_NO_PLUGIN_DOWNLOAD=1` disables automatic downloads of official
plugins, for environments where binaries are managed by a system package manager
or offline.

### Approval Store

Under the `ask` policy, JP persists user approvals in
`$XDG_DATA_HOME/jp/plugin-approvals.json`.
An entry is recorded when the user runs `jp plugin install <name>` for a
third-party plugin, answers `Y` at the execution prompt, or runs `jp plugin
approve <path>`:

```json
{
  "approved": {
    "webui": {
      "path": "/Users/jean/.cargo/bin/jp-webui",
      "sha256": "e3b0c44298fc1c149afbf4c8996fb924...",
      "approved_at": "2026-09-28T10:12:00Z"
    },
    "serve-web": {
      "path": "/Users/jean/.local/share/jp/plugins/command/jp-serve-web",
      "sha256": "6dd441c580ac95298db851b59d88bed95686241f38e63619e7a314e6bbb0c67a",
      "approved_at": "2026-09-28T09:40:00Z",
      "installed": true
    }
  }
}
```

Each entry is keyed by the plugin name, matching `plugins.command.<name>` ([RFD
072]), and records a single binary's path and SHA-256 digest.
`installed: true` marks binaries that JP downloaded itself (either an official
plugin on first use or a third-party plugin via `jp plugin install`).
Official binaries do not need approval to run, but recording their installed
digest lets JP distinguish a binary it manages and can safely update from one
modified locally, which `jp plugin update` leaves untouched.

An approval applies only to that exact file path and SHA-256 digest:

- **Modified binary prompts again.** If the file at the approved path has a
  different digest (for example, after an upgrade or rebuild), JP prompts again
  and notes that the binary changed since it was approved.
- **Moved binary prompts again.** If a plugin with the same name resolves to a
  different path, JP prompts and displays both the new path and the previously
  approved path.
  (If both files still exist on disk, routing rejects them as a conflict before
  reaching admission; see [RFD 072].)
- **Config policy takes precedence.** JP consults the approval store only when
  the effective `run` policy is `ask` and any pinned `checksum` has already
  matched.
  `deny` in any config layer blocks even an approved binary, and `allow` runs
  without checking the store.

The confirmation prompt displays the binary path and why confirmation is needed.
When a third-party binary claims an official command path, it overrides the
official plugin ([RFD 072]), and the prompt warns the user with **third-party**
highlighted in bold red:

```txt
  → jp serve web is claimed by the third-party plugin `webui`, which replaces
    the official one.
    /Users/jean/.cargo/bin/jp-webui
Run it? [y/Y/N]
```

When an official plugin binary does not match the registry digest (such as a
binary built by a system package manager), the prompt notes that the binary does
not match the official release.

When no interactive terminal is available, JP cannot prompt; it refuses to run
the unapproved binary and prints the `jp plugin approve <path>` command needed
to authorize it.

`jp plugin approve <path>` authorizes a binary non-interactively.
Running the command grants consent to spawn the binary once for discovery: the
host sends `describe`, prints the binary's path, description, and command
claims, verifies that the response matches the binary's embedded manifest, and
records the approval.
If the binary's embedded manifest cannot be read from disk (for example, when
packed with UPX), the approval entry also caches the `describe` response under
`manifest` so the router can read its command claims without spawning the binary
([RFD 072]).

`jp plugin revoke <name>` removes an approval entry.
`jp plugin uninstall <name>` removes both the binary from JP's plugin directory
and its approval entry; for binaries discovered on `$PATH`, `uninstall` refuses
to delete the file and directs the user to `jp plugin revoke` instead.
`jp plugin list` displays each plugin's approval state.

Approvals are user-global rather than workspace-scoped.
A plugin binary runs with the user's OS permissions regardless of the active
workspace, and multi-workspace plugins ([RFD 114]) access multiple workspaces in
a single process.
Blocking a plugin in a specific workspace is handled by setting `run = "deny"`
in that workspace's configuration.

Approvals live in a dedicated user-data file rather than in `AppConfig` because
configuration is persisted in conversation event streams and often committed to
version control alongside workspace files.
Machine-local binary paths and digests do not belong in shared repository state.
This follows the same trust-on-first-use pattern JP uses for out-of-workspace
tool mounts, except plugin approvals are user-global so plugins can be approved
and used outside a workspace.

### Config Inheritance

Plugin config participates in JP's standard config inheritance chain:

1. **Global config** (`$XDG_CONFIG_HOME/jp/config.toml`): user-wide defaults.
   Trust a plugin globally, set default options.
2. **Workspace config** (`.jp/config.toml`): per-project overrides.
   Deny a plugin in a sensitive workspace, or change its options.
3. **Local config** (`.jp.toml`): directory-scoped overrides.
4. **CLI flags** (`--cfg plugins.command.serve.run=deny`): one-shot overrides.

A user can set `run = "allow"` globally and override it with `run = "deny"` in a
workspace that handles sensitive data.

### Plugin Management Without a Workspace

Plugin management commands (`jp plugin list`, `install`, `uninstall`, `update`,
`approve`, and `revoke`) run from any directory without requiring a JP
workspace.
Because plugin binaries and approvals are stored globally in
`$XDG_DATA_HOME/jp/`, users do not need to run `jp init` before managing
plugins.

Management commands therefore load only the user-global config layer; workspace
and local config layers are unavailable.
A `run = "deny"` rule in a workspace config blocks `jp <plugin>` dispatch within
that workspace, but does not prevent `jp plugin install` from downloading the
plugin.
Because `jp plugin approve` spawns the binary once for `describe`, it enforces
`run = "deny"` when set in the user-global config.

### Dispatch Integration

Admission runs before every plugin process spawn, including `describe` probes
([RFD 072]).
By the time admission runs, routing has already resolved the binary path
(downloading a missing official plugin if needed, which does not read config).
Admission then evaluates the plugin's resolved configuration in order.
Its `run` is `plugins.command.<name>.run`, or `plugins.command.'*'.run` for a
plugin that sets none:

1. **Deny**: If `run = "deny"`, abort with an error that names the key that
   denies.
2. **Pinned checksum**: If `checksum` is configured and the binary's digest does
   not match, abort.
   This check also applies to newly downloaded official binaries: the registry
   checksum verifies download integrity, while the config pin verifies that the
   binary matches the version the user reviewed.
3. **Allow**: If `run = "allow"`, spawn the binary.
4. **Ask**: Spawn the binary if it is an official plugin whose SHA-256 matches
   its registry entry, or if its path and SHA-256 match an entry in the
   [approval store](#approval-store).
   Otherwise, prompt the user for approval, or abort if no interactive terminal
   is attached (see [Approval Store](#approval-store)).

### Future: Plugin Options Schema

The `options` field is an opaque JSON `Value` that JP forwards without
validation.
A future extension to the `describe` protocol can allow plugins to declare a
JSON Schema for their options:

```json
{
  "type": "describe",
  "name": "serve",
  "options_schema": {
    "type": "object",
    "properties": {
      "web": {
        "type": "object",
        "properties": {
          "port": { "type": "integer", "default": 3141 },
          "host": { "type": "string", "default": "127.0.0.1" }
        }
      }
    }
  }
}
```

Declaring a schema would enable config validation, `jp config show` integration,
and generated help text.
Schema validation is deferred; opaque options suffice for the initial plugin set
and avoid coupling `jp_config` to plugin internals.

## Drawbacks

- **No version constraints.** Configuration has no `version` field to constrain
  which plugin version is installed.
  Supporting version constraints requires version metadata in the registry and a
  resolution algorithm in JP.
  Checksum pinning provides a simpler guarantee: run this exact binary or fail.

- **Opaque options are unvalidated.** JP does not catch typos such as
  `options.web.prrt`; error reporting depends on the plugin.
  This matches the tool options tradeoff ([RFD 042]) until the `describe`
  options schema is implemented.

- **Manual checksum pinning.** Users must obtain the checksum digest manually
  (for example, from the registry or via `shasum`) and paste it into config.
  A future `jp plugin pin <name>` command could automate this.

- **Approvals invalidate on binary changes.** Upgrading a third-party plugin or
  rebuilding a local plugin changes its digest and triggers a new approval
  prompt.
  Users developing a plugin locally can set `run = "allow"` to trust it by name
  instead.

- **Single installed version per machine.** `jp plugin update` updates an
  official plugin across all workspaces.
  A workspace cannot hold an older version without pinning `checksum`, which
  causes the updated binary to fail admission in that workspace.
  Supporting per-workspace plugin versions would require a workspace lockfile.

## Alternatives

### Standalone Approval File Only

The initial Phase 3 implementation used
`$XDG_DATA_HOME/jp/plugin-approvals.json` as the sole trust mechanism, tracking
the approved binary path and checksum per plugin without a `[plugins]` config
section.

Rejected as the sole mechanism because:

- It does not participate in config inheritance, so a workspace cannot deny a
  globally approved plugin.
- It cannot express name-based trust (`allow`) or explicit blocking (`deny`).
- It conflates execution policy (whether the plugin may run) with binary
  identity (whether the file matches a known digest).

Retained in a narrower role as the [Approval Store](#approval-store) to cache
user responses under `run = "ask"`.
Configuration controls execution policy and optional checksum pinning across
layers, while the store records which specific binaries the user has approved,
at the cost of maintaining state in a separate file alongside config.

### Storing Approvals in Config

Persist interactive approvals by writing `run = "allow"` and a `checksum` pin to
the user-global config file.

Rejected because:

- Conversations record the resolved configuration in their event streams, which
  would leak machine-local paths and digests into conversations committed to
  version control.
- A `checksum` pin hard-fails when a binary changes, whereas an interactive
  approval under `ask` should prompt the user again after an upgrade.
- Interactive prompts cannot mutate a read-only user-global config file (such as
  one generated by a system configuration tool).

If JP later introduces a machine-local, JP-managed config layer that is excluded
from conversation streams, the approval store can move into it.

### Configurable Auto-Install

Control automatic installation via `plugins.auto_install` and per-plugin
`install` settings, including for third-party plugins.

Rejected because:

- Evaluating install config requires resolving configuration before the plugin
  binary exists on disk, but which config layers apply depends on the plugin's
  workspace scope ([RFD 114]), which is read from the binary itself.
  Resolving config before download would either duplicate workspace scope in the
  registry or let one workspace's config govern the download and another's
  govern execution.
- A repository's `.jp/config.toml` could trigger automatic downloads of
  third-party binaries when a user clones the project and runs a command.
  Requiring explicit installation ensures third-party binaries arrive only when
  the user runs `jp plugin install` or places them on `$PATH`, matching `cargo
  install`.
- Official plugins do not need a config toggle: system package managers can
  place their binaries on `$PATH`, and `JP_NO_PLUGIN_DOWNLOAD` disables
  automatic downloads altogether.

### Environment Variables for Plugin Options

Pass plugin options via environment variables instead of the config file (for
example, `JP_PLUGIN_SERVE_PORT=3141`).

Rejected because:

- Environment variables do not compose with layered config inheritance.
- Nested option structures do not map cleanly to flat environment variables.
- The command plugin protocol already sends the full config in the `init`
  message, so no extra transport is needed.

### Typed Plugin Config Sections

Define a typed struct per plugin in `jp_config` (for example,
`ServePluginConfig` with `port: u16` and `host: String`).

Rejected for the same reasons as in [RFD 042]: it couples `jp_config` to plugin
internals and does not scale to third-party plugins.
Opaque `Value` options provide a plugin-agnostic starting point, with the
`describe` schema protocol as the future validation layer.

## Non-Goals

- **Plugin version management.** Semantic version constraints, update channels,
  and rollback are package-manager features that are out of scope.
  Checksum pinning covers the security use case.

- **Options schema validation.** Validating plugin options against a
  plugin-declared schema is deferred to a future RFD extending the `describe`
  protocol.

- **Wasm plugin configuration.** [RFD 016] defines the wasm plugin system.
  When wasm plugins need configuration, the `plugins.wasm.<name>` namespace is
  reserved, with its schema left to that design.

## Risks and Open Questions

- **Checksum rotation workflow.** When a pinned plugin is legitimately updated,
  users must update the configured digest manually.
  If checksum pinning becomes common, plugin authors will need a clear way to
  publish digests (release notes or a `jp plugin pin --update` command).

- **Options forwarding path.** The `init` message sends the full `AppConfig` as
  JSON, placing plugin options at `plugins.command.<name>.options`.
  Extracting the active plugin's options into a top-level `options` field on the
  `init` message is cleaner and should be coordinated with [RFD 072].

- **Config scope during management commands.** As described in [Plugin
  Management Without a Workspace](#plugin-management-without-a-workspace),
  management commands load only user-global config.
  Supporting workspace-aware management commands (such as workspace-scoped
  plugin lists) would require loading the workspace when one is available.

## Implementation Plan

- [x] **Phase 1: Config types and dispatch integration**

  - Add `PluginsConfig`, `CommandPluginConfig`, and `RunPolicy` to
        `jp_config`.
  - Wire `plugins` into `AppConfig` with full `AssignKeyValue` /
        `PartialConfigDelta` / `ToPartial` support.
  - Reuse `ChecksumConfig` from MCP for checksum pinning.
  - Update `resolve_plugin_binary` to read config for policy decisions.
  - Keep the approval file as the [Approval Store](#approval-store).
        Its remaining changes (the prompt, `jp plugin approve` and `revoke`, and
        admission before every spawn) are [RFD 072] Phase 6.
  - Can be merged independently.

- [x] **Phase 2: Options forwarding**

  - Extract `plugins.command.<name>.options` from the config and include it
        in the plugin's `init` message in a well-known location.
  - Document the options path for plugin authors.
  - Depends on Phase 1.

- [x] **Phase 3: Plugin kind in registry**

  - [x] Add a `type` field to `RegistryPlugin` (defaulting to `"command"`).
  - [x] Filter on `type` in the dispatch pipeline.
  - [x] Update `jp plugin list` to show plugin kind.
  - Can be merged independently of Phase 1.

- [x] **Phase 4: `allow`**

  - Rename `run = "unattended"` to `allow`, accepting `unattended` as a
        deprecated alias that logs a warning.
  - Rename the matching `unattended` value on tool and label settings
        (`run`, `result`, and `format`) in the same change so config uses a
        single term.
  - Can be merged independently.

- [x] **Phase 5: Explicit installs**

  - [x] Remove `plugins.auto_install` and `plugins.command.<name>.install`
            ([RFD 072] Phase 5).
  - [x] Make `ask` the default `run` policy for every plugin, running
            official binaries without prompting when their SHA-256 matches the
            registry ([RFD 072] Phase 6).
  - [x] Record an approval entry on `jp plugin install`, and set `installed`
            on every binary JP downloads ([RFD 072] Phase 6).

## References

- [RFD 072: Command Plugin System][RFD 072]
- [RFD 016: Wasm Plugin Architecture][RFD 016]
- [RFD 042: Tool Options][RFD 042]
- [RFD 075: Tool Sandbox and Access Policy][RFD 075]
- [RFD 114: Plugin Workspace Scope and Addressing][RFD 114]

[RFD 016]: 016-wasm-plugin-architecture.md
[RFD 042]: 042-tool-options.md
[RFD 072]: 072-command-plugin-system.md
[RFD 075]: 075-tool-sandbox-and-access-policy.md
[RFD 114]: 114-plugin-workspace-scope-and-addressing.md
