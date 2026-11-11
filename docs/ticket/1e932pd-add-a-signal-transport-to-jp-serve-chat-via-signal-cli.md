# Add a Signal transport to jp-serve-chat via signal-cli

- **Status**: Done
- **Authors**: jp
- **Date**: 2026-11-05
- **Label**: domain=plugins
- **Label**: type=feature
- **Parent**: T-1e92tft

The first transport: `signal-cli -a <account> jsonRpc` run as a child process of
the plugin, on a dedicated Signal account.
It reads and writes one JSON object per line on stdin and stdout
(`signal-cli-jsonrpc(5)`), the framing the plugin already speaks with the host.

Write the messages it needs from the man pages, with `serde_json`.
Don't copy `signal-cli`'s Rust example client: `signal-cli` is GPLv3 and its
`client/` crate declares no other license, and it targets the daemon's socket or
HTTP endpoint rather than stdio anyway.

- Start it with `--ignore-attachments --ignore-avatars --ignore-stickers
  --ignore-stories`.

- Only a `dataMessage` reaches the bridge; receipts, typing, and sync messages
  are ignored.
  A chat is a group id or a direct sender.

- `sendTyping` while a turn runs, refreshed before its 15-second expiry, and
  stopped when the answer is sent.

- Signal renders no markdown: send plain text, or convert emphasis and code to
  `textStyle` ranges, which count UTF-16 code units.
  Split a long answer into several messages.

- A `signal-cli` that exits ends the plugin with a reason rather than leaving a
  bridge that receives nothing.

- [x] Tests on recorded JSON-RPC lines: an envelope from an allowed sender
      becomes a message, every other envelope kind is dropped, a long answer is
      split, styles land on the right UTF-16 ranges
