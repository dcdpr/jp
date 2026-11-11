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
- One turn runs at a time.
  If another message arrives from the same chat while its turn is running, the
  bridge injects it into the active turn as a reply (like `[r] Reply` in the
  terminal).
  Messages from other chats, or messages that arrive after the turn finishes,
  are queued and batched into the chat's next turn.
- Each turn sets `user.name` to the sender's name and sets the `assistant`
  option on `jp ticket`, `jp rfd`, and `jp wiki` to `<Name> via jp` (for
  example, `Ada Client via jp`).
- Turns run non-interactively, so tools configured with `run = "ask"` execute
  without prompting.
  The tools enabled by the persona are the only permission boundary.

## Group chats

In a group, every message from a configured person becomes a turn, and the
persona decides whether to answer it.
A message that mentions the bridge's account, or replies to one of its messages,
reaches the turn marked `(to you)`, as in `Ada Client (to you): what's next?`.
A turn that answers with exactly `NO_REPLY` sends nothing back, which is how the
assistant stays out of a conversation between people; the typing indicator still
shows while the turn runs.

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
  chat-take` takes the same lock.
- The clone is pulled (`--ff-only`) before a turn and every `pull_interval`,
  only while the working tree is clean.
- After each turn, every modified or untracked path must be under
  `checkout.writable`.
  If any path is outside those directories, the bridge pauses new turns,
  notifies the chat and the configured `admin`, and waits until the working tree
  is clean again.

Non-empty `git status --porcelain` output indicates uncommitted changes ready to
take.

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
| `people.<key>.name`      | required                               |
| `people.<key>.signal`    | none                                   |
| `checkout.enabled`       | `true`                                 |
| `checkout.writable`      | `docs/ticket`, `docs/rfd`, `docs/wiki` |
| `checkout.pull_interval` | `5m`                                   |
