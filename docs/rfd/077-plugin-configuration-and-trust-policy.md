# RFD 077: Plugin Configuration and Trust Policy

- **Status**: Discussion
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-04-07
- **Requires**: [RFD 072]

## Summary

This RFD defines the configuration surface for JP's plugin system.
It introduces a `[plugins]` section in `AppConfig` that controls plugin
installation, execution policy, binary checksum pinning, and per-plugin options.
The config system replaces standalone approval files as the single source of
truth for plugin trust decisions, and it participates in JP's existing config
inheritance chain (global → workspace → local → CLI overrides).

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

The registry introduces a `kind` field on each plugin entry:

```json
{
  "version": 1,
  "plugins": {
    "serve": {
      "kind": "command",
      "description": "Read-only web UI for conversations",
      "official": true,
      "binaries": { ... }
    }
  }
}
```

`kind` defaults to `"command"` when absent, so existing registry entries remain
valid.
The dispatch pipeline filters on `kind` and ignores entries with unrecognized
values, allowing future plugin types (e.g.
`wasm`) to be added to the registry without breaking older JP versions.

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
# Auto-install official registry plugins on first invocation.
auto_install = true               # default: true

# Grace period (seconds) before SIGKILL after sending Shutdown.
shutdown_timeout_secs = 5         # default: 5

# Per-plugin configuration, keyed by plugin name.
[plugins.command.serve]
install = true                    # override auto_install per plugin
run = "unattended"                # execution policy
```

#### `plugins.auto_install`

Controls whether official plugins are automatically downloaded and installed
when first invoked.
When `false`, the user must run `jp plugin install <name>` explicitly.
Third-party (non-official) plugins are never auto-installed regardless of this
setting unless their per-plugin `install` field is `true`.

#### `plugins.shutdown_timeout_secs`

The grace period between sending `Shutdown` over the protocol and sending
SIGKILL to the child process.
Applies to all command plugins.

#### Per-Plugin Configuration

Each entry under `plugins.command.<name>` configures a specific command plugin:

```toml
[plugins.command.serve]
install = true
run = "unattended"

[plugins.command.serve.checksum]
algorithm = "sha256"
value = "e3b0c44298fc1c149afbf4c8996fb924..."

[plugins.command.serve.options]
web.port = 3141
web.host = "127.0.0.1"
```

**`install`** (`Option<bool>`) — Whether to auto-install this plugin from the
registry.
Overrides the global `auto_install` for this specific plugin.
When absent, falls back to the global setting (for official plugins) or `false`
(for third-party plugins).

**`run`** (`RunPolicy`) — Execution policy:

| Value        | Behavior                                                    |
| ------------ | ----------------------------------------------------------- |
| `ask`        | Prompt the user before running. Default for third-party     |
|              | plugins and PATH-discovered plugins.                        |
| `unattended` | Run without prompting. Default for official registry        |
|              | plugins.                                                    |
| `deny`       | Never run. JP exits with an error if the plugin is invoked. |

The `run` policy applies at every execution point: installed plugins, registry
auto-install, and PATH-discovered plugins.
It replaces the standalone approval file system from the initial Phase 3
implementation.

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

**`options`** (`Option<serde_json::Value>`) — An opaque JSON value passed to
the plugin in the `config` field of the `init` message.
JP does not validate the contents — the plugin is responsible for parsing and
error reporting.
This follows the same pattern as tool options ([RFD 042]).

Example: the `serve` plugin reads `options.web.port` and `options.web.host` from
its init config to configure its HTTP listener.

### Config Inheritance

Plugin config participates in JP's standard config inheritance chain:

1. **Global config** (`$XDG_CONFIG_HOME/jp/config.toml`): user-wide defaults.
   Trust a plugin globally, set default options.
2. **Workspace config** (`.jp/config.toml`): per-project overrides.
   Deny a plugin in a sensitive workspace, or change its options.
3. **Local config** (`.jp.toml`): directory-scoped overrides.
4. **CLI flags** (`--cfg plugins.command.serve.run=deny`): one-shot overrides.

This means a user can set `run = "unattended"` globally and override it to `run
= "deny"` in a workspace that handles sensitive data.

### Plugin Management Without a Workspace

Plugin management commands (`jp plugin list`, `jp plugin install`, `jp plugin
update`) run before workspace loading.
They work from any directory, including outside of any JP workspace.
This is intentional: plugin binaries are user-global (installed to
`$XDG_DATA_HOME/jp/plugins/`), not workspace-local, so requiring `jp init`
before installing a plugin would be unnecessary friction.

The trade-off is that management commands only see the user-global config layer.
Workspace and local config layers are unavailable.
This means a `run = "deny"` set in a workspace config is enforced during `jp
<plugin>` dispatch (which runs inside a workspace) but not during `jp plugin
install` (which runs outside).
This is acceptable: `deny` means "don't run this plugin here," not "don't
download it."

### Dispatch Integration

The dispatch pipeline in `resolve_plugin_binary` reads `AppConfig::plugins` to
make decisions at each resolution step:

1. **Deny check**: If `plugins.command.<name>.run = "deny"`, abort immediately.
2. **Installed plugins**: Verify pinned checksum if configured.
3. **Registry install**: Respect `install` and `run` policy.
   Official plugins with `auto_install = true` and `run = "unattended"` install
   silently.
   Third-party plugins with `run = "ask"` prompt interactively.
4. **PATH plugins**: Verify pinned checksum.
   Respect `run` policy.
   Default is `ask`, which prompts once per invocation (not persisted — use
   `run = "unattended"` in config for permanent trust).
5. **Post-install verification**: After downloading from the registry, verify
   against the pinned checksum if one is configured.
   This catches the scenario where the registry checksum passes (download is
   intact) but differs from the user's pinned value (binary changed since the
   user last reviewed it).

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

## Alternatives

### Standalone approval file

The initial Phase 3 implementation stored plugin approvals in a separate
`$XDG_DATA_HOME/jp/plugin-approvals.json` file, tracking binary path and
checksum per approved plugin.

Rejected because:

- Does not participate in config inheritance.
  Can't deny a plugin per-workspace.
- Separate persistence mechanism to maintain alongside the config system.
- No support for execution policy beyond binary approve/deny.
- Mixes concerns: trust decisions (should this run?) and identity assertions (is
  this the right binary?) should be expressible independently.

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

### Phase 1: Config types and dispatch integration

- Add `PluginsConfig`, `CommandPluginConfig`, and `RunPolicy` to `jp_config`.
- Wire `plugins` into `AppConfig` with full `AssignKeyValue` /
  `PartialConfigDelta` / `ToPartial` support.
- Reuse `ChecksumConfig` from MCP for checksum pinning.
- Update `resolve_plugin_binary` to read config for policy decisions.
- Remove the standalone approval file system.
- Can be merged independently.

### Phase 2: Options forwarding

- Extract `plugins.command.<name>.options` from the config and include it in the
  plugin's `init` message in a well-known location.
- Document the options path for plugin authors.
- Depends on Phase 1.

### Phase 3: Plugin kind in registry

- Add `kind` field to `RegistryPlugin` (defaulting to `"command"`).
- Filter on `kind` in the dispatch pipeline.
- Update `jp plugin list` to show plugin kind.
- Can be merged independently of Phase 1.

## References

- [RFD 072: Command Plugin System][RFD 072]
- [RFD 016: Wasm Plugin Architecture][RFD 016]
- [RFD 042: Tool Options][RFD 042]
- [RFD 075: Tool Sandbox and Access Policy][RFD 075]

[RFD 016]: 016-wasm-plugin-architecture.md
[RFD 042]: 042-tool-options.md
[RFD 072]: 072-command-plugin-system.md
[RFD 075]: 075-tool-sandbox-and-access-policy.md
