# RFD 072: Command Plugin System

- **Status**: Discussion
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-04-06

## Summary

This RFD introduces a command plugin system for JP. Command plugins are
standalone binaries (`jp-<name>`) that communicate with JP over a structured
JSON-lines protocol on stdin/stdout. JP handles workspace discovery, config
loading, conversation locking, data access, output formatting, and signal
management. Plugins request these services over the protocol, making it possible
to write a plugin in any language — including shell scripts.

This is one of several plugin mechanisms in JP. [RFD 016] defines the Wasm
plugin system for sandboxed in-process capabilities (attachment handlers, tools,
LLM providers). Command plugins operate at a different level: they are
long-running processes that extend JP with new subcommands.

## Motivation

JP's functionality is growing beyond its core query loop. An ongoing experiment
with a web UI is the first example: a long-running server that reads (and will
soon write) conversation data. Possible future candidates include HTTP APIs, TUI
dashboards, import/export tools, and IDE integrations.

Today, adding any of these means compiling them into the `jp` binary. This has
costs:

- **Binary size**: The web server pulls in axum, hyper, tower, and maud. Every
  user pays this cost whether they use the web UI or not.
- **Coupling**: Every extension must be Rust, must link against JP's internal
  crates, and must be wired into the `Commands` enum and startup pipeline.
- **Release cadence**: A bug fix in the web UI requires a full JP release.

A plugin system solves these problems. But the design must handle a tension that
cargo-style "just exec the binary" dispatch does not face: JP's startup pipeline
provides services (workspace discovery, config loading, conversation locking,
structured output) that plugins need. If we push all of that into the plugin,
every plugin author re-implements JP's bootstrap — and gets it wrong (we already
hit this with the web server missing `.with_local_storage()`).

The goal is a plugin system where:

1. JP remains the orchestrator — it finds the workspace, loads config, manages
   locks, and formats output.
2. Plugins are standalone executables that can be written in any language.
3. A shell script can be a useful plugin.
4. A plugin that starts read-only (web viewer) can gain write access (chat)
   without changing its architecture.

## Design

### User Experience

Plugins are invoked as JP subcommands and can hook into any level of the command
hierarchy:

```txt
jp serve                        # runs jp-serve plugin
jp serve web                    # runs jp-serve-web plugin
jp export --format html         # runs jp-export plugin
jp dashboard                    # runs jp-dashboard plugin
jp conversation export --html   # runs jp-conversation-export plugin
jp conversation stats           # runs jp-conversation-stats plugin
```

Each plugin declares the command path it provides via the `command` field in
its `Describe` response (see [Plugin Self-Description](#plugin-self-description)).
For example, a plugin with `"command": ["serve", "web"]` handles `jp serve web`
regardless of its binary name.

When no `command` field is present, the binary name determines the command
path: the `jp-` prefix is stripped, and remaining `-`-separated segments form
the path. `jp-conversation-export` provides `jp conversation export`. This is
the simplest integration path for plugins that expose a single command and
don't need explicit routing control.

A plugin cannot shadow a built-in command at any level — JP checks built-in
commands first.

When JP encounters an unknown subcommand, it:

1. Checks the user-local install directory
   (`$XDG_DATA_HOME/jp/plugins/command/`) for an already-installed binary.
2. Checks the local plugin registry cache for a known plugin.
3. If found in the registry: installs the binary based on the `run` policy from
   a future `[plugins.command.<name>]` config. Official plugins default to
   unattended install; third-party plugins prompt.
4. If not in the registry: searches `$PATH` for `jp-<name>` (or
   `jp-parent-child` for nested commands). Runs based on the `run` policy
   (default: `ask`).
5. Verifies the binary's checksum against any pinned value in config.
6. Spawns the plugin binary and communicates over the protocol.

Users can also install plugins manually by placing a `jp-<name>` binary on
`$PATH`.

Plugin management commands (`jp plugin list`, `jp plugin install`,
`jp plugin update`) are a built-in subcommand group, not external plugins.
They are handled before workspace loading and work from any directory,
including outside a workspace. The `jp plugin` subcommand takes priority
over any external `jp-plugin` binary on `$PATH`.

### Channel Model

JP spawns the plugin as a child process with three channels:

| Channel    | Direction       | Purpose                                    |
|------------|-----------------|--------------------------------------------|
| **stdin**  | JP → plugin     | Protocol messages: init, responses          |
| **stdout** | plugin → JP     | Protocol messages: requests, print, log     |
| **stderr** | plugin → JP     | Captured and forwarded to JP's tracing      |
|            |                 | subsystem at `trace` level                  |

The plugin never writes directly to the user's terminal. All user-facing output
goes through the protocol as `print` commands, which JP routes through its
printer. This gives plugins automatic support for `--quiet`, `--format json`,
and other output modes.

Stderr is captured line-by-line and emitted as `trace`-level tracing events
attributed to the plugin. This provides a zero-effort debugging channel for
plugin authors — `fprintf(stderr, ...)` in C, `eprintln!()` in Rust, or
`echo >&2` in shell — without polluting user output.

### Protocol

JSON-lines over stdin/stdout. One JSON object per line, no framing beyond
newlines. Each message has a `type` field.

#### Stdout Hygiene

Using stdout for protocol framing means any non-protocol output from the
plugin (a stray `printf` in a C library, an uncaught panic message) corrupts
the JSON-lines stream. This is a known trade-off shared with LSP, MCP, and
git remote helpers, all of which use stdin/stdout successfully.

The mitigation is straightforward: stderr is the designated escape valve.
Plugin authors use stderr for all debugging output (`eprintln!()` in Rust,
`echo >&2` in shell, `fprintf(stderr, ...)` in C), and JP forwards it to
tracing. The protocol contract is simple: stdout is exclusively for protocol
messages.

Stdin/stdout is chosen because it is the simplest cross-language,
cross-platform transport — no socket setup, no file descriptor passing, and
it works in shell scripts with `echo` and `read`. An alternative transport
(FD 3/4, domain sockets) could be explored in a future protocol version if
stdout pollution proves to be a recurring problem in practice, but the added
complexity is not justified given the current design goal of shell-script
accessibility.

#### Request IDs

Plugin-to-JP messages may include an optional `id` field (string). When
present, JP echoes the same `id` in the corresponding response. This allows
multi-threaded plugins to issue concurrent requests and match responses to
the originating request.

For synchronous plugins (shell scripts, single-threaded tools), `id` can be
omitted entirely. JP processes requests in order and responses arrive in the
same order, so correlation is implicit.

```json
{
  "type": "list_conversations",
  "id": "a"
}
{
  "type": "read_events",
  "id": "b",
  "conversation": "17127583920"
}
```

Responses:

```json
{"type": "conversations", "id": "a", "data": [...]}
{"type": "events", "id": "b", "conversation": "17127583920", "data": [...]}
```

If a request has no `id`, the response also has no `id`.

#### Lifecycle

JP sends `init` immediately after spawning the plugin:

```json
{
  "type": "init",
  "version": 1,
  "workspace": {
    "root": "/path/to/project",
    "storage": "/path/to/project/.jp",
    "id": "a1b2c"
  },
  "config": {
    "server": {
      "web": {
        "bind": "127.0.0.1",
        "port": 3141
      }
    }
  },
  "args": [
    "--web"
  ],
  "log_level": 3
}
```

The `config` field contains the fully resolved `AppConfig` serialized as JSON.
The `args` field contains the remaining CLI arguments after the subcommand name.
The `log_level` field conveys the host's verbosity (0 = error, 1 = warn,
2 = info, 3 = debug, 4 = trace) so plugins can configure their own tracing to
match the user's `-v` flags.

The plugin acknowledges with:

```json
{
  "type": "ready"
}
```

When the plugin is done, it sends:

```json
{
  "type": "exit",
  "code": 0
}
```

For non-zero exits, an optional `reason` field provides a user-facing error
message that the host displays through its normal error rendering pipeline:

```json
{
  "type": "exit",
  "code": 1,
  "reason": "use `jp serve --web` to start the web server"
}
```

JP then cleans up any locks held on behalf of the plugin and exits with the
given code. If the plugin process exits without sending `exit` (crash, signal),
JP detects the EOF, releases locks, and exits with code 1.

#### Shutdown

When JP receives a signal (SIGINT, SIGTERM), it sends:

```json
{
  "type": "shutdown"
}
```

The plugin should begin graceful shutdown and eventually send `exit`. If the
plugin does not exit within a grace period (configurable, default 5 seconds),
JP sends SIGKILL (Unix) or `TerminateProcess` (Windows) to the child process.

To ensure the plugin receives `Shutdown` via the protocol rather than being
killed directly by the OS signal, JP spawns the child in its own process group
(`process_group(0)` on Unix). This prevents SIGINT/SIGTERM from reaching the
child directly — only the host receives the signal and relays it through the
protocol.

#### Plugin Self-Description

JP can request plugin metadata without a full initialization. This is used for
`jp -h` (listing available plugins) and `jp <plugin> -h` (showing plugin help).

Instead of `init`, JP sends:

```json
{
  "type": "describe"
}
```

The plugin responds with its metadata and exits:

```json
{
  "type": "describe",
  "name": "serve-web",
  "version": "0.1.0",
  "description": "Read-only web UI for browsing conversations",
  "command": [
    "serve",
    "web"
  ],
  "author": "Jean Mertz <git@jeanmertz.com>",
  "help": "Start the read-only web interface...\n\nUsage: jp serve web [OPTIONS]\n...",
  "repository": "https://github.com/dcdpr/jp"
}
```

All fields except `name`, `version`, and `description` are optional.

**`command`** (array of strings) — The subcommand path this plugin handles.
`["serve", "web"]` means the plugin handles `jp serve web`. When absent, the
host derives the path from the binary name by stripping the `jp-` prefix and
splitting on `-` (see [User Experience](#user-experience)). Plugins that
handle a command path with dashes in a segment (e.g., `jp serve http-api`)
must use the `command` field because the binary name convention is ambiguous
in that case.

The `description` field is used for the one-line listing in `jp -h`. The `help`
field is shown for `jp <plugin> -h`. A future RFD can introduce structured help
text that enables the host to render plugin help through clap for consistent
formatting.

When a plugin binary is invoked directly (not through `jp`), it should detect
that stdin is a TTY and print its own help to stderr before exiting.

For `jp -h`, the host discovers plugins by scanning `$PATH` for `jp-*`
binaries, spawning each, sending `describe`, and collecting responses. The
`command` field from each response determines where the plugin appears in the
command listing. Plugin descriptions are appended as a "Plugins:" section after
the built-in commands.

#### Help Aggregation for Command Groups

The registry supports `command_group` entries — command namespaces with no binary.
A group provides help text and lists sub-plugins, but does not execute any
code. When the user runs `jp serve -h` and `serve` is a group:

1. JP reads the group's `description` and `suggests` list from the registry.
2. Checks for installed plugins whose `command` path starts with `["serve",
   ...]`.
3. Checks the registry for uninstalled plugins under the same prefix.
4. Merges everything into the help output:

```txt
JP server components

Usage: jp serve <COMMAND>

Commands:
  web         Read-only web UI for conversations
  http-api    HTTP API for conversations (not installed)

Run `jp serve <command> -h` for more information.
```

The "(not installed)" marker signals that the subcommand is available but not
yet downloaded. Running `jp serve http-api` triggers a standard auto-install
flow to be defined in a future RFD.

When a group is invoked without a subcommand (`jp serve`), JP prints the
help text and exits with code 2, matching the behavior of built-in command
groups like `jp conversation`.

A real plugin can also have sub-plugins beneath it. When `jp serve -h` is
requested and `jp-serve` is a real binary, JP sends `describe` to it *and*
checks the registry for plugins whose command path extends `["serve", ...]`.
Both the plugin's own help text and the discovered sub-plugins are merged
into the output.

#### Plugin Tracing

Plugins can send structured log messages at any level through the protocol.
JP re-emits these as tracing events under the `plugin` target at the specified
level, making them visible in `jp -v` output and the trace log file.

For Rust plugins, this is best implemented as a custom `tracing::Layer` that
serializes events as `PluginToHost::Log` messages on stdout. The layer can
buffer events during startup and flush them once the protocol writer is
available. Use `try_lock` on the shared stdout writer to avoid deadlocking
when a tracing event fires while the writer is already held.

#### Workspace Queries

**List conversations:**

```json
{
  "type": "list_conversations"
}
```

Response:

```json
{
  "type": "conversations",
  "data": [
    {
      "id": "17127583920",
      "title": "Refactor config",
      "last_activated_at": "2025-07-20T10:30:00Z",
      "events_count": 42
    }
  ]
}
```

**Read conversation events:**

```json
{
  "type": "read_events",
  "conversation": "17127583920"
}
```

Response:

```json
{
  "type": "events",
  "conversation": "17127583920",
  "data": [
    {
      "timestamp": "...",
      "type": "chat_request",
      "content": "..."
    },
    {
      "timestamp": "...",
      "type": "chat_response",
      "message": "..."
    }
  ]
}
```

The events use the same JSON format as on-disk storage (the `ConversationEvent`
serialization). The host decodes base64-encoded storage fields (tool call
arguments, tool response content, metadata) to plain text before sending, so
plugins receive human-readable values and do not need to handle base64
themselves.

**Read config:**

```json
{
  "type": "read_config"
}
```

Response:

```json
{
  "type": "config",
  "data": {
    "server": {
      "web": {
        "bind": "127.0.0.1",
        "port": 3141
      }
    },
    "assistant": {
      "name": "JP"
    },
    "...": {}
  }
}
```

This returns the full resolved config. It is equivalent to the `config` field
in the `init` message but can be re-requested if the plugin needs it later.

A `path` field can narrow the response to a subtree of the config:

```json
{
  "type": "read_config",
  "path": "assistant.model"
}
```

Response:

```json
{
  "type": "config",
  "path": "assistant.model",
  "data": {
    "id": {
      "provider": "anthropic",
      "name": "claude-sonnet-4-20250514"
    },
    "parameters": {
      "max_tokens": 8192
    }
  }
}
```

The `path` syntax uses the same dot-separated keys as the `--cfg` CLI flag
(e.g., `assistant.model`, `server.web.port`, `conversation.tools`). An invalid
path returns an error.

#### Workspace Mutations

**Lock a conversation:**

```json
{
  "type": "lock",
  "conversation": "17127583920"
}
```

Response (success):

```json
{
  "type": "locked",
  "conversation": "17127583920"
}
```

Response (already locked by another process):

```json
{
  "type": "error",
  "request": "lock",
  "conversation": "17127583920",
  "message": "conversation is locked by another process"
}
```

JP acquires the flock on behalf of the plugin and tracks it internally. The
lock is released when the plugin sends `unlock`, sends `exit`, or the process
terminates.

**Push events to a locked conversation:**

```json
{
  "type": "push_events",
  "conversation": "17127583920",
  "events": [
    {
      "type": "turn_start"
    },
    {
      "type": "chat_request",
      "content": "Hello"
    }
  ]
}
```

Response:

```json
{
  "type": "pushed",
  "conversation": "17127583920",
  "count": 2
}
```

The conversation must be locked by this plugin. JP validates the events before
appending them to the stream. Validation includes:

- Every `ToolCallResponse` must reference an existing `ToolCallRequest` ID.
- Every `InquiryResponse` must reference an existing `InquiryRequest` ID.
- A `ChatRequest` must be preceded by a `TurnStart` (JP injects one if the
  push batch starts with a `ChatRequest` and no turn is active).
- Event types must be well-formed (required fields present, correct types).

If validation fails, the entire push is rejected — no partial writes. The
response is an error with details about which event failed:

```json
{
  "type": "error",
  "request": "push_events",
  "message": "ToolCallResponse references unknown request ID `tc_99`"
}
```

**Unlock a conversation:**

```json
{
  "type": "unlock",
  "conversation": "17127583920"
}
```

Response:

```json
{
  "type": "unlocked",
  "conversation": "17127583920"
}
```

**Create a conversation:**

```json
{
  "type": "create_conversation",
  "title": "Web chat session"
}
```

Response:

```json
{
  "type": "created",
  "conversation": "17127583921"
}
```

The new conversation is created and automatically locked by the plugin.

#### Output

All user-facing output goes through the protocol. JP routes it through the
`Printer`, which respects `--quiet`, `--format json`, and other output modes.

**Print command:**

```json
{
  "type": "print",
  "channel": "content",
  "format": "markdown",
  "text": "## Results\n\n- item 1\n- item 2\n"
}
```

The `channel` field specifies the output category, controlling filtering and
semantic treatment. The `format` field specifies how JP should render the text.
Both are optional.

**Channels** (default: `content`):

| Channel       | Purpose                                            |
|---------------|----------------------------------------------------|
| `content`     | Primary output (assistant messages, results)       |
| `chrome`      | UI decorations (headers, separators, progress)     |
| `tool_call`   | Tool call names and arguments                      |
| `tool_result` | Tool call results                                  |
| `reasoning`   | Model reasoning/thinking content                   |
| `error`       | Error messages                                     |

**Formats** (default: `plain`):

| Format     | Rendering                                              |
|------------|--------------------------------------------------------|
| `plain`    | Pass through as-is                                     |
| `markdown` | Render via `jp_md::Buffer` with theme/width config     |
| `json`     | Pretty-print and syntax-highlight                      |
| `code`     | Syntax-highlight with optional `language` field        |

For `code`, a `language` hint can be provided:

```json
{
  "type": "print",
  "channel": "content",
  "format": "code",
  "language": "rust",
  "text": "fn main() {}"
}
```

The simplest case remains simple — a shell script can send
`{"type":"print","text":"hello\n"}` and it works.

**Structured log message:**

```json
{
  "type": "log",
  "level": "info",
  "message": "Web server listening",
  "fields": {
    "addr": "127.0.0.1:3141"
  }
}
```

JP emits this as a tracing event at the specified level, attributed to the
plugin. Valid levels: `trace`, `debug`, `info`, `warn`, `error`.

#### Error Handling

Any request can return an error:

```json
{
  "type": "error",
  "request": "read_events",
  "message": "conversation not found: 999"
}
```

The `request` field echoes the type of the failed request so the plugin can
correlate errors with requests.

### Shell Script Example

A plugin that prints all conversation titles:

```bash
#!/bin/bash
# jp-titles: list conversation titles

# Read first message from host.
read -r msg
type=$(echo "$msg" | jq -r '.type')

# Handle describe request.
if [ "$type" = "describe" ]; then
    echo '{"type":"describe","name":"titles","version":"0.1.0","description":"List conversation titles","command":["titles"]}'
    exit 0
fi

# It's an init message. Signal ready.
echo '{"type":"ready"}'

# Request conversation list.
echo '{"type":"list_conversations"}'
read -r response

# Print each title.
for title in $(echo "$response" | jq -r '.data[].title'); do
    echo "{\"type\":\"print\",\"text\":\"$title\n\"}"
done

# Exit cleanly.
echo '{"type":"exit","code":0}'
```

### Web Server Example

The web server plugin (`jp-serve`) uses the protocol for data access but
manages its own HTTP listener:

1. Receives `init`, extracts config for bind address and port.
2. Sends `ready`.
3. Starts an HTTP server (axum, actix, whatever).
4. On each page request, sends `list_conversations` or `read_events` over the
   protocol and renders the response as HTML.
5. On `shutdown`, stops accepting connections, finishes in-flight requests,
   sends `exit`.

A future chat interface would need a subscription and query delegation
mechanisms to stream LLM responses to the browser and handle tool approval
prompts.

### Plugin Registry

The registry is a JSON file served from `https://jp.computer/plugins.json`:

```json
{
  "version": 1,
  "plugins": {
    "serve": {
      "id": "serve",
      "kind": "command_group",
      "description": "JP server components",
      "official": true,
      "suggests": [
        "serve web",
        "serve http-api"
      ]
    },
    "serve web": {
      "id": "serve-web",
      "description": "Read-only web UI for browsing conversations",
      "official": true,
      "requires": [
        "serve"
      ],
      "repository": "https://github.com/dcdpr/jp",
      "binaries": {
        "aarch64-apple-darwin": {
          "url": "https://...",
          "sha256": "..."
        }
      }
    }
  }
}
```

Registry keys are space-separated command paths. `"serve web"` corresponds
to `jp serve web`. Each key is unique by construction (JSON object keys),
which guarantees that no two plugins can claim the same subcommand.

The `kind` field identifies the entry type:

| Kind            | Description                                              |
|-----------------|----------------------------------------------------------|
| `command`       | A standalone binary using the JSON-lines protocol.       |
|                 | Default when absent, so older registry entries remain    |
|                 | valid.                                                   |
| `command_group` | A command namespace with no binary. Provides help text   |
|                 | and lists sub-plugins via `suggests`. `jp <group>`       |
|                 | prints help and exits with code 2.                       |

Future plugin types (e.g. `"wasm"` from [RFD 016]) will use additional
values. JP ignores entries with unrecognized `kind` values.

**`id`** (required) — Stable identifier used for binary naming, config keys,
and install paths. The binary is `jp-{id}`, config lives at
`plugins.command.{id}`, and the install path is
`$XDG_DATA_HOME/jp/plugins/command/jp-{id}`. The `id` must be unique across
all registry entries.

**`requires`** — Command paths (registry keys) of plugins that must be
installed for this one to work. When JP installs a plugin, it first installs
all required dependencies (with their own trust policy — each dependency is
evaluated independently, not inherited from the requesting plugin). If a
required dependency is denied, the requesting plugin is not installed either.

**`suggests`** — Command paths (registry keys) of plugins that extend this
one. Used for help aggregation: `jp serve -h` shows suggested sub-plugins as
available subcommands, with an "(not installed)" marker for those not yet
downloaded. Suggested plugins are not installed automatically.

JP caches the registry locally at `$XDG_DATA_HOME/jp/registry.json` and
refreshes it on `jp plugin update`. Binary checksums are validated after
download before the binary is made executable. Installed binaries are stored
at `$XDG_DATA_HOME/jp/plugins/command/jp-{id}`, keeping them separate from
`$PATH` and leaving room for other plugin types.

### Plugin Trust and Configuration

Plugin installation, execution policy, checksum pinning, and per-plugin options
are controlled through the `[plugins]` section of `AppConfig`. This will be
defined in a future RFD, which supersedes the approval model originally proposed
here.

In summary:

- Each plugin has a `run` policy: `ask` (prompt), `unattended` (silent),
  or `deny` (block). Official registry plugins default to `unattended`;
  PATH-discovered plugins default to `ask`.
- A `checksum` field pins the binary to a specific hash. JP refuses to run
  a binary whose checksum doesn't match the pinned value.
- An `options` field passes opaque configuration to the plugin via the
  `init` message.
- All plugin config participates in the standard config inheritance chain
  (global → workspace → local → CLI overrides).

A future RFD will define the full configuration schema and trust model.

## Drawbacks

- **Latency**: Every workspace operation requires a JSON round-trip over a
  pipe. For human-interactive use cases (web pages, CLI output) this is
  negligible. For batch processing of thousands of conversations, it would be
  noticeable. This can be mitigated later with bulk operations or a binary
  protocol.

- **Protocol maintenance**: The protocol is a public API surface that must be
  versioned and maintained. Adding new operations is straightforward (additive
  change), but changing existing message formats requires care.

- **No shared memory**: Plugins cannot access JP's in-memory data structures
  directly. Every piece of data must be serialized and sent over the pipe. For
  the conversation events that are already stored as JSON, this is natural. For
  complex types like `AppConfig`, the serialization must be complete and
  stable.

- **Two binaries for the web server**: Users who previously had a single `jp`
  binary now need `jp` plus `jp-serve`. The auto-install mechanism mitigates
  this, but it adds moving parts.

## Alternatives

### Cargo-style thin dispatch (exec and forget)

JP sets environment variables (`JP_WORKSPACE_ROOT`, `JP_STORAGE_DIR`, etc.)
and execs the plugin. The plugin opens the workspace itself using `jp_workspace`
as a library dependency.

Rejected because:

- Plugins must be Rust (or FFI into Rust crates) to use the workspace safely.
- Every plugin re-implements bootstrap logic and gets it wrong.
- No way for a shell script to access conversations.
- No lock management — plugins hold flocks directly, making crash recovery
  harder.
- Switching from read-only to read-write requires architectural changes in the
  plugin.

### Feature-gated built-in commands

Keep plugins as built-in commands behind cargo feature flags. Users compile with
`--features web` to include the web server.

Rejected because:

- Not extensible at runtime. Third parties cannot add commands.
- Users must compile from source to choose features.
- Does not establish a plugin pattern for the ecosystem.

### Wasm plugin model ([RFD 016])

Use the Wasm component model for command plugins.

Not rejected, but not suitable for this use case. Wasm plugins are sandboxed
in-process components for capability extensions (attachment handlers, tools).
External command plugins are long-running processes that need direct network
access (web servers), filesystem access (exporters), or terminal control
(TUI dashboards). The two systems are complementary: Wasm for fine-grained
capabilities, external commands for coarse-grained extensions.

## Non-Goals

- **In-process plugin loading**: Shared library (`.so`/`.dylib`) plugins are
  not in scope. The process boundary provides isolation and language
  independence.
- **Plugin authoring SDK**: A Rust crate that wraps the protocol into a
  convenient API is future work. The protocol is simple enough that early
  plugins can be written against it directly.
- **Event subscriptions and query delegation**: Live event streaming, agent loop
  delegation, and interactive events (tool approval, inquiries) will be defined
  in a future extending RFD.
- **Plugin-to-plugin communication**: Plugins communicate with JP, not with
  each other.

## Risks and Open Questions

- **Config serialization completeness**: `AppConfig` contains custom types
  (`ModelIdConfig`, `ToolConfig`, etc.) with complex serialization. The
  protocol sends the full config as JSON, which must faithfully represent all
  fields a plugin might need. The existing `serde` implementations should
  cover this, but edge cases (e.g., enum variants with custom serializers)
  need testing.

- **Event format stability**: The protocol exposes `ConversationEvent` JSON as
  a public API. Changes to the event schema (new fields, renamed types) become
  breaking changes for plugins. This is already partially true for on-disk
  compatibility, but the plugin protocol makes it explicit.

- **Conversation write validation**: When a plugin pushes events, JP must
  validate them (e.g., `ToolCallResponse` must have a matching
  `ToolCallRequest`). The existing `ConversationStream::sanitize` logic
  handles some of this, but the validation boundary for external writers needs
  to be clearly defined.

- **Protocol evolution**: The `version` field in `init` provides basic
  versioning, but the strategy for handling version mismatches (plugin wants
  v2, JP only speaks v1) needs to be defined. The simplest approach: JP
  refuses to run plugins that require a higher version than it supports, and
  plugins must handle missing optional fields gracefully.

- **Registry trust model**: Auto-installing official plugins requires trusting
  the registry file and the download URLs. The checksum validation protects
  against tampering in transit. The registry itself is served from
  `https://jp.computer/plugins.json`
  over HTTPS. The registry URL is hardcoded in the JP binary.

## Implementation Plan

### Phase 1: Protocol core and dispatcher

- Define the protocol message types in a new `jp_plugin` crate.
- Implement the parent-side message loop in JP: spawn child, send `init`,
  relay requests to `Workspace` methods, capture stderr to tracing.
- Implement unknown-subcommand dispatch: search `$PATH` for `jp-<name>`.
- Test with a minimal shell script plugin.
- Can be merged independently.

### Phase 2: Web server as external plugin

- Extract `jp-serve` into a standalone binary crate (`crates/jp_serve/`).
- Implement the plugin-side protocol client (reads init, sends requests,
  renders responses).
- Remove `jp serve` as a built-in command; it becomes a plugin dispatch.
- Remove `jp_web` dependency from `jp_cli`.
- Depends on Phase 1.

### Phase 3: Plugin registry and auto-install

- Define the registry JSON format.
- Implement registry fetch, caching, and binary download with checksum
  validation.
- Implement the install flow (silent for official, prompted for third-party).
- Add `jp plugin list`, `jp plugin install`, `jp plugin update` subcommands.
- Depends on Phase 1. Independent of Phase 2.

### Phase 4: Write operations

- Add `lock`, `unlock`, `push_events`, and `create_conversation` to the
  protocol.
- Implement lock tracking in JP's dispatcher (release on plugin exit/crash).
- Implement event validation for externally pushed events.
- Depends on Phase 1.

### Phase 5: Command routing and plugin dependencies

- Use the `command` field from `Describe` and the registry keys for routing
  instead of relying solely on binary name conventions.
- Cache `describe` responses to avoid spawning plugins repeatedly for
  `jp -h` and routing.
- Implement `requires` / `suggests` in the registry and install flow.
- Update `jp plugin install` to resolve and install required dependencies.
- Implement `command_group` registry entries and help aggregation.
- Update help rendering to merge suggested sub-plugins (installed and
  uninstalled) into parent plugin help output.
- Depends on Phase 3 (registry).

## References

- [RFD 016: Wasm Plugin Architecture][RFD 016]
- [RFD 026: Agent Loop Extraction][RFD 026]
- [RFD 027: Client-Server Query Architecture][RFD 027]
- [Cargo external tools documentation][cargo-external]
- [Git remote helpers protocol][git-remote-helpers]

[RFD 016]: 016-wasm-plugin-architecture.md
[RFD 026]: 026-agent-loop-extraction.md
[RFD 027]: 027-client-server-query-architecture.md
[cargo-external]: https://doc.rust-lang.org/cargo/reference/external-tools.html#custom-subcommands
[git-remote-helpers]: https://git-scm.com/docs/gitremote-helpers
