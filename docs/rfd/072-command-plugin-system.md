# RFD 072: Command Plugin System

- **Status**: Implemented
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-04-06
- **Extended by**: [RFD 114]
- **Summary**: Extends JP with standalone `jp-<name>` command plugin binaries
  that communicate with the host over a JSON-lines protocol.

## Summary

This RFD introduces a command plugin system where standalone binaries
(`jp-<name>`) extend JP with new subcommands over a JSON-lines protocol on stdin
and stdout.
JP handles workspace discovery, configuration loading, conversation locking,
output formatting, and signal management, so plugins can be written in any
language.
Unlike the in-process Wasm plugins in [RFD 016], command plugins run as separate
processes that own subcommand execution.

## Motivation

JP is growing beyond its core query loop.
The web UI experiment is the first example: a long-running server that reads
(and will soon write) conversation data.
Future candidates include HTTP APIs, TUI dashboards, import/export tools, and
IDE integrations.

Compiling these extensions into the `jp` binary has three costs:

- **Binary size**: The web server pulls in axum, hyper, tower, and maud.
  Every user pays this cost whether they use the web UI or not.
- **Coupling**: Every extension must be written in Rust, link against JP's
  internal crates, and wire into the `Commands` enum and startup pipeline.
- **Release cadence**: A bug fix in the web UI requires a full JP release.

A plugin system avoids these costs, but it cannot use Cargo's "exec the binary"
dispatch.
Plugins need services from JP's startup pipeline: workspace discovery, config
loading, conversation locking, and structured output.
Pushing those responsibilities into plugins forces every plugin author to
re-implement JP's bootstrap, which is error-prone (the web server already hit
this by omitting `.with_local_storage()`).

The goal is a plugin system where:

1. JP remains the orchestrator: it finds the workspace, loads config, manages
   locks, and formats output.
2. Plugins are standalone executables written in any language, including shell
   scripts.
3. A plugin that starts read-only (such as a web viewer) can gain write access
   (such as chat) without changing its architecture.

## Design

### User Experience

Plugins are invoked as JP subcommands, either at the root or under a command
group provided by a plugin or the registry:

```txt
jp serve                        # runs jp-serve plugin
jp serve web                    # runs jp-serve-web plugin
jp export --format html         # runs jp-export plugin
jp dashboard                    # runs jp-dashboard plugin
```

Built-in command groups do not accept plugin subcommands: `jp conversation
export` is an unknown subcommand on a built-in group, not a plugin.
Opening built-in groups to plugins is future work (see [Non-Goals](#non-goals)).

Each plugin embeds a manifest in its binary: a single line of JSON that declares
the command path the plugin provides, which the host reads without running the
binary (see [Plugin Manifest](#plugin-manifest)).
A plugin whose manifest claims `["serve", "web"]` handles `jp serve web`
regardless of its binary name.
The `jp-` prefix on the binary name marks the file as a plugin, and the name
after `jp-` keys the plugin's configuration (see [Plugin
Identity](#plugin-identity)).

JP checks built-in commands first, so a plugin cannot shadow a built-in command
at any level.

Official plugins (published by the JP project) ship separately from `jp` but
install automatically the first time the user runs their command.
Third-party plugins are installed only on request, similar to `cargo install`
for `cargo-*` subcommands:

```txt
$ jp foo bar
error: unrecognized subcommand 'foo'

  `jp foo` is provided by the third-party plugin `foo`
  (https://github.com/acme/jp-foo). Install it with `jp plugin install foo`.

$ jp plugin install foo
  → jp-foo 0.3.0, from https://github.com/acme/jp-foo/releases/...
Install it? [y/N] y

$ jp foo bar
```

Users can also install a plugin by placing a `jp-<name>` binary with a manifest
on `$PATH`; JP prompts for approval before the first run ([RFD 077]).

When JP encounters an unknown subcommand, it:

1. Collects command claims from three sources: manifests in `jp-*` binaries in
   the user-local install directory (`$XDG_DATA_HOME/jp/plugins/command/`) and
   on `$PATH`, approval records for binaries without a readable manifest, and
   official entries in the cached registry.
   If no source claims the command and the registry cache is missing or older
   than a day, JP fetches the registry once and collects claims again.
2. Selects the longest claimed command path that matches the CLI arguments,
   following the precedence rules in [Phase 5](#implementation-plan).
   The remaining arguments are passed to the selected plugin.
3. Downloads the binary and verifies its checksum against the registry if the
   winning claim is an uninstalled official plugin.
4. Admits the binary against the `run` policy, any pinned checksum, and the
   approval store ([RFD 077]).
5. Spawns the plugin binary and communicates over the protocol.

Third-party registry entries do not claim commands during routing.
They serve as catalog entries for `jp plugin list`, `jp plugin install`, and the
unrecognized-subcommand install hint.

Setting `JP_NO_PLUGIN_DOWNLOAD=1` disables automatic network requests for
plugins, including registry fetches and first-use downloads of official plugins.
Explicit commands (`jp plugin list`, `install`, and `update`) still use the
network.
System package managers can ship official plugins by placing their binaries on
`$PATH`, which JP uses instead of downloading.

Plugin management commands (`jp plugin list`, `install`, `uninstall`, `update`,
`approve`, and `revoke`) form a built-in subcommand group that runs from any
directory without a workspace.
Because `jp plugin` is built in, it takes priority over any `jp-plugin` binary
on `$PATH`.

### Channel Model

JP spawns the plugin as a child process with three channels:

| Channel    | Direction   | Purpose                                 |
| ---------- | ----------- | --------------------------------------- |
| **stdin**  | JP → plugin | Protocol messages: init, responses      |
| **stdout** | plugin → JP | Protocol messages: requests, print, log |
| **stderr** | plugin → JP | Captured and forwarded to JP's tracing  |
|            |             | subsystem at `trace` level              |

The plugin never writes directly to the terminal.
All user-facing output is sent as `print` protocol messages, which JP routes
through its printer so `--quiet` and `--format` apply consistently (see
[Output](#output)).

JP captures stderr line by line and emits each line as a `trace`-level event
attributed to the plugin.
Plugin authors can write diagnostics to stderr (`fprintf(stderr, ...)` in C,
`eprintln!()` in Rust, or `echo >&2` in shell) without corrupting the protocol
stream or polluting user output.

### Protocol

The protocol uses JSON-lines over stdin and stdout: one JSON object per line,
delimited by newlines, with a `type` field on every message.

#### Stdout Hygiene

Because stdout carries protocol messages, any stray output on stdout (such as a
`printf` in a C library or an uncaught panic message) corrupts the JSON-lines
stream.
LSP, MCP, and git remote helpers share this trade-off.

To avoid corrupting stdout, plugins write all diagnostic output to stderr
(`eprintln!()` in Rust, `echo >&2` in shell, `fprintf(stderr, ...)` in C), which
JP forwards to tracing.
Stdout is reserved exclusively for protocol messages.

Stdin and stdout need no socket setup or file descriptor passing, and shell
scripts can speak the protocol with `echo` and `read`.
Future protocol versions could add an alternative transport (such as FD 3/4 or
domain sockets) if stdout pollution becomes a problem in practice.

#### Request IDs

Plugin-to-JP requests may include an optional string `id` field, which JP echoes
in the corresponding response.
Multi-threaded plugins use `id` to correlate responses with concurrent requests.

Synchronous plugins (such as shell scripts and single-threaded tools) can omit
`id` on synchronous requests.
JP processes requests in order and replies to immediate requests in that same
order, so correlation is implicit.

`query` and `interrupt` complete asynchronously on the turn they target, so
their responses can arrive out of order relative to later requests.
A plugin sending `query` or `interrupt` must set `id`; JP executes an
`interrupt` without an `id` but sends no response.

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
The `log_level` field conveys the host's verbosity (0 = error, 1 = warn, 2 =
info, 3 = debug, 4 = trace) so plugins can configure their own tracing to match
the user's `-v` flags.

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

JP waits for any turn the plugin delegated to finish, then exits with the given
code.
If the plugin process exits without sending `exit` (for example, on a crash or
signal), JP detects EOF on stdout, waits for any delegated turn to finish, and
exits with code 1.

A delegated turn outlives the plugin that requested it.
Its conversation stays locked until the turn ends, and the host waits for it
without a timeout because the turn performs user work.
The user can stop a running turn early through the interrupt ladder.

#### Shutdown

When JP receives a signal (SIGINT, SIGTERM), it sends:

```json
{
  "type": "shutdown"
}
```

The plugin begins graceful shutdown and sends `exit` when done.
If the plugin does not exit within a configurable grace period (default 5
seconds), JP terminates it.

JP owns the plugin's entire process tree for the lifetime of the invocation, not
just the spawned child process.
Termination kills the plugin's process group on Unix and its job object on
Windows, so background workers started by a shell-script plugin cannot outlive
the plugin, hold a port open, or keep the stdout pipe open.
Descendants that deliberately detach from the process group are unsupported.

JP spawns the plugin in its own process group (`process_group(0)` on Unix) so OS
signals (SIGINT, SIGTERM) reach only the host, which relays them as `shutdown`
protocol messages rather than letting the OS kill the child directly.

#### Plugin Manifest

Every plugin binary embeds a manifest: a single line of JSON that the host reads
without executing the binary.

```txt
jp-plugin/v1 {"protocol":1,"description":"Read-only web UI for browsing conversations","command":["serve","web"]}
```

The host scans the binary for `jp-plugin/v` followed by a version number and a
space, then parses the JSON up to the first newline or NUL byte.
Where the line sits in the file depends on how the plugin is built:

- Compiled plugins embed the manifest as a referenced string constant so the
  linker retains it.
  In Rust, a single `jp_plugin` declaration generates both the manifest constant
  and the matching `describe` response.
- Scripts place the manifest in a comment on any line after the shebang, such as
  `# jp-plugin/v1 {...}`.
  The host ignores any text before the `jp-plugin/v` marker on that line.

The manifest contains only the metadata the host needs before deciding to run
the plugin:

- **`protocol`** (required): The minimum host protocol version the plugin
  requires, matching the version it reports in `ready`.
  A host that supports only an older protocol version rejects the plugin before
  spawning it.
- **`description`** (required): A one-line summary shown in `jp -h` and in the
  approval prompt.
- **`command`** (required): The command path the plugin claims.
  `["serve", "web"]` claims `jp serve web` regardless of the binary's filename,
  and path segments may contain hyphens.

New manifest claims are added as the protocol evolves.
Claims that older hosts can safely ignore are added as optional JSON keys, and
hosts ignore unrecognized keys.
Claims that change host dispatch semantics, such as [RFD 114]'s
`workspace_scope: "multi"`, require a higher `protocol` version so older hosts
reject the plugin instead of misinterpreting it.
The `v1` marker version changes only if the framing changes or an existing field
is removed or redefined; a host that encounters an unknown marker version
reports that the plugin requires a newer `jp`.

A manifest is valid when:

- The marker appears exactly once in the file, preventing conflicting manifests
  within the same binary.
- The JSON payload is at most 64 KiB, is valid UTF-8, and parses as JSON.
- No string value contains control characters other than `\n` and `\t`,
  preventing terminal escape injection when `jp -h` or the approval prompt
  prints the description of an unapproved binary.

A binary without a valid manifest claims no commands.
`jp -h` and `jp plugin list` list it by filename along with the validation
error.
To route commands to a binary without a readable manifest (such as a binary
compressed with UPX), the user runs `jp plugin approve <path>` ([RFD 077]),
which runs `describe` and stores the returned manifest fields in the approval
record.

#### Plugin Identity

A plugin's name is its filename without the `jp-` prefix (and without `.exe` on
Windows).
Plugins installed from the registry are written as `jp-{id}`, so their plugin
name matches their registry `id`.
The plugin name keys configuration (`plugins.command.<name>`), approval records,
`jp plugin uninstall <name>`, and `jp plugin revoke <name>`.
The claimed command path is never used as an identity key: the plugin asserts it
in its manifest, it can change across versions, and a single plugin may claim
multiple commands in the future.

A plugin is official when its name matches the `id` of an official registry
entry, whether JP downloaded the binary or a system package manager placed it on
`$PATH`.
Every other binary is third-party, regardless of the command path it claims.

Two binaries with the same plugin name conflict regardless of what commands they
claim, because they would share the same `run` policy, pinned checksum, options,
and approval record.
When a name conflict occurs, the host reports both paths and runs neither.
Renaming one of the files resolves the conflict because routing uses the
manifest rather than the filename; if one of the copies was installed by JP, `jp
plugin uninstall` removes it.
Multiple paths that resolve to the same underlying file (such as symlinks into a
package store) count as a single binary.

#### Tool Calls

A command plugin can also provide tools to the assistant.
The tool configuration references the plugin in `source`:

```toml
[conversation.tools.ticket_create]
source = "plugin.command.ticket.create"
summary = "File a work item as a ticket."

[conversation.tools.ticket_create.parameters.title]
type = "string"
required = true
```

`plugin.command.<plugin>` uses the configured tool name as the tool identifier,
whereas `plugin.command.<plugin>.<tool>` specifies the tool name the plugin
expects.
The `command` segment identifies the plugin kind, matching the
`plugins.command.<plugin>` configuration path.
Tool parameters are declared in configuration, just as for `local` tools.

For each tool execution attempt, JP spawns the plugin and sends an `init`
message with a `tool` payload and an empty `args` list:

```json
{
  "type": "init",
  "version": 12,
  "workspace": { "root": "/path/to/project", "storage": "/path/to/project/.jp", "id": "a1b2c" },
  "config": {},
  "options": { "dir": "packages/foo/tickets" },
  "args": [],
  "log_level": 0,
  "output_format": "text",
  "tool": {
    "action": "run",
    "name": "create",
    "arguments": { "title": "Fix the header" },
    "answers": {},
    "options": {},
    "access": { "fs": [], "net": [], "env": [{ "name": "AWS_*", "read": false }] },
    "conversation": "jp-c17000000000"
  }
}
```

`config` and `options` contain the query's resolved configuration (including
nested `.jp.toml` files, conversation config, and `--cfg` overrides), not the
workspace root's base config.
`tool.options` contains `conversation.tools.<name>.options`.

`tool.action` is either `run` or, when the tool uses `style.parameters =
"tool"`, `format_arguments`.
A `format_arguments` call asks the plugin to render the tool call for display
without executing it; JP prints the `content` of a `success` outcome verbatim
above the tool call.
`format_arguments` runs under the same admission, options, and access policy as
`run`, and the tool's `format` setting controls whether it runs before or after
user approval, matching custom formatter commands.

The plugin responds with a single `tool_outcome` message followed by `exit`:

```json
{ "type": "tool_outcome", "outcome": { "type": "success", "content": "Created T-0abc123" } }
```

`outcome` uses the same schema that a `local` tool writes to stdout: `success`,
`error`, or `needs_input`.
The plugin must follow `tool_outcome` with `exit`; if the plugin exits without
sending `exit`, the call fails so a post-outcome crash is not treated as a clean
completion.
Returning `needs_input` ends the current attempt.
Once JP collects the user's answer, it spawns the plugin again with the
accumulated answers in `tool.answers`.

Each tool attempt is a one-shot invocation, like a `local` tool command.
JP writes `init` to the plugin's stdin and closes the stream, so the plugin
cannot send host requests during a tool call; `init` carries all required
context, and the active turn owns the terminal.
JP ignores `print` messages during tool execution and uses only `tool_outcome`.
Cancelling a tool call kills the plugin and (on Unix) its process group,
matching `local` tool cancellation.
Tool calls require protocol version 12.

Plugin tools follow the same execution policies as `local` tools.
The tool's `run`, `result`, and `format` settings control whether JP prompts for
approval, allows result editing, or skips execution.
The host compiles the tool's `access` grants (including grants inherited from
`conversation.tools.'*'.access` and added by `--mount`) into `tool.access`, and
the plugin enforces them just as a local tool binary does.

Before any tool runs, JP admits the plugin binary under
`plugins.command.<plugin>.run`, using the same admission rules as `jp <plugin>`.
Admission runs once per plugin at the start of a turn so the user is never
prompted to approve a binary mid-turn.
Selecting "run this time" admits the binary for the duration of the turn.
When no terminal is attached, any binary that requires an approval prompt is
rejected, matching `jp <plugin>`.
Turn start never downloads plugins: if an official plugin binary is missing, JP
rejects it until the user installs it with `jp plugin install` or runs `jp
<plugin>` once.

Admission is bound to the binary's SHA-256 digest at turn start.
Before each tool invocation, the host re-hashes the binary and rejects the call
if the file changed on disk since the turn started.

If a plugin is not admitted, JP omits its tools from the turn and prints the
omitted tool names and reason to stderr; explicitly requesting one of those
tools with `--tool` fails the query.

#### Plugin Self-Description

An admitted plugin can describe its full metadata over the protocol for `jp
<plugin> -h` and `jp plugin approve`.

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
  "protocol": 1,
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

The response includes every manifest field with the same meaning, plus:

- **`name`**, **`version`** (required).
- **`help`** (required): Rendered for `jp <plugin> -h`.
  Because the plugin computes `help` at runtime, it can generate the text from
  its own CLI parser definition.
- **`author`**, **`repository`** (optional).

The embedded manifest and the `describe` response must agree.
Whenever the host has both, it compares them and reports any mismatch in
`protocol` or `command` as an error naming the plugin.
For a binary without an embedded manifest, the host compares the `describe`
response against the manifest fields stored in its approval record.

A future RFD can add structured help metadata so the host can format plugin help
through `clap`.

When a user invokes a plugin binary directly from the shell rather than through
`jp`, the binary should detect that stdin is a TTY, print its help text to
stderr, and exit.

JP admits a plugin binary (checking the `run` policy, any pinned checksum, and
`$PATH` binary approval per [RFD 077]) before every spawn, including `describe`.
Spawning an unapproved binary just to send `describe` is unsafe because the
executable can run arbitrary code before reading stdin.

`jp -h` reads manifests from disk and never spawns plugin binaries.
It lists each plugin under its claimed command path with its manifest
description in a "Plugins:" section below the built-in commands.
Binaries without a valid manifest are listed using the metadata stored in their
approval record, or by filename if unapproved.
Uninstalled official plugins are listed from the registry cache so they appear
in `jp -h` on a fresh installation; if no cache exists, `jp -h` fetches the
registry unless `JP_NO_PLUGIN_DOWNLOAD` is set.
Running `jp <plugin> -h` performs normal admission (prompting when the policy is
`ask`) and spawns the plugin with `describe` to retrieve its full `help` text.

#### Help Aggregation for Command Groups

The registry supports `command_group` entries: command namespaces that have no
binary.
A group provides help text and lists sub-plugins without executing any code.
When the user runs `jp serve -h` and `serve` is a command group, JP:

1. Reads the group's `description` and `suggests` list from the registry.
2. Scans local plugin manifests for claims under `["serve", ...]`.
3. Checks the registry for uninstalled plugins under the same prefix.
4. Merges the results into the help output:

<!-- end list -->

```txt
JP server components

Usage: jp serve <COMMAND>

Commands:
  web         Read-only web UI for conversations
  http-api    HTTP API for conversations (not installed)
  metrics     Prometheus exporter (third-party: `jp plugin install metrics`)

Run `jp serve <command> -h` for more information.
```

The `(not installed)` marker indicates that an official subcommand is available
in the registry and will be downloaded when the user runs `jp serve http-api`.
Third-party subcommands show the `jp plugin install` command needed to install
them.

Invoking a command group without a subcommand (`jp serve`) prints the help text
and exits with code 2, matching built-in command groups such as `jp
conversation`.

An executable plugin can also have sub-plugins beneath it.
When the user runs `jp serve -h` and a binary claims `serve`, JP admits the
binary, sends `describe` to retrieve its help text, and merges in any local or
registry sub-plugins whose command path extends `["serve", ...]`.

#### Plugin Tracing

Plugins can send structured log messages at any level over the protocol.
JP re-emits them as `tracing` events under the `plugin` target at the requested
level, so they appear in `jp -v` output and the trace log file.

Rust plugins can implement a `tracing::Layer` that serializes events as
`PluginToHost::Log` messages on stdout, buffering startup events until the
protocol writer is initialized.
The layer should acquire the shared stdout writer with `try_lock` to avoid
deadlocking if a tracing event fires while the writer lock is already held.

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
serialization).
The host decodes base64-encoded storage fields (tool call arguments, tool
response content, metadata) to plain text before sending, so plugins receive
human-readable values and do not need to handle base64 themselves.

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

This returns the full resolved config.
It is equivalent to the `config` field in the `init` message but can be
re-requested if the plugin needs it later.

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

The `path` syntax uses the same dot-separated keys as the `--cfg` CLI flag (for
example, `assistant.model`, `server.web.port`, `conversation.tools`).
An invalid path returns an error.

By default, the host serves the configuration it loaded at startup.
A long-running plugin can set `reload: true` to have the host reload
configuration from disk before responding:

```json
{
  "type": "read_config",
  "reload": true
}
```

Reloading repeats the startup configuration pass across every config file, its
`extends` chain, environment variables, and the invocation's `--cfg` arguments.
The newly loaded configuration is cached and used for this response and all
subsequent `read_config` requests, so plugins can poll for config changes
without knowing how configuration files are laid out.
If reloading fails, the host returns an `error` and retains the previous
configuration.
`reload` requires protocol version 10.

#### Workspace Mutations

Plugins mutate workspace state through task-level operations.
The host acquires the conversation lock inside each operation, so a plugin never
holds a lock across messages or appends raw events to a stream.

| Request                | Reply                                                                | Lock held                        |
| ---------------------- | -------------------------------------------------------------------- | -------------------------------- |
| `archive_conversation` | `done`                                                               | for the request                  |
| `set_title`            | `done`                                                               | for the request                  |
| `write_draft`          | `draft`, with `conflict` when refused                                | none; a revision check guards it |
| `query`                | `created` (new conversations only), then `query_complete` or `error` | until the turn ends              |
| `interrupt`            | `done` or `error`, once the turn has acted, when it carries an `id`  | the turn's own                   |

`query` delegates a turn to the host: JP locks the conversation, appends the
request, calls the LLM provider, executes requested tools, and persists the
resulting events, running the same turn pipeline as `jp query`.
Delegating the turn keeps provider credentials, the tool registry, and MCP
server connections inside the host.

The turn runs concurrently with the protocol message loop, which continues
serving other requests while the turn is active.
When `query` creates a new conversation, the host sends `created` with the new
conversation ID as soon as the conversation exists.
When the turn finishes, the host sends `query_complete` (or `error` if the turn
could not start or failed).
Because the conversation remains locked for the duration of the turn, a second
`query` on the same conversation is rejected as already locked; plugins interact
with an active turn through `interrupt`.

A `query` with a `schema` field requests structured output (matching `jp query
--schema`) and returns the parsed JSON value in `query_complete.data`.
A `query` with `new` can also set `expires_in` (such as `5m` or `1h`) to create
a temporary conversation, matching `jp query --tmp`.
Both features require protocol version 11.

See [Lifecycle](#lifecycle) for how JP handles an active turn when the plugin
exits.

#### Output

All user-facing output is sent over the protocol and routed through JP's
`Printer`.

**Print command:**

```json
{
  "type": "print",
  "channel": "content",
  "format": "markdown",
  "text": "## Results\n\n- item 1\n- item 2\n"
}
```

The optional `channel` field selects the output stream and filtering behavior,
and the optional `format` field selects how JP renders `text`.

**Channels** (default: `content`):

| Channel       | Purpose                                        |
| ------------- | ---------------------------------------------- |
| `content`     | Primary output (assistant messages, results)   |
| `chrome`      | UI decorations (headers, separators, progress) |
| `tool_call`   | Tool call names and arguments                  |
| `tool_result` | Tool call results                              |
| `reasoning`   | Model reasoning/thinking content               |
| `error`       | Error messages                                 |

**Formats** (default: `plain`):

| Format     | Rendering                                          |
| ---------- | -------------------------------------------------- |
| `plain`    | Pass through as-is                                 |
| `markdown` | Render via `jp_md::Buffer` with theme/width config |
| `json`     | Pretty-print and syntax-highlight                  |
| `code`     | Syntax-highlight with optional `language` field    |

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

In the simplest case, a shell script can send
`{"type":"print","text":"hello\n"}`.

**Output modes.** The host's output rules ([RFD 048]) determine where `print`
messages are written and how they are formatted:

- `content` writes to stdout.
  `chrome`, `tool_call`, `tool_result`, and `reasoning` write to stderr and are
  suppressed by `--quiet`.
  `error` writes to stderr and is never suppressed.
- In terminal output mode, `format` selects the renderer from the table above.
- Under `--format json`, terminal rendering is disabled.
  A `print` message with `format: "json"` is emitted as the raw JSON value in
  `text`; all other formats are wrapped in a `{"message": ...}` JSON object.
  Plugins that want to emit custom JSON under `--format json` can inspect
  `init.output_format` and set `format: "json"`.

For `jp --format json titles`:

```json
{"type":"print","text":"Refactor config\n"}
{"type":"print","format":"json","text":"{\"id\":\"17127583920\"}"}
```

emits on stdout:

```json
{"message":"Refactor config\n"}
{"id":"17127583920"}
```

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

JP emits this as a `tracing` event at the specified level, attributed to the
plugin.
Valid levels are `trace`, `debug`, `info`, `warn`, and `error`.

#### Error Handling

Any request can return an error:

```json
{
  "type": "error",
  "request": "read_events",
  "message": "conversation not found: 999"
}
```

The `request` field echoes the `type` of the failed request so the plugin can
correlate errors with requests.

### Shell Script Example

A plugin that prints all conversation titles:

```bash
#!/bin/bash
# jp-titles: list conversation titles
# jp-plugin/v1 {"protocol":1,"description":"List conversation titles","command":["titles"]}

# Read first message from host.
read -r msg
type=$(echo "$msg" | jq -r '.type')

# Handle describe request.
if [ "$type" = "describe" ]; then
    echo '{"type":"describe","protocol":1,"name":"titles","version":"0.1.0","description":"List conversation titles","command":["titles"],"help":"Usage: jp titles"}'
    exit 0
fi

# It's an init message. Signal ready.
echo '{"type":"ready"}'

# Request conversation list.
echo '{"type":"list_conversations"}'
read -r response

# Print each title. jq builds each message, so quotes and spaces in a title
# stay inside one valid JSON string.
echo "$response" | jq -c '.data[] | {type: "print", text: ((.title // "Untitled") + "\n")}'

# Exit cleanly.
echo '{"type":"exit","code":0}'
```

### Web Server Example

The web server plugin (`jp-serve`) uses the protocol for workspace data access
while managing its own HTTP listener:

1. Receives `init` and reads the bind address and port from `config`.
2. Sends `ready`.
3. Starts an HTTP server (such as `axum` or `actix-web`).
4. On each HTTP request, sends `list_conversations` or `read_events` over the
   protocol and renders the response as HTML.
5. On `shutdown`, stops accepting new connections, finishes in-flight HTTP
   requests, and sends `exit`.

An interactive chat UI builds on this flow by sending `query` and `interrupt`
and polling `read_events` while a turn runs; streaming live events to the plugin
requires the event subscriptions deferred in [Non-Goals](#non-goals).

### Plugin Registry

The registry is a JSON file served from `https://jp.computer/plugins.json`:

```json
{
  "version": 1,
  "plugins": {
    "serve": {
      "id": "serve",
      "type": "command_group",
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

Registry keys are space-separated command paths (`"serve web"` corresponds to
`jp serve web`).
Because JSON object keys are unique, no two registry entries can claim the same
command path.
Installed binaries outside the registry can still claim overlapping paths; the
host resolves conflicts using the rules in [Phase 5](#implementation-plan).

The `type` field identifies the entry's kind:

| Kind            | Description                                            |
| --------------- | ------------------------------------------------------ |
| `command`       | A standalone binary using the JSON-lines protocol.     |
|                 | Default when absent, so older registry entries remain  |
|                 | valid.                                                 |
| `command_group` | A command namespace with no binary. Provides help text |
|                 | and lists sub-plugins via `suggests`. `jp <group>`     |
|                 | prints help and exits with code 2.                     |

Future plugin types (such as `"wasm"` from [RFD 016]) will add new `type`
values.
JP ignores individual registry entries that fail to parse or have an
unrecognized `type`, and loads the remaining entries.

- **`id`** (required): Unique identifier across all registry entries, used for
  binary naming (`jp-{id}`), configuration keys (`plugins.command.{id}`), and
  install paths (`$XDG_DATA_HOME/jp/plugins/command/jp-{id}`).
- **`official`**: Whether the JP project publishes this plugin.
  Official entries claim their command path for automatic installation on first
  use and are updated by `jp plugin update`.
  Third-party entries do not claim commands during routing and are installed
  only via `jp plugin install`.
- **`requires`**: Command paths (registry keys) of plugins that must be
  installed for this plugin to run.
  Installing a plugin installs all of its required dependencies first.
  Official plugins may depend only on other official plugins, so first-use
  installation never pulls in third-party code.
  For third-party plugins, `jp plugin install` lists all required third-party
  dependencies in a single confirmation prompt and installs nothing if the user
  declines.
- **`suggests`**: Command paths (registry keys) of plugins that extend this
  entry.
  Used for help aggregation: `jp serve -h` lists suggested sub-plugins as
  available subcommands, marking uninstalled official plugins with `(not
  installed)`.
  Suggested plugins are not installed automatically.

JP caches the raw registry JSON at `$XDG_DATA_HOME/jp/registry.json` as served,
so a newer `jp` binary sharing the data directory can still read entries that an
older `jp` binary ignored.
Running an already-installed plugin or a built-in command never makes a network
request.
JP fetches the registry:

- On every `jp plugin list`, `install`, and `update` invocation (`list` and
  `install` fall back to the cached copy with a warning if the fetch fails).
- Automatically when an unknown command is not claimed by any installed plugin
  and the cached registry is missing or older than a day.
- On `jp -h` when no cached registry exists.

Registry fetches time out after two seconds so a stalled network connection does
not hang the CLI, and a failed fetch leaves any cached registry untouched.

After downloading a plugin binary, JP verifies its SHA-256 checksum before
marking the file executable.
Downloaded binaries are stored in `$XDG_DATA_HOME/jp/plugins/command/jp-{id}`,
separate from `$PATH` and scoped to command plugins.

`jp plugin update` updates official plugins that JP installed.
When the registry lists a new checksum for an installed official plugin, JP
checks the approval store ([RFD 077]) to verify that the on-disk binary still
matches the hash JP originally installed; if so, JP downloads, verifies, and
replaces the binary.
JP skips the update and explains why if the binary was modified locally, was
installed by another tool, or is pinned to a specific checksum in configuration
([RFD 077]).
If a download fails, JP reports the error, continues updating the remaining
plugins, and exits non-zero listing the failed plugins.
Third-party plugins are never updated automatically, and JP never checks for
updates before running an installed plugin.

### Plugin Trust and Configuration

Execution policy, checksum pinning, and per-plugin options are configured under
the `[plugins]` section of `AppConfig`, defined in [RFD 077].
Installation behavior is not configurable: official plugins install on first
use, and third-party plugins install only via `jp plugin install`.

In summary:

- Each plugin has a `run` policy: `ask` (prompt before running), `allow` (run
  without prompting), or `deny` (never run).
  The default policy for every plugin is `ask`.
- Under `ask`, a binary runs without prompting in two cases:
  1. It is an official plugin whose SHA-256 matches its registry entry.
  2. It has a matching record in the user-local approval store, created by `jp
     plugin install`, answering `Y` at the approval prompt, or running `jp
     plugin approve <path>`.
     Each approval record binds the binary's path and SHA-256 digest and is
     never stored in workspace config or conversation history.
     `jp plugin revoke <name>` removes the approval record, and `jp plugin
     uninstall <name>` removes both the approval record and the JP-installed
     binary.
- The `checksum` setting pins a plugin binary to a specific SHA-256 digest; JP
  refuses to run a binary whose digest does not match.
- The `options` setting passes plugin-specific configuration to the plugin in
  the `init` message.
- All plugin settings participate in standard configuration inheritance (global
  → workspace → local → CLI overrides).

## Drawbacks

- **Latency**: Every workspace operation requires a JSON round-trip over a pipe.
  For interactive use (web pages, CLI output), this overhead is negligible; for
  batch processing across thousands of conversations, it would be noticeable and
  could require bulk protocol operations.

- **Protocol maintenance**: The protocol is a versioned public API.
  Adding new messages or optional fields is straightforward, but changing
  existing message schemas requires migration care.

- **No shared memory**: Plugins cannot access JP's in-memory data structures
  directly; every payload must be serialized over the pipe.
  Conversation events are already stored as JSON, making them cheap to forward,
  whereas complex types like `AppConfig` require complete, stable serialization.

- **Two binaries for the web server**: Running the web server requires both `jp`
  and `jp-serve` rather than a single `jp` binary.
  Automatic installation of official plugins hides this during normal use, but
  deployment has more moving parts.

## Alternatives

### Cargo-style thin dispatch (exec and forget)

JP sets environment variables (`JP_WORKSPACE_ROOT`, `JP_STORAGE_DIR`, etc.) and
execs the plugin, which opens the workspace itself using `jp_workspace` as a
library dependency.

Rejected because:

- Plugins must be written in Rust (or FFI into Rust crates) to access the
  workspace safely.
- Every plugin must re-implement JP's bootstrap logic.
- Shell scripts cannot read or write conversations.
- Plugins hold file locks directly, complicating crash recovery.
- Evolving a plugin from read-only to read-write requires re-architecting the
  plugin.

### Feature-gated built-in commands

Keep extensions as built-in commands behind Cargo feature flags (for example,
compiling with `--features web` to include the web server).

Rejected because:

- Not extensible at runtime; third parties cannot add commands.
- Users must compile from source to select features.
- Does not establish a plugin model for external tools.

### Lock-and-push writes

Allow plugins to acquire a conversation lock, push raw events, and release the
lock, with the host validating each batch.

Rejected in favor of task-level operations.
Allowing a plugin to hold a lock across protocol messages requires the host to
track locks and release orphaned locks when a plugin crashes.
It also forces the host to validate externally constructed events against stream
invariants that the host otherwise guarantees by construction.
The trade-off is that the write surface is closed: each new write operation
requires a new protocol message.

### Routing by binary name

Route `jp serve web` directly to `jp-serve-web` by filename, matching `git`,
`cargo`, and `kubectl`, so the host inspects only filenames before admission.

Rejected because a filename encodes only a single command path.
Pre-spawn metadata such as [RFD 114]'s workspace scope (or future claims like
flags on built-in commands) still needs a place to live before the binary runs.
Coupling routing and identity in the filename also means two plugins with the
same name cannot be disambiguated by renaming one file without changing the
subcommand it serves.

### Asking the binary

Spawn each `jp-*` binary with `describe` to discover what command it claims,
matching Docker's CLI plugin discovery.

Rejected because it executes an unapproved binary just to decide how to route a
command.
If the host enforces admission before spawning `describe`, a user running an
unapproved plugin's command gets an "unrecognized subcommand" error and cannot
tell which binary on `$PATH` needs approval.

### Metadata at a fixed offset or in a linker section

Store the manifest at the start of the file, at the end of the file, or in an
executable linker section.

Rejected because the start of an executable is reserved for the OS binary header
(ELF, Mach-O, PE), and appending data to the end of a Mach-O binary invalidates
macOS code signatures (which require `__LINKEDIT` to extend to EOF).
Reading a named linker section requires format-specific binary parsers and has
no equivalent for scripts.
Scanning the file for a marker prefix works uniformly across native binaries and
scripts, at the cost of reading each plugin binary during discovery.

### Wasm plugin model ([RFD 016])

Use the Wasm component model for command plugins.

Wasm plugins and command plugins solve different problems.
Wasm plugins are sandboxed, in-process components suited for fine-grained
capability extensions (attachment handlers, tools).
Command plugins are standalone processes that may run indefinitely and require
direct network access (web servers), filesystem access (exporters), or terminal
control (TUI dashboards).

## Non-Goals

- **In-process native plugins**: Shared library (`.so`/`.dylib`) plugins are out
  of scope; the process boundary provides fault isolation and language
  independence.
- **Plugin authoring SDK**: A high-level Rust SDK wrapping the protocol is left
  for future work; early plugins can use the protocol types directly.
- **Event subscriptions**: Streaming live turn events and forwarding interactive
  prompts (tool approvals, inquiries) to plugins are deferred to a future RFD.
  Delegating a turn via `query` is in scope (see [Workspace
  Mutations](#workspace-mutations)).
- **Plugin-to-plugin communication**: Plugins communicate only with the JP host,
  never directly with each other.
- **Plugin subcommands under built-in command groups**: Supporting plugins like
  `jp conversation export` requires allowing unknown subcommands on built-in
  groups and resolving group aliases (such as `jp c export`), which is left to a
  future RFD.

## Risks and Open Questions

- **Config serialization completeness**: `AppConfig` contains custom types
  (`ModelIdConfig`, `ToolConfig`, etc.) with custom `serde` implementations.
  Because `init` and `read_config` serialize the resolved `AppConfig` to JSON,
  any serialization edge cases must be tested so plugins receive complete
  configuration data.

- **Event format stability**: Exposing `ConversationEvent` JSON over the
  protocol makes the on-disk event schema a public plugin API.
  Schema changes (renamed variants, altered fields) become breaking changes for
  plugins as well as stored conversations.

- **Protocol evolution**: A plugin declares its minimum required `protocol`
  version in both its manifest and `ready`, allowing the host to reject plugins
  that require a newer host before spawning them.
  Because there is no minimum-plugin-version check on the host, deprecating or
  removing a protocol message requires returning an explicit `error` response to
  older plugins rather than rejecting them at startup.

- **Manifest discovery cost**: Command routing and `jp -h` scan every `jp-*`
  binary in the install directory and on `$PATH`.
  If scanning binaries on `$PATH` proves slow in practice, or if future claims
  require scanning manifests on every `jp` invocation, the host can cache parsed
  manifests keyed by file path and modification time.

- **Unverified manifest claims**: Any binary on `$PATH` can claim any command
  path in its manifest.
  If two third-party binaries claim the same command path, JP reports the
  conflict rather than picking whichever appears first on `$PATH`.
  A third-party binary can intentionally override an official command (for
  example, replacing `jp serve web` with a custom server), which also means an
  untrusted binary written to `$PATH` could claim an official command.
  Admission guards against silent hijacking: unless `run = "allow"` is
  configured for that plugin name, JP prompts for approval, labels the binary as
  third-party, names the official command it overrides, and displays the binary
  path.

- **Network fetch on command typos**: Typing an unknown command when the
  registry cache is missing or older than 24 hours triggers a single registry
  fetch (bounded by the two-second timeout) at most once per day.

- **Registry trust model**: Auto-installing official plugins relies on the
  hardcoded HTTPS registry URL (`https://jp.computer/plugins.json`) and SHA-256
  checksum verification of downloaded binaries.

## Implementation Plan

- [x] **Phase 1: Protocol core and dispatcher**

  - Define the protocol message types in a new `jp_plugin` crate.
  - Implement the host-side message loop in JP: spawn the child process,
        send `init`, dispatch requests to `Workspace` methods, and forward
        stderr to tracing.
  - Implement unknown-subcommand dispatch by searching `$PATH` for
        `jp-<name>`.
  - Test with a minimal shell-script plugin.
  - Can be merged independently.

- [x] **Phase 2: Web server as external plugin**

  - Extract the web server into a standalone binary crate
        (`crates/plugins/command/serve-web/`).
  - Implement the plugin-side protocol client (read `init`, send requests,
        render responses).
  - Remove `jp serve` as a built-in command so it dispatches as a plugin.
  - Remove the `jp_web` dependency from `jp_cli`.
  - Depends on Phase 1.

- [x] **Phase 3: Plugin registry and auto-install**

  - Define the registry JSON schema.
  - Implement registry fetching, caching, and binary downloads with SHA-256
        checksum verification.
  - Implement the installation flow (automatic for official plugins,
        prompted for third-party plugins).
  - Add the `jp plugin list`, `jp plugin install`, and `jp plugin update`
        subcommands.
  - Depends on Phase 1; independent of Phase 2.

- [x] **Phase 4: Write operations**

  Implemented as task-level host operations (`ArchiveConversation`,
      `SetTitle`, `WriteDraft`, and `Query` with a `Created` response) rather
      than low-level lock-and-push primitives.
      Each operation acquires and releases the conversation lock inside the
      host, so plugins never hold locks across protocol messages or append raw
      events to a stream.
      This avoids orphan-lock recovery and external event validation, at the
      cost of requiring a new protocol message for each new write operation.

  - Depends on Phase 1.

- [x] **Phase 5: Command routing and plugin dependencies**

  When multiple sources claim a command path, the host resolves them using
      these rules:

  1. Built-in commands always win.
  2. The longest matching claimed command path wins, and any remaining CLI
         arguments are passed to that plugin (if both `serve` and `serve web`
         are claimed, `jp serve web` dispatches to `serve web`).
  3. A third-party binary claiming an official command path overrides the
         official plugin until the binary is removed; the admission prompt
         labels it third-party and names the overridden official command ([RFD
         077]).
  4. Two third-party binaries claiming the same path fail with a diagnostic
         naming both paths.
  5. An official binary's manifest claim must match its registry key; a
         mismatch is reported as an error rather than rerouted.
  6. `plugins.command.<name>` is keyed by the plugin name rather than the
         claimed command path, and two binaries with the same plugin name
         conflict (see [Plugin Identity](#plugin-identity)).

  <!-- end list -->

  - Define the binary manifest format, its validation rules, and a
        `jp_plugin` declaration that generates both the manifest constant and
        the `describe` response.
  - Add `protocol` and a required `help` field to the `describe` response.
  - Scan manifests from the user-local install directory and `$PATH`.
  - Route commands using manifest claims and official registry entries under
        the precedence rules above; ignore binaries without a valid manifest
        unless approved.
  - Fetch the registry when no installed plugin claims a command and the
        cache is missing or older than a day, and on `jp -h` when no cache
        exists, respecting `JP_NO_PLUGIN_DOWNLOAD`.
  - Install official plugins on first use and third-party plugins only via
        `jp plugin install` with confirmation; remove `plugins.auto_install` and
        `plugins.command.<name>.install` ([RFD 077]).
  - Print a `jp plugin install` hint when an unknown command matches a
        third-party registry entry.
  - Run `jp plugin` management commands without requiring a workspace.
  - Populate the "Plugins:" section of `jp -h` from local manifests and the
        registry cache without spawning any binaries.
  - Embed manifests in `jp-gui`, `jp-path`, `jp-serve-web`, and `jp-ticket`.
  - Update `jp plugin install` to resolve and install `requires`
        dependencies.
  - Implement `command_group` help aggregation and merge `suggests`
        sub-plugins into parent plugin help output.
  - Depends on Phase 3 (registry).

- [x] **Phase 6: Admission, containment, and output**

  - Enforce plugin admission before every spawn (including `describe`) in
        the order defined by [RFD 077], with `ask` as the default `run` policy.
  - Record approvals in [RFD 077]'s approval store from `jp plugin install`,
        interactive `Y` confirmations, and `jp plugin approve`.
  - Include context in the approval prompt when a binary's digest has
        changed, when a different path is already approved, or when a
        third-party binary overrides an official command.
  - Add `jp plugin approve <path>`, `jp plugin revoke <name>`, and `jp
        plugin uninstall <name>`, and route manifestless binaries using the
        manifest fields recorded in their approval.
  - Update `jp plugin update` to replace unmodified JP-installed official
        binaries when the registry publishes a new checksum.
  - Reject any `describe` response whose fields disagree with the binary's
        manifest.
  - Terminate the plugin's process group (Unix) or job object (Windows) when
        the shutdown grace period expires.
  - Route `print` messages through the host `Printer` by channel and format
        as specified in [Output](#output), so `--quiet` and `--format json`
        apply to plugin output.
  - Depends on Phase 5.

## References

- [RFD 016: Wasm Plugin Architecture][RFD 016]
- [RFD 026: Agent Loop Extraction][RFD 026]
- [RFD 027: Client-Server Query Architecture][RFD 027]
- [Cargo external tools documentation][cargo-external]
- [Git remote helpers protocol][git-remote-helpers]

[RFD 016]: 016-wasm-plugin-architecture.md
[RFD 026]: 026-agent-loop-extraction.md
[RFD 027]: 027-client-server-query-architecture.md
[RFD 048]: 048-four-channel-output-model.md
[RFD 077]: 077-plugin-configuration-and-trust-policy.md
[RFD 114]: 114-plugin-workspace-scope-and-addressing.md
[cargo-external]: https://doc.rust-lang.org/cargo/reference/external-tools.html#custom-subcommands
[git-remote-helpers]: https://git-scm.com/docs/gitremote-helpers
