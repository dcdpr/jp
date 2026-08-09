# jp-serve-web

A command plugin that serves JP conversations over HTTP, and lets you continue
them from a browser.

Run it with `jp serve-web`. The server is read-write: it renders the transcript,
takes a message from a composer, and asks the host to run the turn.

```sh
jp serve-web --bind 127.0.0.1 --port 3000
```

## What it does and does not own

The plugin is a presentation layer. It never talks to a model, holds a
credential, executes a tool, or writes to a conversation. Everything it shows it
asked the host for, and every turn it starts the host runs.

That split is the reason the protocol exists. A plugin that ran its own agent
loop would need the user's API keys, the tool registry, the MCP servers, and a
second copy of the turn loop to keep in step with the first.

| Concern                     | Owner  |
| --------------------------- | ------ |
| Rendering, routing, styling | Plugin |
| Conversation storage        | Host   |
| Config resolution           | Host   |
| Model calls and tool runs   | Host   |
| Interrupting a turn         | Host   |

## Protocol

Needs protocol 7 (`REQUIRED_PROTOCOL`). The host refuses an older pairing at the
handshake rather than failing later, so a stale `jp` alongside a fresh plugin is
an error message and not a mystery.

| Message              | Direction | Used for                                |
| -------------------- | --------- | --------------------------------------- |
| `list_conversations` | → host    | The conversation index                  |
| `read_events`        | → host    | One conversation's transcript and title |
| `list_configs`       | → host    | The configurations a new conversation can name |
| `query`              | → host    | Start a turn, or start a conversation   |
| `created`            | ← host    | The id of a conversation just created   |
| `query_complete`     | ← host    | That turn finished                      |
| `interrupt`          | → host    | Stop the turn on one named conversation |
| `read_draft`         | → host    | The message being composed, as the CLI stores it |
| `write_draft`        | → host    | Save it back, conditional on a revision |

Starting a conversation is answered twice: `created` as soon as there is
somewhere to send the reader, and `query_complete` when the first turn ends. The
client registers both waiters before sending, because a turn that finishes
quickly would otherwise arrive before anything was listening for it.

## How the page stays current

There is no push channel yet, so the page polls `/conversations/{id}/messages`
every second while a turn is running and every three when it isn't. The endpoint
returns an event count and the rendered transcript; the page swaps its contents
only when the count moves, so reading isn't interrupted on every tick.

The host re-reads the conversation from disk on each request, which means a turn
you started in a terminal shows up in the browser too, without a restart.

Events arrive in batches rather than token by token: the turn loop persists at
each streaming boundary, so a page sees a complete assistant response or tool
call at a time. Per-token updates need the host to push, which is future work.

Everything on the page works without JavaScript except the polling. The composer
and the stop button are plain form posts, and the transcript is server-rendered.

## Endpoints

| Path                             | Method | Purpose                          |
| -------------------------------- | ------ | -------------------------------- |
| `/conversations`                 | GET    | Index                            |
| `/conversations/{id}`            | GET    | Transcript and composer          |
| `/conversations/{id}/turn`       | POST   | Start a turn                     |
| `/conversations/{id}/messages`   | GET    | Transcript as JSON, for the poll |
| `/conversations/{id}/interrupt`  | POST   | Stop the running turn            |
| `/status`                        | GET    | Whether a turn is in flight      |

`/status` exists for whoever supervises the process: restarting to pick up a new
build aborts a turn in flight, so a supervisor polls it and waits for `busy` to
go false. `just serve-web-watch` does exactly that.

## Security

No authentication, and every conversation in the workspace is readable. Anyone
who can reach the port can also start a turn, which spends tokens and runs
whatever tools the conversation allows.

Binding to a non-loopback address hands that to the network. The plugin warns on
startup when you do.

## Development

```sh
just serve-web-watch --bind 0.0.0.0 --port 3001
```

Rebuilds on any change under `crates/` and restarts once no turn is running. A
plain file watcher can't be used here: a turn started from the browser runs
inside the host process the plugin is attached to, so restarting on save aborts
whatever the assistant was in the middle of — including the assistant editing
these files.
