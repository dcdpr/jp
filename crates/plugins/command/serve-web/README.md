# jp-serve-web

A command plugin that serves JP conversations over HTTP, and lets you continue
them from a browser.

Run it with `jp serve-web`.
The server is read-write: it renders the transcript, takes a message from a
composer, and asks the host to run the turn.

```sh
jp serve-web --bind 127.0.0.1 --port 3000
```

## What it does and does not own

The plugin is a presentation layer.
It never talks to a model, holds a credential, executes a tool, or writes to a
conversation.
Everything it shows it asked the host for, and every turn it starts the host
runs.

That split is the reason the protocol exists.
A plugin that ran its own agent loop would need the user's API keys, the tool
registry, the MCP servers, and a second copy of the turn loop to keep in step
with the first.

| Concern                     | Owner  |
| --------------------------- | ------ |
| Rendering, routing, styling | Plugin |
| Conversation storage        | Host   |
| Config resolution           | Host   |
| Model calls and tool runs   | Host   |
| Interrupting a turn         | Host   |

## Protocol

Needs protocol 9 (`REQUIRED_PROTOCOL`).
The host refuses an older pairing at the handshake rather than failing later, so
a stale `jp` alongside a fresh plugin is an error message and not a mystery.

| Message                | Direction | Used for                                           |
| ---------------------- | --------- | -------------------------------------------------- |
| `list_conversations`   | → host    | The conversation index                             |
| `read_events`          | → host    | One conversation's transcript, title and lock      |
| `list_configs`         | → host    | The configurations a message can name              |
| `query`                | → host    | Start a turn, or start a conversation              |
| `created`              | ← host    | The id of a conversation just created              |
| `query_complete`       | ← host    | That turn finished                                 |
| `interrupt`            | → host    | Stop a named turn, or answer one without ending it |
| `read_draft`           | → host    | The message being composed, as the CLI stores it   |
| `write_draft`          | → host    | Save it back, conditional on a revision            |
| `archive_conversation` | → host    | Move one conversation to the archive               |
| `set_title`            | → host    | Rename one conversation                            |

Starting a conversation is answered twice: `created` as soon as there is
somewhere to send the reader, and `query_complete` when the first turn ends.
The client registers both waiters before sending, because a turn that finishes
quickly would otherwise arrive before anything was listening for it.

## How the page stays current

There is no push channel yet, so the page polls `/conversations/{id}/messages`
every second while a turn is running and every three when it isn't.
The page says how much of the transcript it holds and how far back its copy is
provisional; the answer carries only the part it does not have, or drew in a
form that has since changed, so a tick that brings nothing new costs one small
response and no re-render.

That second half is what a tool call needs.
A call is drawn when it is requested and gains its result later, so the entry on
the page is not final when it first appears — and by the time the result lands,
the server's own boundary has moved on to the next call still waiting.
Only the page knows how far back to ask from, so it is the page that says.

A boundary is not enough for the newest entry, though, so while a turn is
running that one is re-sent on every tick if it is one that can still change.
A run of assistant text is rendered as one block that the next flush adds to,
and it sits at the end — nothing after it is provisional and the count does not
move, so neither the boundary nor the page's own floor reaches back for it.
An entry that is finished the moment it appears, such as the request itself, is
not re-sent; waiting for the first token is the longest stretch of a turn, and
nothing changes on the page during it.

The host re-reads the conversation from disk on each request, which means a turn
you started in a terminal shows up in the browser too, without a restart.

Sending while a turn is running interrupts the assistant and answers it, inside
the turn that was already going: what it had produced is kept, the message
follows it, and it carries on from there.
This is Ctrl-C then `[r] Reply` at a terminal, and the conversation is never
unlocked in between, so there is no window in which the message could be refused.

Events arrive in batches rather than token by token: the turn loop persists at
each streaming boundary, so a page sees a complete assistant response or tool
call at a time.
Per-token updates need the host to push, which is future work.

Everything on the page works without JavaScript except the polling.
The composer and the stop button are plain form posts, and the transcript is
server-rendered.

## Endpoints

| Path                            | Method | Purpose                          |
| ------------------------------- | ------ | -------------------------------- |
| `/conversations`                | GET    | Index                            |
| `/conversations/{id}`           | GET    | Transcript and composer          |
| `/conversations/{id}/turn`      | POST   | Send a message                   |
| `/conversations/{id}/messages`  | GET    | Transcript as JSON, for the poll |
| `/conversations/{id}/interrupt` | POST   | Stop the running turn            |
| `/configs`                      | GET    | The configuration chooser        |
| `/status`                       | GET    | Whether a turn is in flight      |

The chooser offers the configurations on the load paths by name, and rows for
setting a value directly.
Both reach the host as `--cfg` arguments, the typed values last, so one of those
overrides the same key set by a configuration chosen beside it.

`/configs` answers with markup rather than data.
The new-conversation form offers the same chooser, and rendering it in one place
is what keeps the two grouping, labelling and posting the choices alike.

`/status` exists for whoever supervises the process: restarting to pick up a new
build aborts a turn in flight, so a supervisor polls it and waits for `busy` to
go false.

## Security

No authentication, and every conversation in the workspace is readable.
Anyone who can reach the port can also start a turn, which spends tokens and
runs whatever tools the conversation allows.

Binding to a non-loopback address hands that to the network.
The plugin warns on startup when you do.

Writing requests are refused when they come from a page on another origin, which
is checked from `Sec-Fetch-Site` and `Origin`.
Loopback is no defence on its own here: a form post is not subject to a
preflight, so any site a browser visits can submit one to `127.0.0.1` and start
a turn, and the same-origin policy only stops it reading the answer.
A request that carries neither header — `curl`, a script, another tool — is
allowed, since no browser can be made to omit both.

## Development

A file watcher that restarts on save can't be used here: a turn started from the
browser runs inside the host process the plugin is attached to, so restarting on
save aborts whatever the assistant was in the middle of — including the
assistant editing these files.
Poll `/status` and restart only once `busy` is false.
