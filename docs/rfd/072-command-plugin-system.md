# RFD 072: Command Plugin System

- **Status**: Implemented
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-04-06
- **Extended by**: [RFD 114]
- **Summary**: Standalone command plugins communicate with JP via JSON-lines
  protocol to extend subcommands across languages.

## Summary

This RFD introduces a command plugin system for JP.
Command plugins are standalone binaries (`jp-<name>`) that communicate with JP
over a structured JSON-lines protocol on stdin/stdout.
JP handles workspace discovery, config loading, conversation locking, data
access, output formatting, and signal management.
Plugins request these services over the protocol, making it possible to write a
plugin in any language — including shell scripts.

This is one of several plugin mechanisms in JP.
[RFD 016] defines the Wasm plugin system for sandboxed in-process capabilities
(attachment handlers, tools, LLM providers).
Command plugins operate at a different level: they are long-running processes
that extend JP with new subcommands.

## Motivation

JP's functionality is growing beyond its core query loop.
An ongoing experiment with a web UI is the first example: a long-running server
that reads (and will soon write) conversation data.
Possible future candidates include HTTP APIs, TUI dashboards, import/export
tools, and IDE integrations.

Today, adding any of these means compiling them into the `jp` binary.
This has costs:

- **Binary size**: The web server pulls in axum, hyper, tower, and maud.
  Every user pays this cost whether they use the web UI or not.
- **Coupling**: Every extension must be Rust, must link against JP's internal
  crates, and must be wired into the `Commands` enum and startup pipeline.
- **Release cadence**: A bug fix in the web UI requires a full JP release.

A plugin system solves these problems.
But the design must handle a tension that cargo-style "just exec the binary"
dispatch does not face: JP's startup pipeline provides services (workspace
discovery, config loading, conversation locking, structured output) that plugins
need.
If we push all of that into the plugin, every plugin author re-implements JP's
bootstrap — and gets it wrong (we already hit this with the web server missing
`.with_local_storage()`).

The goal is a plugin system where:

1. JP remains the orchestrator — it finds the workspace, loads config, manages
   locks, and formats output.
2. Plugins are standalone executables that can be written in any language.
3. A shell script can be a useful plugin.
4. A plugin that starts read-only (web viewer) can gain write access (chat)
   without changing its architecture.

## Design

### User Experience

Plugins are invoked as JP subcommands, at the root or under a command group a
plugin or the registry provides:

```txt
jp serve                        # runs jp-serve plugin
jp serve web                    # runs jp-serve-web plugin
jp export --format html         # runs jp-export plugin
jp dashboard                    # runs jp-dashboard plugin
```

No built-in command group accepts plugin children yet: `jp conversation export`
is a built-in group's unknown subcommand, not a plugin.
Opening a built-in group to plugins is future work (see
[Non-Goals](#non-goals)).

Each plugin declares the command path it provides in its manifest, a line of
JSON embedded in the binary that the host reads without running it (see [Plugin
Manifest](#plugin-manifest)).
A plugin whose manifest claims `["serve", "web"]` handles `jp serve web`
regardless of its binary name.
The binary name marks a file as a plugin, through its `jp-` prefix, and names
the plugin's configuration (see [Plugin Identity](#plugin-identity)).

A plugin cannot shadow a built-in command at any level — JP checks built-in
commands first.

Official plugins, the ones the JP project publishes, behave as if they were part
of the `jp` binary but ship separately: typing an official command installs its
plugin on first use.
Third-party plugins are installed only when the user asks, the way `cargo
install` puts a `cargo-*` subcommand in place:

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

Placing a `jp-<name>` binary that carries a manifest on `$PATH` installs a
plugin as well; its first run asks before running it ([RFD 077]).

When JP encounters an unknown subcommand, it:

1. Collects the claims: the manifests of the `jp-*` binaries in the user-local
   install directory (`$XDG_DATA_HOME/jp/plugins/command/`) and on `$PATH`, the
   claims approvals recorded for binaries without a readable manifest, and the
   official entries of the cached registry.
   When nothing claims the command and the cache is missing or more than a day
   old, it fetches the registry once and collects again.
2. Picks the longest claimed path the arguments start with, under the rules in
   [Phase 5](#implementation-plan).
   The remaining arguments go to that plugin.
3. If the claim is an official plugin that is not installed, downloads it and
   checks it against the registry's checksum.
4. Admits the binary: the `run` policy, any pinned checksum, and the approval
   store ([RFD 077]).
5. Spawns the plugin binary and communicates over the protocol.

A third-party registry entry claims no command.
It is a catalog entry for `jp plugin list` and `jp plugin install`, and the
source of the hint above.

`JP_NO_PLUGIN_DOWNLOAD=1` turns off every network request JP makes for plugins
without being asked: fetching the registry, and downloading an official plugin
on first use.
`jp plugin list`, `install`, and `update` still reach the network, because
running them asks for it.
A packager that ships official plugins, such as a system package manager, puts
their binaries on `$PATH`, and JP uses those instead of downloading.

Plugin management commands (`jp plugin list`, `install`, `uninstall`, `update`,
`approve`, and `revoke`) are a built-in subcommand group, not external plugins.
They need no workspace and work from any directory, including outside a
workspace.
The `jp plugin` subcommand takes priority over any external `jp-plugin` binary
on `$PATH`.

### Channel Model

JP spawns the plugin as a child process with three channels:

| Channel    | Direction   | Purpose                                 |
| ---------- | ----------- | --------------------------------------- |
| **stdin**  | JP → plugin | Protocol messages: init, responses      |
| **stdout** | plugin → JP | Protocol messages: requests, print, log |
| **stderr** | plugin → JP | Captured and forwarded to JP's tracing  |
|            |             | subsystem at `trace` level              |

The plugin never writes directly to the user's terminal.
All user-facing output goes through the protocol as `print` commands, which JP
routes through its printer, so a plugin's output follows the host's `--quiet`
and `--format` the way the rules in [Output](#output) describe.

Stderr is captured line-by-line and emitted as `trace`-level tracing events
attributed to the plugin.
This provides a zero-effort debugging channel for plugin authors —
`fprintf(stderr, ...)` in C, `eprintln!()` in Rust, or `echo >&2` in shell —
without polluting user output.

### Protocol

JSON-lines over stdin/stdout.
One JSON object per line, no framing beyond newlines.
Each message has a `type` field.

#### Stdout Hygiene

Using stdout for protocol framing means any non-protocol output from the plugin
(a stray `printf` in a C library, an uncaught panic message) corrupts the
JSON-lines stream.
This is a known trade-off shared with LSP, MCP, and git remote helpers, all of
which use stdin/stdout successfully.

The mitigation is straightforward: stderr is the designated escape valve.
Plugin authors use stderr for all debugging output (`eprintln!()` in Rust, `echo

> &2`in shell,`fprintf(stderr, ...)` in C), and JP forwards it to tracing.
> The protocol contract is simple: stdout is exclusively for protocol messages.

Stdin/stdout is chosen because it is the simplest cross-language, cross-platform
transport — no socket setup, no file descriptor passing, and it works in shell
scripts with `echo` and `read`.
An alternative transport (FD 3/4, domain sockets) could be explored in a future
protocol version if stdout pollution proves to be a recurring problem in
practice, but the added complexity is not justified given the current design
goal of shell-script accessibility.

#### Request IDs

Plugin-to-JP messages may include an optional `id` field (string).
When present, JP echoes the same `id` in the corresponding response.
This allows multi-threaded plugins to issue concurrent requests and match
responses to the originating request.

For synchronous plugins (shell scripts, single-threaded tools), `id` can be
omitted entirely.
JP processes requests in order, and a request answered immediately is answered
in that order, so correlation is implicit.

`query` and `interrupt` are answered later, from the turn they act on, and their
replies can arrive between replies to requests sent after them.
A plugin that sends either must set `id`: an `interrupt` without one is
performed and never answered.

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

JP then waits for any turn the plugin delegated to finish, and exits with the
given code.
If the plugin process exits without sending `exit` (crash, signal), JP detects
the EOF, waits the same way, and exits with code 1.

A delegated turn outlives the plugin that asked for it.
Its conversation stays locked until the turn ends, and the host waits for it
without a time limit, because the work is the user's.
A run that has to end sooner goes through the interrupt ladder, which stops the
turn.

#### Shutdown

When JP receives a signal (SIGINT, SIGTERM), it sends:

```json
{
  "type": "shutdown"
}
```

The plugin should begin graceful shutdown and eventually send `exit`.
If the plugin does not exit within a grace period (configurable, default 5
seconds), JP terminates it.

JP owns the plugin's process tree for the lifetime of the invocation, not only
the process it spawned.
Termination kills the plugin's process group on Unix and its job object on
Windows, so a worker a shell-script plugin started cannot outlive it, keep a
port bound, or hold the pipes JP waits on.
A descendant that deliberately detaches from the group is unsupported.

To ensure the plugin receives `Shutdown` via the protocol rather than being
killed directly by the OS signal, JP spawns the child in its own process group
(`process_group(0)` on Unix).
This prevents SIGINT/SIGTERM from reaching the child directly — only the host
receives the signal and relays it through the protocol.

#### Plugin Manifest

Every plugin binary carries a manifest: one line of JSON, embedded in the file,
that the host reads without running the binary.

```txt
jp-plugin/v1 {"protocol":1,"description":"Read-only web UI for browsing conversations","command":["serve","web"]}
```

The host searches the file for `jp-plugin/v` followed by a version number and a
space, and parses the JSON after it, up to the first newline or NUL byte.
Where the line sits is up to the plugin:

- A compiled plugin carries it as a string constant its code references, so the
  linker keeps it.
  In Rust, `jp_plugin` builds the constant and the matching `describe` answer
  from one declaration.
- A script carries it in a comment on any line after the shebang, such as `#
  jp-plugin/v1 {...}`.
  Anything before the marker on that line is ignored.

The manifest holds only what the host needs before it has decided to run the
plugin:

- **`protocol`** (required): The lowest host protocol version the plugin needs,
  the same number it states in `ready`.
  A host that speaks less refuses the plugin before spawning it.
- **`description`** (required): One line, shown in `jp -h` and in the approval
  prompt.
- **`command`** (required): The claimed command path.
  `["serve", "web"]` claims `jp serve web`, whatever the binary is named, and a
  segment may contain dashes.

The claims a manifest can make grow with the protocol.
A claim an older host can safely ignore is added as an optional key, and a host
ignores keys it does not know.
A claim an older host must not ignore, such as [RFD 114]'s `workspace_scope:
"multi"`, comes with a higher `protocol`, so an older host refuses the plugin
rather than misreading it.
The `v1` in the marker changes only when the framing changes, or when a field is
removed or changes meaning; a host that finds a version it does not know reports
the plugin as built for a newer `jp`.

A manifest is valid when:

- The marker occurs exactly once in the file.
  Two could disagree, and the host would have to pick one.
- The JSON after it is at most 64 KiB, is UTF-8, and parses.
- No string in it holds a control character other than `\n` and `\t`.
  The description reaches the terminal from a binary nobody has approved yet,
  and an escape sequence in it would reach the terminal with it.

A binary without a valid manifest claims nothing.
`jp -h` and `jp plugin list` show it by file name, with the reason, and the only
way to route to it is `jp plugin approve <path>` ([RFD 077]), which records the
manifest fields from its `describe` answer in the approval.
That is also the path for a binary whose file hides the manifest, such as one
compressed with UPX.

#### Plugin Identity

A plugin's name is its file name without the `jp-` prefix, and without `.exe` on
Windows.
A plugin installed from the registry is written as `jp-{id}`, so its name is its
registry `id`.
The name keys its configuration (`plugins.command.<name>`), its approval, `jp
plugin uninstall <name>`, and `jp plugin revoke <name>`.
The claimed command path never does: it is asserted by the plugin, can change
between versions, and a plugin may later claim more than one command.

A plugin is official when its name is the `id` of an official registry entry:
the binary JP downloaded for that entry, or a copy of it that a packager put on
`$PATH`.
Every other binary is third-party, whatever it claims.

Two binaries with the same name conflict, whatever they claim.
They would share one `run` policy, one pinned checksum, one set of options, and
one approval, so approving one would approve the other.
The host names both paths and runs neither.
Renaming one resolves it, because the manifest, not the file name, decides what
a plugin handles; when one is a copy JP installed, `jp plugin uninstall` removes
it.
Paths that resolve to the same file, such as two symlinks into one package
store, are one binary.

#### Plugin Self-Description

An admitted plugin describes itself in full over the protocol.
This is used for `jp <plugin> -h` (showing plugin help) and for `jp plugin
approve`.

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

The answer carries every manifest field, with the same meaning, and adds:

- **`name`**, **`version`** (required).
- **`help`** (required): Shown for `jp <plugin> -h`.
  It is computed when the plugin runs, so it can come from the same definition
  the plugin parses its arguments with.
- **`author`**, **`repository`** (optional).

The manifest and the answer come from one binary and must agree.
The host compares them whenever it has both, and a `protocol` or claim that
differs is an error naming the plugin, not a choice between the two.
For a binary without a manifest, the answer is compared with the manifest fields
its approval recorded.

A future RFD can introduce structured help text that enables the host to render
plugin help through clap for consistent formatting.

When a plugin binary is invoked directly (not through `jp`), it should detect
that stdin is a TTY and print its own help to stderr before exiting.

Admission — the `run` policy, a pinned checksum, approval of a `$PATH` binary
([RFD 077]) — precedes every spawn of a plugin binary, including one that only
answers `describe`.
Sending `describe` does not make running an unapproved executable safe: it can
do anything before it reads its first message.

`jp -h` reads manifests and spawns nothing.
Each plugin is listed under its claimed command path with its manifest's
description, in a "Plugins:" section after the built-in commands.
A binary without a valid manifest is listed under the manifest fields its
approval recorded, or by its file name when it has no approval.
Official commands whose plugin is not installed yet are listed from the registry
cache, so they read as part of `jp` from the first run; `jp -h` fetches the
registry when there is no cache, unless `JP_NO_PLUGIN_DOWNLOAD` is set.
`jp <plugin> -h` goes through ordinary admission, prompting where the policy is
`ask`, and shows the `help` from a fresh `describe`.

#### Help Aggregation for Command Groups

The registry supports `command_group` entries — command namespaces with no
binary.
A group provides help text and lists sub-plugins, but does not execute any code.
When the user runs `jp serve -h` and `serve` is a group:

1. JP reads the group's `description` and `suggests` list from the registry.
2. Checks for local plugins whose manifest claims a path under `["serve", ...]`.
3. Checks the registry for plugins under the same prefix that are not installed.
4. Merges everything into the help output:

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

The "(not installed)" marker signals that an official subcommand is available
but not yet downloaded; running `jp serve http-api` installs it.
A third-party subcommand is listed with the command that installs it.

When a group is invoked without a subcommand (`jp serve`), JP prints the help
text and exits with code 2, matching the behavior of built-in command groups
like `jp conversation`.

A real plugin can also have sub-plugins beneath it.
When `jp serve -h` is requested and a plugin claims `serve`, JP admits it, sends
`describe` to it, *and* checks local manifests and the registry for plugins
whose command path extends `["serve", ...]`.
Both the plugin's own help text and the discovered sub-plugins are merged into
the output.

#### Plugin Tracing

Plugins can send structured log messages at any level through the protocol.
JP re-emits these as tracing events under the `plugin` target at the specified
level, making them visible in `jp -v` output and the trace log file.

For Rust plugins, this is best implemented as a custom `tracing::Layer` that
serializes events as `PluginToHost::Log` messages on stdout.
The layer can buffer events during startup and flush them once the protocol
writer is available.
Use `try_lock` on the shared stdout writer to avoid deadlocking when a tracing
event fires while the writer is already held.

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

The `path` syntax uses the same dot-separated keys as the `--cfg` CLI flag
(e.g., `assistant.model`, `server.web.port`, `conversation.tools`).
An invalid path returns an error.

The host answers from the configuration it read at startup.
A plugin that runs long enough to outlive a config edit sets `reload` to have
the host read it again first:

```json
{
  "type": "read_config",
  "reload": true
}
```

The host repeats its startup load: every config file and its `extends` chain,
the environment, and the invocation's `--cfg` arguments.
The fresh result answers this request and every later `read_config`, so a plugin
polls and compares without knowing where configuration lives.
A configuration that fails to load is reported as an `error`, and the last one
that loaded stays in place.
`reload` needs protocol 10.

#### Workspace Mutations

A plugin changes a workspace through task-level operations.
The host takes the conversation's lock itself for each one, so a plugin never
holds a lock and never appends events to a stream directly.

| Request                | Reply                                                                | Lock held                        |
| ---------------------- | -------------------------------------------------------------------- | -------------------------------- |
| `archive_conversation` | `done`                                                               | for the request                  |
| `set_title`            | `done`                                                               | for the request                  |
| `write_draft`          | `draft`, with `conflict` when refused                                | none; a revision check guards it |
| `query`                | `created` (new conversations only), then `query_complete` or `error` | until the turn ends              |
| `interrupt`            | `done` or `error`, once the turn has acted, when it carries an `id`  | the turn's own                   |

`query` asks the host to run a turn: it locks the conversation, appends the
request, calls the provider, runs the tools the assistant asks for, and persists
the result — the same turn `jp query` runs.
A plugin doing this itself would need the user's credentials, the tool registry,
and the MCP servers.

The turn runs beside the message loop, which keeps answering other requests
while it does.
When the request starts a conversation, `created` arrives as soon as it exists,
carrying the id the host assigned.
`query_complete` arrives when the turn ends; an `error` arrives instead if it
could not start or failed.
The conversation stays locked in between, so a second `query` for it is refused
as already locked, and a message meant for the running turn goes through
`interrupt`.

A `query` carrying a `schema` asks for structured output, as `jp query --schema`
does, and its `query_complete` carries the parsed response in `data`.
One with `new` can also set `expires_in` (`5m`, `1h`), making the conversation
it creates temporary, as `jp query --tmp` does.
Both need protocol 11.

What happens to a running turn when the plugin exits is described under
[Lifecycle](#lifecycle).

#### Output

All user-facing output goes through the protocol, and JP routes it through the
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

The `channel` field specifies the output category, controlling filtering and
semantic treatment.
The `format` field specifies how JP should render the text.
Both are optional.

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

The simplest case remains simple — a shell script can send
`{"type":"print","text":"hello\n"}` and it works.

**Output modes.** The host's output rules ([RFD 048]) decide where a `print`
lands and how it is shaped:

- `content` goes to stdout.
  `chrome`, `tool_call`, `tool_result`, and `reasoning` are chrome on stderr,
  which `--quiet` suppresses.
  `error` goes to stderr and is never suppressed.
- In terminal output, `format` decides the rendering in the table above.
- Under `--format json`, rendering is off.
  A `print` with `format: "json"` is emitted as the JSON value its `text` holds;
  every other format is wrapped in the printer's message record.
  A plugin that wants its own structure reads `init.output_format` and sends
  `json`.

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

JP emits this as a tracing event at the specified level, attributed to the
plugin.
Valid levels: `trace`, `debug`, `info`, `warn`, `error`.

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

The web server plugin (`jp-serve`) uses the protocol for data access but manages
its own HTTP listener:

1. Receives `init`, extracts config for bind address and port.
2. Sends `ready`.
3. Starts an HTTP server (axum, actix, whatever).
4. On each page request, sends `list_conversations` or `read_events` over the
   protocol and renders the response as HTML.
5. On `shutdown`, stops accepting connections, finishes in-flight requests,
   sends `exit`.

A chat interface adds `query` and `interrupt` to this, and polls `read_events`
while a turn runs; pushing events to it as they happen needs the subscriptions
listed under Non-Goals.

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

Registry keys are space-separated command paths.
`"serve web"` corresponds to `jp serve web`.
Each key is unique by construction (JSON object keys), so no two registry
entries can claim the same subcommand.
That says nothing about installed binaries outside the registry; how the host
chooses between claims is part of [Phase 5](#implementation-plan).

The `type` field identifies the entry's kind:

| Kind            | Description                                            |
| --------------- | ------------------------------------------------------ |
| `command`       | A standalone binary using the JSON-lines protocol.     |
|                 | Default when absent, so older registry entries remain  |
|                 | valid.                                                 |
| `command_group` | A command namespace with no binary. Provides help text |
|                 | and lists sub-plugins via `suggests`. `jp <group>`     |
|                 | prints help and exits with code 2.                     |

Future plugin types (e.g. `"wasm"` from [RFD 016]) will use additional values.
JP ignores an entry with an unrecognized `type`, or one that does not parse, and
reads the rest of the registry.

**`id`** (required) — Stable identifier used for binary naming, config keys,
and install paths.
The binary is `jp-{id}`, config lives at `plugins.command.{id}`, and the install
path is `$XDG_DATA_HOME/jp/plugins/command/jp-{id}`.
The `id` must be unique across all registry entries.

**`official`** — Whether the JP project publishes this plugin.
An official entry claims its command path: JP installs it the first time the
command is typed, and updates it as below.
A third-party entry claims nothing, and is installed only by `jp plugin
install`.

**`requires`** — Command paths (registry keys) of plugins that must be
installed for this one to work.
When JP installs a plugin, it first installs all required dependencies.
An official plugin may require only official ones, so installing it on first use
never brings third-party code with it.
`jp plugin install` lists a third-party plugin's third-party dependencies in the
same confirmation, and installs none of them if the user declines.

**`suggests`** — Command paths (registry keys) of plugins that extend this one.
Used for help aggregation: `jp serve -h` shows suggested sub-plugins as
available subcommands, with an "(not installed)" marker for those not yet
downloaded.
Suggested plugins are not installed automatically.

JP keeps the registry it last fetched at `$XDG_DATA_HOME/jp/registry.json`, as
served, so a newer `jp` sharing the directory still finds the entries an older
one ignores.
Running a plugin installed on this machine never reaches the network, and
neither does any built-in command.
JP fetches the registry:

- For `jp plugin list`, `install`, and `update`, every time; `list` and
  `install` fall back to the stored copy when the fetch fails, and say so.
- On its own, for a command nothing installed claims, when the stored copy is
  missing or more than a day old.
- For `jp -h`, when there is no stored copy.

Every fetch gives up after two seconds, so a network that stalls rather than
fails costs a moment, not a hang.
A failed fetch leaves the stored copy as it was.

Binary checksums are validated after download before the binary is made
executable.
Installed binaries are stored at `$XDG_DATA_HOME/jp/plugins/command/jp-{id}`,
keeping them separate from `$PATH` and leaving room for other plugin types.

`jp plugin update` updates the official plugins JP installed.
When the registry lists a different checksum for one, and the installed binary
is still the one JP wrote, which the approval store records ([RFD 077]), JP
downloads the new one, verifies it, and replaces the old one.
It leaves a binary in place, and says why, when that binary has been changed on
this machine, when something other than JP put it there, or when a pinned
checksum ([RFD 077]) says which binary to run.
A download that fails is reported and the others still run; the command then
exits non-zero, naming the plugins it could not update.
Third-party plugins are never updated automatically.
Updating is left to `jp plugin update` rather than done before a plugin runs, so
running an installed plugin costs no request on a slow network.

### Plugin Trust and Configuration

Execution policy, checksum pinning, and per-plugin options are controlled
through the `[plugins]` section of `AppConfig`, defined in [RFD 077].
Installing is not configured: official plugins install on first use, and
third-party plugins when the user runs `jp plugin install`.

In summary:

- Each plugin has a `run` policy: `ask` (prompt), `allow` (run without asking),
  or `deny` (never run).
  `ask` is the default for every plugin.
- Under `ask`, two things answer without a prompt.
  An official binary whose SHA-256 matches its registry entry is verified by the
  registry.
  Any other binary needs an approval: `jp plugin install` records one, as do
  answering `Y` at the prompt and running `jp plugin approve <path>`.
  An approval holds the binary's path and SHA-256 in a user-local approval
  store, which never enters config or a conversation.
  `jp plugin revoke <name>` removes it, and `jp plugin uninstall <name>` removes
  it with the binary JP installed.
- A `checksum` field pins the binary to a specific hash.
  JP refuses to run a binary whose checksum doesn't match the pinned value.
- An `options` field passes opaque configuration to the plugin via the `init`
  message.
- All plugin config participates in the standard config inheritance chain
  (global → workspace → local → CLI overrides).

## Drawbacks

- **Latency**: Every workspace operation requires a JSON round-trip over a pipe.
  For human-interactive use cases (web pages, CLI output) this is negligible.
  For batch processing of thousands of conversations, it would be noticeable.
  This can be mitigated later with bulk operations or a binary protocol.

- **Protocol maintenance**: The protocol is a public API surface that must be
  versioned and maintained.
  Adding new operations is straightforward (additive change), but changing
  existing message formats requires care.

- **No shared memory**: Plugins cannot access JP's in-memory data structures
  directly.
  Every piece of data must be serialized and sent over the pipe.
  For the conversation events that are already stored as JSON, this is natural.
  For complex types like `AppConfig`, the serialization must be complete and
  stable.

- **Two binaries for the web server**: Users who previously had a single `jp`
  binary now need `jp` plus `jp-serve`.
  The auto-install mechanism mitigates this, but it adds moving parts.

## Alternatives

### Cargo-style thin dispatch (exec and forget)

JP sets environment variables (`JP_WORKSPACE_ROOT`, `JP_STORAGE_DIR`, etc.) and
execs the plugin.
The plugin opens the workspace itself using `jp_workspace` as a library
dependency.

Rejected because:

- Plugins must be Rust (or FFI into Rust crates) to use the workspace safely.
- Every plugin re-implements bootstrap logic and gets it wrong.
- No way for a shell script to access conversations.
- No lock management — plugins hold flocks directly, making crash recovery
  harder.
- Switching from read-only to read-write requires architectural changes in the
  plugin.

### Feature-gated built-in commands

Keep plugins as built-in commands behind cargo feature flags.
Users compile with `--features web` to include the web server.

Rejected because:

- Not extensible at runtime.
  Third parties cannot add commands.
- Users must compile from source to choose features.
- Does not establish a plugin pattern for the ecosystem.

### Lock-and-push writes

The plugin locks a conversation, pushes events into it, and unlocks it, with the
host validating each batch.

Rejected in favor of task-level operations.
A plugin holding a lock across messages needs the host to track it and release
it when the plugin crashes, and events written from outside have to be validated
against the stream's invariants, which the host otherwise upholds by
construction.
The cost of the task-level surface is that it is closed: a new kind of write
needs a new message.

### Routing by binary name

`jp-serve-web` answers `jp serve web`, the way git, cargo, and kubectl route.
Nothing runs before admission, since the host reads only the file name.

Rejected because a file name carries one command path and nothing else.
[RFD 114]'s workspace scope, and any later claim such as a flag on a built-in
command, has to be known before the plugin runs, and needs somewhere to live.
The file name would also be both the route and the identity, so two plugins that
happen to share a name could not be told apart by renaming one without changing
the command it answers.

### Asking the binary

Spawn each `jp-*` binary with `describe` and route by its answer, as Docker does
for binaries in its plugin directories.

Rejected because it runs a binary to decide whether to run it.
With admission first, a user typing a documented command for a plugin they have
not approved finds nothing to route to, and cannot tell which of the unapproved
binaries on `$PATH` to approve.

### Metadata at a fixed position or in a linker section

Put the manifest in the first bytes of the file, at its end, or in a named
section of the executable.

Rejected because the first bytes belong to the header the operating system loads
a native executable by, and data appended at the end breaks macOS code signing,
which expects `__LINKEDIT` to cover the end of the file.
A named section needs a parser for each executable format, and has no equivalent
in a script.
A marker anywhere in the file works for all of them, at the cost of reading each
plugin binary to find it.

### Wasm plugin model ([RFD 016])

Use the Wasm component model for command plugins.

Not rejected, but not suitable for this use case.
Wasm plugins are sandboxed in-process components for capability extensions
(attachment handlers, tools).
External command plugins are long-running processes that need direct network
access (web servers), filesystem access (exporters), or terminal control (TUI
dashboards).
The two systems are complementary: Wasm for fine-grained capabilities, external
commands for coarse-grained extensions.

## Non-Goals

- **In-process plugin loading**: Shared library (`.so`/`.dylib`) plugins are not
  in scope.
  The process boundary provides isolation and language independence.
- **Plugin authoring SDK**: A Rust crate that wraps the protocol into a
  convenient API is future work.
  The protocol is simple enough that early plugins can be written against it
  directly.
- **Event subscriptions**: Live event streaming and forwarding interactive
  events (tool approval, inquiries) to a plugin will be defined in a future
  extending RFD.
  Delegating a turn is in scope; see [Workspace
  Mutations](#workspace-mutations).
- **Plugin-to-plugin communication**: Plugins communicate with JP, not with each
  other.
- **Plugin children of built-in command groups**: `jp conversation export` as a
  plugin.
  Opening a built-in group needs it to accept unknown subcommands, and a rule
  for its aliases so `jp c export` resolves too; a later RFD can do both.

## Risks and Open Questions

- **Config serialization completeness**: `AppConfig` contains custom types
  (`ModelIdConfig`, `ToolConfig`, etc.) with complex serialization.
  The protocol sends the full config as JSON, which must faithfully represent
  all fields a plugin might need.
  The existing `serde` implementations should cover this, but edge cases (e.g.,
  enum variants with custom serializers) need testing.

- **Event format stability**: The protocol exposes `ConversationEvent` JSON as a
  public API.
  Changes to the event schema (new fields, renamed types) become breaking
  changes for plugins.
  This is already partially true for on-disk compatibility, but the plugin
  protocol makes it explicit.

- **Protocol evolution**: A plugin states in its manifest, and again in `ready`,
  the lowest protocol version it needs, and the host refuses one that needs more
  than it speaks before spawning it.
  Nothing lets the host refuse a plugin too old for it, so removing a message or
  a field means answering the old form with an error rather than relying on the
  version check.

- **Manifest discovery cost**: Routing and `jp -h` read every `jp-*` binary in
  the install directory and on `$PATH`.
  How long that takes has not been measured; a cache is added once it has been,
  and only if the measurement asks for one.
  A claim the host needs on every invocation, such as a flag on a built-in
  command, puts that reading on every `jp` run, and needs measuring before it
  ships.

- **Claims are asserted by the plugin**: A manifest can claim any command path,
  as a file name could.
  Two third-party claimants of one path are reported rather than resolved by
  `$PATH` order, and whichever binary a claim selects is still admitted before
  it runs.
  A third-party binary can replace an official command, which is how a user
  swaps in a web server of their own for `jp serve web`; the same rule lets
  anything that can write to a directory on `$PATH` try it.
  The admission prompt is the guard: it marks the binary as third-party, names
  the official command it replaces, and shows the binary's path.
  Config that sets `run = "allow"` for that plugin's name skips the prompt, as
  it does for any plugin.

- **Network use on an unknown command**: A typo on a machine whose registry
  cache is missing or stale costs one registry fetch, at most once a day.

- **Registry trust model**: Auto-installing official plugins requires trusting
  the registry file and the download URLs.
  The checksum validation protects against tampering in transit.
  The registry itself is served from `https://jp.computer/plugins.json` over
  HTTPS.
  The registry URL is hardcoded in the JP binary.

## Implementation Plan

- [x] **Phase 1: Protocol core and dispatcher**

  - Define the protocol message types in a new `jp_plugin` crate.
  - Implement the parent-side message loop in JP: spawn child, send `init`,
        relay requests to `Workspace` methods, capture stderr to tracing.
  - Implement unknown-subcommand dispatch: search `$PATH` for `jp-<name>`.
  - Test with a minimal shell script plugin.
  - Can be merged independently.

- [x] **Phase 2: Web server as external plugin**

  - Extract the web server into a standalone binary crate
        (`crates/plugins/command/serve-web/`).
  - Implement the plugin-side protocol client (reads init, sends requests,
        renders responses).
  - Remove `jp serve` as a built-in command; it becomes a plugin dispatch.
  - Remove the `jp_web` dependency from `jp_cli`.
  - Depends on Phase 1.

- [x] **Phase 3: Plugin registry and auto-install**

  - Define the registry JSON format.
  - Implement registry fetch, caching, and binary download with checksum
        validation.
  - Implement the install flow (silent for official, prompted for
        third-party).
  - Add `jp plugin list`, `jp plugin install`, `jp plugin update`
        subcommands.
  - Depends on Phase 1.
        Independent of Phase 2.

- [x] **Phase 4: Write operations**

  Shipped as task-level operations rather than the lock-and-push primitives
      originally proposed: `ArchiveConversation`, `SetTitle`, `WriteDraft`, and
      `Query` (with a `Created` response), each performing its own locking
      inside the host.
      A plugin therefore never holds a lock across protocol messages, which
      removes the lock-tracking and orphan-release problem the original phase
      carried, and removes the need to validate externally pushed events — no
      plugin appends to an event stream directly.
      The cost is that the write surface is closed rather than general: a new
      write operation needs a new protocol message.

  - Depends on Phase 1.

- [x] **Phase 5: Command routing and plugin dependencies**

  Once manifests route, several sources can claim one path, and the host
      chooses by these rules:

  1. A built-in command path wins.
  2. The longest claimed path wins, and the unmatched arguments go to that
         plugin: with plugins claiming `serve` and `serve web` both installed,
         `jp serve web` runs the second.
  3. A third-party binary claiming an official command wins over the
         official plugin, and replaces it until the binary is removed.
         Admission labels it third-party and names the official command it
         replaces ([RFD 077]).
  4. Two third-party binaries claiming the same path produce a diagnostic
         naming both, never a silent choice.
  5. An official binary must claim the path its registry key names; a
         mismatch is an error, not a reroute.
  6. `plugins.command.<name>` is keyed by the plugin's name, never by the
         command path, and two binaries with the same name conflict ([Plugin
         Identity](#plugin-identity)).

  <!-- end list -->

  - Define the manifest and its validation, and a `jp_plugin` declaration
        that builds both the manifest and the `describe` answer.
  - Add `protocol` and a required `help` to the `describe` answer.
  - Read manifests from the install directory and `$PATH`.
  - Route by manifest claims and official registry entries under the rules
        above; a binary without a valid manifest claims nothing.
  - Fetch the registry when nothing claims a command and the cache is
        missing or more than a day old, and for `jp -h` when there is no cache;
        honor `JP_NO_PLUGIN_DOWNLOAD`.
  - Install official plugins on first use and third-party ones only through
        `jp plugin install`, which asks first; remove `plugins.auto_install` and
        `plugins.command.<name>.install` ([RFD 077]).
  - Hint at `jp plugin install` for a command a third-party registry entry
        names.
  - Run the `jp plugin` management commands without a workspace.
  - List plugins in `jp -h` from their manifests and the registry cache,
        spawning nothing.
  - Give `jp-gui`, `jp-path`, `jp-serve-web`, and `jp-ticket` manifests.
  - Update `jp plugin install` to resolve and install required dependencies.
  - Implement `command_group` help aggregation.
  - Update help rendering to merge suggested sub-plugins (installed and
        uninstalled) into parent plugin help output.
  - Depends on Phase 3 (registry).

- [x] **Phase 6: Admission, containment, and output**

  - Admit a plugin before every spawn, including `describe` for help, in the
        order [RFD 077] gives, with `ask` as the default `run` policy for every
        plugin.
  - Record approvals as [RFD 077]'s approval store describes, from `jp
        plugin install`, a `Y` answer, and `jp plugin approve`.
  - Say in the prompt when the binary changed, when another path is the
        approved one, and when a third-party binary replaces an official
        command.
  - Add `jp plugin approve <path>`, `jp plugin revoke <name>`, and `jp
        plugin uninstall <name>`, and route a binary without a manifest by the
        manifest fields its approval recorded.
  - Have `jp plugin update` update an official plugin JP installed when the
        registry lists a new checksum for it.
  - Refuse a `describe` answer that disagrees with the manifest.
  - Terminate the plugin's process group (Unix) or job object (Windows)
        after the shutdown grace period.
  - Route `print` through the printer by channel and format, as
        [Output](#output) describes; today the host writes `text` to stdout
        unrendered, so `--quiet` and `--format json` do not reach it.
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
