# jp-serve-chat

A command plugin (`jp serve chat`) that relays messages from a chat app to JP
turns and sends the assistant's answers back.

The plugin acts as a bridge.
It does not call models, hold LLM credentials, or execute tools.
Instead, the `jp` host runs each message as a turn under the configured persona,
and the bridge sends the turn's final response back to the chat.

| Concern                                   | Owner  |
| ----------------------------------------- | ------ |
| Receiving and sending chat messages       | Plugin |
| Sender allowlist and conversation routing | Plugin |
| Locking, pulling, and checking the clone  | Plugin |
| Model calls, tools, conversation storage  | Host   |

## How a message becomes a turn

- Only messages from configured senders are answered; messages from unknown
  senders are dropped and logged without their text.
- Each chat gets one conversation per day, or per week with `rollover` set to a
  weekday, local-only and expiring after `conversation_ttl`.
- One turn runs at a time, as the person whose message started it.
  A later message from that person in the same chat is injected into the running
  turn as a reply (like `[r] Reply` in the terminal), one reply at a time and in
  the order the messages were sent.
  Any other message is queued: consecutive messages from one person in one chat
  become one turn, and a different person's message becomes a turn of its own.
- Each turn sets `user.name` to the sender's name and sets the `assistant`
  option on `jp ticket`, `jp rfd`, and `jp wiki` to `<Name> via jp` (for
  example, `Ada Client via jp`).
- Turns run non-interactively, so tools configured with `run = "ask"` execute
  without prompting.
  Before each turn the bridge reads the configured tools from the host, and the
  turn's `--cfg` disables every one of them before applying the persona, so the
  tools the persona enables by name are the only ones the turn has, whatever a
  user or workspace config enables.
  A turn doesn't start when the tool list can't be read, or holds a tool whose
  name contains a `.`, which a `--cfg` key can't address.
- When `jp serve chat` is stopped, or `signal-cli` exits, during a turn, the
  bridge stops the turn (on a stop) or lets it finish (when `signal-cli` exits),
  checks what it wrote, and only then exits and releases the checkout.

## Group chats

A message that mentions the bridge's account, or replies to one of its messages,
reaches the turn marked `(to you)`, as in `Ada Client (to you): what's next?`.

Without `decide`, every other group message from a configured person becomes a
turn too, and the persona decides whether to answer it.
A turn that answers with exactly `NO_REPLY` sends nothing back, which is how the
assistant stays out of a conversation between people; the typing indicator still
shows while the turn runs, and the turn still costs a model call.

With `decide`, the bridge first asks a small model whether the message is meant
for the assistant, and runs no turn for one that isn't:

```toml
[plugins.command.serve-chat.options.decide]
# The token is read from this environment variable; the bridge refuses to start
# without it.
api_key_env = "JEV_API_KEY"
threshold = 0.5
instructions = """
Should the assistant answer the last message in this group chat? It answers \
questions about the project's work: tickets, RFDs, the wiki, and what is being \
worked on. Answer no for messages between the people, such as thanks or \
small talk."""
```

- Only group messages that don't mention the assistant or reply to it are asked
  about; direct messages and addressed messages always get a turn.
- The model reads the chat's last eight lines, the assistant's answers included,
  so a follow-up such as "why?" can count as meant for it.
  Those lines leave the machine: they go to `decide.url`.
- A message is answered when the probability of "yes" is at least `threshold`.
  When the API fails, the message is answered, and the failure is logged.
- Messages wait for a decision in the order they arrived, so a correction can't
  overtake the message it corrects.
  A correction the model accepts goes into the running turn, like any other
  message from the person the turn runs as.
- The persona's `NO_REPLY` rule still applies to the messages that get through.

Signal mentions may reference the bridge's account by UUID (ACI) instead of
phone number.
Set `signal.account_id` to the bridge account's UUID so those mentions are
recognized (look for `sourceUuid` on messages from the bridge's number in
`signal-cli -o json receive`).

## The checkout

The bridge never commits changes.
Files written by a turn stay uncommitted in the working tree until you pull them
into a local review worktree with `just chat-take`, which then resets the
bridge's working tree.

- A turn holds the lock at `git rev-parse --git-path jp-serve-chat.lock`; `just
  chat-take` takes the same lock, through `take.sh`, which it runs on the
  bridge's host and which stops at the first failing command, before discarding
  anything.
- The clone is pulled (`--ff-only`) before a turn and every `pull_interval`,
  only while the working tree is clean.
- After each turn, every modified or untracked path must be under
  `checkout.writable`.
  If any path is outside those directories, the bridge pauses new turns,
  notifies the chat and the configured `admin`, and waits until the working tree
  is clean again.

Non-empty `git status --porcelain` output indicates uncommitted changes ready to
take.

## Attachments

`signal-cli` saves the files people send (images, documents) in its attachments
directory, `$XDG_DATA_HOME/signal-cli/attachments` by default, or wherever
`signal.attachments_dir` says.
The model can't open them, so the turn reads one line per file instead:

```text
Ada Client: Search crashes like this
[Attached crash.png (image/png), saved on the bridge's machine at /var/lib/jp-bridge/.local/share/signal-cli/attachments/Xd9f2a.png]
```

The assistant can put that path into a ticket, and a maintainer copies the file
off the bridge's machine (`scp`).
Nothing removes old attachments; prune the directory yourself if it grows.
`signal-cli` downloads attachments from anyone who can message the account,
configured or not.

## Setting it up

1. Install `signal-cli` (keep it current: releases older than three months may
   stop working), and register the bridge's own account:

   ```sh
   signal-cli -a +31600000000 register
   signal-cli -a +31600000000 verify 123-456
   ```

2. Clone the repository, and install `jp`, the ticket, RFD, and wiki plugins,
   and this plugin (`JP_INSTALL=1 just _install-serve-chat`).

3. Create a configuration file outside the repository (since it contains phone
   numbers), for example `bridge.toml`:

   ```toml
   [plugins.command.serve-chat]
   run = "allow"
   
   [plugins.command.serve-chat.options]
   persona = "personas/planner"
   admin = "maintainer"
   
   [plugins.command.serve-chat.options.signal]
   account = "+31600000000"
   
   [plugins.command.serve-chat.options.people.maintainer]
   name = "Max Maintainer"
   signal = "+31611111111"
   
   [plugins.command.serve-chat.options.people.client]
   name = "Ada Client"
   signal = "+31622222222"
   ```

   If `persona` is omitted, turns run with the workspace defaults, which do not
   enable any tools.

4. Run the bridge from the clone:

   ```sh
   jp --cfg @/path/to/bridge.toml serve chat
   ```

   Add `-vv` to see the bridge's log; dropped messages, failed turns, and failed
   pulls are logged there.

5. Pull uncommitted changes from the bridge into a local review worktree:

   ```sh
   just chat-take bridge-host src/jp
   ```

## Adding someone

The first time an unconfigured sender writes in a direct chat or group, the
bridge prints their identity and a
`[plugins.command.serve-chat.options.people.<key>]` snippet to allowlist them:

```text
Ada Client (33333333-3333-4333-8333-333333333333) wrote in group:Z3JvdXA=, and isn't configured, …

[plugins.command.serve-chat.options.people.ada-client]
name = "Ada Client"
signal = "33333333-3333-4333-8333-333333333333"
```

Copy the snippet into `bridge.toml`, adjust `name` if needed, and restart the
bridge.
The `signal` field uses the sender's account UUID when available so the
allowlist still matches if the sender hides their phone number.
Each unknown sender is printed at most once per run.

## Options

`rollover` controls when each chat starts a new conversation: `daily` (default)
or a weekday name such as `sunday` for weekly rollover.
`conversation_ttl` must be at least as long as the rollover period (`24h` for
daily, `7d` for weekly).

`cfg` is a list of extra `--cfg` overrides applied after `persona` on every
turn.
Use it to tune turn settings in `bridge.toml` without modifying tracked files in
the bridge's checkout (which would pause the bridge):

```toml
[plugins.command.serve-chat.options]
persona = "personas/planner"
cfg = ["assistant.model.id=haiku", "assistant.model.parameters.reasoning=off"]
```

Overrides in `cfg` are passed on every query, so they take effect on the next
turn even within an existing conversation.

| Option                   | Default                                |
| ------------------------ | -------------------------------------- |
| `persona`                | none                                   |
| `cfg`                    | none                                   |
| `admin`                  | none                                   |
| `rollover`               | `daily`                                |
| `conversation_ttl`       | `7d`, or `14d` with a weekly rollover  |
| `signal.account`         | required                               |
| `signal.account_id`      | none                                   |
| `signal.command`         | `signal-cli`                           |
| `signal.attachments_dir` | `signal-cli`'s attachments directory   |
| `decide.url`             | `https://api.typesafe.ai/v1/systemone` |
| `decide.model`           | `jev-latest`                           |
| `decide.api_key_env`     | `JEV_API_KEY`                          |
| `decide.threshold`       | `0.5`                                  |
| `decide.instructions`    | a generic "is this for the assistant?" |
| `people.<key>.name`      | required                               |
| `people.<key>.signal`    | none                                   |
| `checkout.enabled`       | `true`                                 |
| `checkout.writable`      | `docs/ticket`, `docs/rfd`, `docs/wiki` |
| `checkout.pull_interval` | `5m`                                   |
