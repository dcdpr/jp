# RFD 077: Plugin Configuration and Trust Policy

- **Status**: Discussion
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-04-07
- **Summary**: Plugin configuration integration with AppConfig enabling trust
  policies, checksum pinning, execution policies, and per-plugin options.

## Summary

This RFD defines the configuration surface for JP's plugin system.
It introduces a `[plugins]` section in `AppConfig` that controls execution
policy, binary checksum pinning, and per-plugin options.
Config is the source of truth for plugin trust policy, and it participates in
JP's existing config inheritance chain (global → workspace → local → CLI
overrides).
Under the `ask` policy, a user-local approval store remembers the user's
answers, so a prompt answered once is not asked again for the same binary.
Installing is not configured: official plugins install on first use, and
third-party plugins only when the user runs `jp plugin install`.

## Motivation

[RFD 072] defines the command plugin system: standalone binaries that
communicate with JP over a JSON-lines protocol.
Phase 3 of that RFD adds a plugin registry and auto-install flow.
But [RFD 072] leaves open how users control plugin behavior:

- **Trust decisions are ad-hoc.** Without config integration, plugin approval
  must be tracked in a separate JSON file that doesn't benefit from config
  inheritance.
  A user who trusts a plugin globally has no way to deny it in a specific
  workspace, or vice versa.

- **No checksum pinning.** The registry provides checksums for download
  verification, but there is no mechanism for a user to pin a specific binary
  and refuse to run a changed one.
  This matters for supply-chain security: if a registry-hosted binary is
  compromised and re-published with a new checksum, users who haven't pinned are
  silently exposed.

- **No per-plugin options.** Plugins like `jp-serve` need configuration (bind
  address, port) that is specific to the plugin.
  Without a config path, these settings must be passed as CLI arguments every
  time, or the plugin must invent its own config file.

- **Plugin types aren't distinguished.** The registry currently assumes all
  plugins are command plugins.
  When wasm plugins ([RFD 016]) arrive, the registry and config need a way to
  distinguish between plugin kinds.

This RFD addresses all four gaps by defining the plugin config model and its
interaction with the dispatch pipeline.

## Design

### Plugin Kind Taxonomy

The registry introduces a `type` field on each plugin entry:

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

`type` defaults to `"command"` when absent, so existing registry entries remain
valid.
The dispatch pipeline filters on `type` and ignores entries with unrecognized
values, allowing future plugin types (e.g. `wasm`) to be added to the registry
without breaking older JP versions.

The `PluginKind` enum in `jp_plugin::registry`:

```rust
pub enum PluginKind {
    Command,  // standalone binary, RFD 072 protocol
    // Wasm,  // future: RFD 016
}
```

### Configuration Schema

The `[plugins]` section is added to `AppConfig`:

```toml
[plugins]
# Grace period (seconds) before SIGKILL after sending Shutdown.
shutdown_timeout_secs = 5         # default: 5

# Per-plugin configuration, keyed by plugin name.
[plugins.command.serve-web]
run = "allow"                     # execution policy
```

#### `plugins.shutdown_timeout_secs`

The grace period between sending `Shutdown` over the protocol and killing the
plugin and everything it started.
Applies to all command plugins.

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

**`run`** (`RunPolicy`) — Execution policy:

| Value   | Behavior                                                    |
| ------- | ----------------------------------------------------------- |
| `ask`   | Run a binary JP can vouch for, and prompt for any other.    |
|         | The default for every plugin.                               |
| `allow` | Run without prompting.                                      |
| `deny`  | Never run. JP exits with an error if the plugin is invoked. |

The `run` policy applies at every execution point, wherever the binary came
from.
Under `ask`, JP vouches for two kinds of binary: an official one whose SHA-256
matches its registry entry, and one the [approval store](#approval-store) holds.
An official plugin therefore runs without a prompt, as if it were part of `jp`,
for as long as its binary is the one the registry publishes.
`allow` trusts a plugin by name, whatever its binary: it suits a plugin that is
rebuilt often, which `ask` would prompt for after every build.

`allow` names the decision, not how the plugin runs: a plugin that runs without
a prompt can still ask the user questions.
`unattended` is accepted as a deprecated spelling of `allow` and logs a warning.

**`checksum`** — Pins the binary to a specific hash.
When set, JP computes the binary's checksum before execution and refuses to run
if it doesn't match.
This catches two scenarios:

1. A registry-hosted binary is re-published with different content (supply-chain
   compromise).
2. A PATH-discovered binary is replaced or modified.

The checksum config reuses the existing `ChecksumConfig` type from MCP server
configuration:

```rust
pub struct ChecksumConfig {
    pub algorithm: AlgorithmConfig,  // sha256 (default) | sha1
    pub value: String,               // hex-encoded digest
}
```

When a checksum mismatch occurs, JP prints the expected and actual values and
tells the user which config key to update.
This makes it easy to intentionally accept a new binary after reviewing the
change.

**`options`** — A table of opaque values passed to the plugin in the `options`
field of the `init` message.
JP does not validate the contents — the plugin is responsible for parsing and
error reporting.
Options merge key by key across config layers, recursing into nested tables, so
a later layer only replaces the options it names.
This follows the same pattern as tool options ([RFD 042]).

Example: the `serve-web` plugin reads `options.bind` and `options.port` from
`init` to configure its HTTP listener.

### Installing

No config decides whether a plugin is installed:

- **Official plugins install on first use.** Typing an official command whose
  plugin is missing downloads it, checks it against the registry's checksum, and
  runs it ([RFD 072]).
  `jp plugin update` updates it the same way when the registry publishes a new
  binary.
- **Third-party plugins install only when asked.** `jp plugin install <name>`
  shows where the binary comes from and asks before downloading it.
  Answering yes is also the approval to run it, so the first run does not ask
  again.
  A binary the user places on `$PATH` is installed by that act, and its first
  run asks.

`JP_NO_PLUGIN_DOWNLOAD=1` stops JP from downloading an official plugin on its
own, for a machine where binaries arrive through a package manager or not at
all.

### Approval Store

Under `ask`, the host remembers the user's answer.
Running `jp plugin install <name>` for a third-party plugin, answering `Y` at
the prompt, or running `jp plugin approve <path>` records an approval in
`$XDG_DATA_HOME/jp/plugin-approvals.json`:

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

An approval is keyed by the plugin's name, the same key as
`plugins.command.<name>` ([RFD 072]), and holds one binary: the path it was
found at and the SHA-256 of its contents.
`installed` marks a binary JP wrote itself, for an official plugin on first use
or for `jp plugin install`.
An official binary needs no approval to run, but the record is how JP tells a
binary it can update from one changed on this machine, which it leaves alone.

An approval answers the prompt for that exact file with those exact bytes, and
for nothing else:

- **A changed binary asks again.** The same path with different contents, after
  an upgrade or a rebuild, prompts and says the binary changed since it was
  approved.
- **Another path asks again, naming the approved one.** A binary with the same
  name at a different path prompts with both paths.
  While both files exist they conflict before admission is reached ([RFD 072]),
  so this is the case where the approved binary has moved or gone.
- **Policy comes first.** The store is consulted only when the effective `run`
  policy is `ask`, after a pinned checksum has matched.
  A `deny` from any config layer refuses a plugin the user approved, and `allow`
  needs no approval.

The prompt shows the binary's path, and says which kind of binary it is asking
about.
A third-party binary that claims an official command replaces the official
plugin ([RFD 072]), so its prompt says so, with **third-party** in bold red:

```txt
  → jp serve web is claimed by the third-party plugin `webui`, which replaces
    the official one.
    /Users/jean/.cargo/bin/jp-webui
Run it? [y/Y/N]
```

An official plugin's binary that does not match its registry entry, such as a
copy a package manager built, is asked about as not matching the official
release.

`jp plugin approve <path>` names a file, which is consent to run it once: the
host sends it `describe`, shows its path, description, and claims, refuses when
the answer disagrees with the binary's manifest, and records the approval.
For a binary whose manifest cannot be read, such as one compressed with UPX, the
approval also records the manifest fields from the answer, under `manifest`,
which is the only way such a binary gets claims to route by ([RFD 072]).
`jp plugin revoke <name>` removes an approval.
`jp plugin uninstall <name>` deletes a binary from JP's install directory along
with its approval; a binary on `$PATH` belongs to whatever put it there, so
`uninstall` refuses it and points at `revoke`.
`jp plugin list` shows each plugin's approval state.

Without a terminal the host cannot prompt.
It refuses the plugin and prints the `jp plugin approve` command that would
admit it.

Approvals are user-global, not per workspace.
An approval answers whether the binary may run as the user, and a binary has the
same reach in every workspace; a plugin that serves several workspaces ([RFD
114]) reaches all of them at once.
Refusing a plugin in one workspace is config's job: `run = "deny"` in that
workspace's config.

Approvals are kept out of config because config does not stay on this machine.
A conversation records the configuration it started with and every change to it,
and a conversation projected into a checkout is usually committed with the
project.
An approval names a path on this machine, and belongs in neither.
The store follows the trust-on-first-use pattern JP uses for tool mounts outside
the workspace, but is user-global where that one is per workspace, because
approving a plugin works outside any workspace.

### Config Inheritance

Plugin config participates in JP's standard config inheritance chain:

1. **Global config** (`$XDG_CONFIG_HOME/jp/config.toml`): user-wide defaults.
   Trust a plugin globally, set default options.
2. **Workspace config** (`.jp/config.toml`): per-project overrides.
   Deny a plugin in a sensitive workspace, or change its options.
3. **Local config** (`.jp.toml`): directory-scoped overrides.
4. **CLI flags** (`--cfg plugins.command.serve.run=deny`): one-shot overrides.

This means a user can set `run = "allow"` globally and override it to `run =
"deny"` in a workspace that handles sensitive data.

### Plugin Management Without a Workspace

Plugin management commands (`jp plugin list`, `install`, `uninstall`, `update`,
`approve`, and `revoke`) need no workspace.
They work from any directory, including outside of any JP workspace.
This is intentional: plugin binaries and approvals are user-global (installed to
`$XDG_DATA_HOME/jp/plugins/`), not workspace-local, so requiring `jp init`
before installing a plugin would be unnecessary friction.

The trade-off is that management commands only see the user-global config layer.
Workspace and local config layers are unavailable.
This means a `run = "deny"` set in a workspace config is enforced during `jp
<plugin>` dispatch (which runs inside a workspace) but not during `jp plugin
install` (which runs outside).
This is acceptable: `deny` means "don't run this plugin here," not "don't
download it."

Approving runs the plugin once, so `jp plugin approve` refuses a plugin the
user-global config denies.

### Dispatch Integration

Admission precedes every spawn of a plugin binary, including one that only
answers `describe` ([RFD 072]).
Routing has picked a binary by then, downloading an official plugin that was
missing; that download reads no config.
Admission then decides under the plugin's configuration, in order:

1. **Deny check**: If `plugins.command.<name>.run = "deny"`, refuse.
2. **Pinned checksum**: If a checksum is pinned and the binary does not match
   it, refuse.
   This also covers a binary just downloaded from the registry: the registry
   checksum proves the download is intact, and the pin proves it is still the
   binary the user last reviewed.
3. **`allow`**: Run.
4. **`ask`**, the default: run an official binary whose SHA-256 matches its
   registry entry, or a binary the approval store holds at this path with these
   contents.
   Prompt for anything else, and without a terminal, refuse (see [Approval
   Store](#approval-store)).

### Future: Plugin Options Schema

The current `options` field is an opaque `Value` — JP passes it through without
validation.
A future extension could have plugins declare their options schema via the
`describe` protocol:

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

This would enable config validation, `jp config show` integration, and help text
generation.
It is explicitly deferred — the opaque approach is sufficient for the initial
plugin set and avoids coupling the config system to plugin internals.

## Drawbacks

- **No version constraints.** The config has no `version` field for constraining
  which plugin version to install.
  This requires the registry to carry version metadata and JP to implement a
  resolution algorithm.
  The checksum pin provides a weaker but simpler guarantee: "run exactly this
  binary, or nothing."

- **Opaque options are unvalidated.** A typo in `options.web.prrt` is silently
  ignored.
  The plugin may or may not report the error.
  This is the same tradeoff as tool options ([RFD 042]) and is acceptable until
  the options schema protocol is implemented.

- **No checksum auto-population.** Users must manually obtain the checksum value
  (e.g., from the registry or by running `shasum`) and paste it into config.
  A future `jp plugin pin <name>` command could automate this.

- **Approvals are per binary.** A third-party plugin asks again after every
  upgrade, and a plugin under development after every rebuild.
  `run = "allow"` trusts a plugin by name instead, for the user who wants that.

- **One installed version per machine.** `jp plugin update` updates an official
  plugin for every workspace at once, and a workspace cannot hold it at an older
  version except by pinning its checksum, which stops the update everywhere.
  Per-workspace versions need a plugin lock file, which is its own design.

## Alternatives

### Standalone approval file

The initial Phase 3 implementation stored plugin approvals in a separate
`$XDG_DATA_HOME/jp/plugin-approvals.json` file, tracking binary path and
checksum per approved plugin, as the only trust mechanism.

Rejected as the trust mechanism because:

- Does not participate in config inheritance.
  Can't deny a plugin per-workspace.
- No support for execution policy beyond binary approve/deny.
- Mixes concerns: trust decisions (should this run?) and identity assertions (is
  this the right binary?) should be expressible independently.

Kept as the [Approval Store](#approval-store), which only remembers answers
given under `ask`.
Config decides whether the host asks, and the store records what the user
answered for one binary, so policy still layers, a workspace can still deny a
plugin, and a pinned checksum still asserts identity on its own.
The cost is a second file to maintain beside config.

### Approvals in config

The prompt records an approval as config, `run = "allow"` with a pinned
checksum, in the user-global config file.

Rejected because:

- Config does not stay on the machine.
  Every conversation records its configuration, which would carry local paths
  into checkouts where conversations are committed.
- A pinned checksum refuses a changed binary, where an approval should ask
  again.
- The prompt cannot write to a config file the user manages read-only, such as
  one generated by a system configuration tool.

A configuration layer JP manages itself and keeps out of conversations could
hold approvals.
That is a design of its own, and the store can move into it once it exists.

### Configurable installs

`plugins.auto_install`, and an `install` setting per plugin, decide whether a
plugin that is missing is installed when its command is typed, third-party
plugins included.

Rejected because:

- Config that decides an install has to be resolved before the binary exists,
  and which configuration applies depends on the plugin's workspace scope ([RFD
  114]), which is read from the binary.
  Resolving it anyway means copying the scope into the registry, or letting one
  workspace's configuration decide the download and another's the run.
- A third-party binary would arrive on the machine because a config file said
  so, including one committed to a repository the user cloned.
  With installs explicit, third-party code arrives only when the user runs `jp
  plugin install` or puts a binary on `$PATH`, as with `cargo install`.
- Official plugins need no switch: a package manager puts their binaries on
  `$PATH`, and `JP_NO_PLUGIN_DOWNLOAD` stops downloads altogether.

### Environment variables for plugin options

Pass plugin options via environment variables instead of the config file (e.g.,
`JP_PLUGIN_SERVE_PORT=3141`).

Rejected because:

- Doesn't compose with config inheritance.
- Awkward for nested options (port is fine, but complex structures don't map
  well to env vars).
- The plugin protocol already sends the full config in the `init` message, so
  the transport is free.

### Typed plugin config sections

Define a typed struct per plugin in `jp_config` (e.g., `ServePluginConfig` with
`port: u16` and `host: String`).

Rejected for the same reasons as in [RFD 042]: it couples the config crate to
plugin internals and doesn't scale to third-party plugins.
The opaque `Value` approach is the right starting point, with the schema
protocol as the future validation layer.

## Non-Goals

- **Plugin version management.** Semantic version constraints, update channels,
  and rollback are package-manager features that are out of scope.
  Checksum pinning covers the security use case.

- **Options schema validation.** Validating plugin options against a
  plugin-declared schema is deferred to a future RFD extending the `describe`
  protocol.

- **Wasm plugin configuration.** [RFD 016] defines the wasm plugin system.
  When wasm plugins need configuration, the `plugins.wasm.<name>` namespace is
  reserved but its schema is undefined here.

## Risks and Open Questions

- **Checksum rotation workflow.** When a plugin is legitimately updated, users
  with a pinned checksum must manually update the value.
  If many users pin checksums, plugin authors need a way to communicate new
  checksums (release notes, a `jp plugin pin --update` command, etc.).
  The UX for this needs attention.

- **Options forwarding path.** The `init` message sends the full `AppConfig` as
  JSON.
  Plugin-specific options currently live at `plugins.command.<name>.options` in
  this blob.
  Plugins must navigate this path to find their options.
  A cleaner approach might extract the plugin's options and send them in a
  dedicated `options` field in the `init` message.
  This is a protocol change that should be coordinated with [RFD 072].

- **Config scope during management commands.** As described in the "Plugin
  Management Without a Workspace" section, management commands only see
  user-global config.
  If a future use case requires workspace-aware management (e.g.
  workspace-scoped plugin lists), the management commands would need to
  optionally load the workspace when one is available.

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
        deprecated spelling that logs a warning.
  - Tool and label settings that use the same value (`run`, and a tool's
        `result` and `format`) are renamed in the same change, so config uses
        one word for it.
  - Can be merged independently.

- [x] **Phase 5: Explicit installs**

  - [x] Remove `plugins.auto_install` and `plugins.command.<name>.install`
            ([RFD 072] Phase 5).
  - [x] Make `ask` the default `run` policy for every plugin, answered
            without a prompt by a matching registry checksum for an official
            binary ([RFD 072] Phase 6).
  - [x] Record an approval for `jp plugin install`, and an `installed`
            record for every binary JP writes ([RFD 072] Phase 6).

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
