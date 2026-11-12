# Add a Signal transport to jp-serve-chat via signal-cli

- **Status**: Done
- **Authors**: jp
- **Date**: 2026-11-05
- **Label**: domain=plugins
- **Label**: type=feature
- **Parent**: T-1e92tft

Implement the Signal transport by spawning `signal-cli -a <account> jsonRpc` as
a child process using a dedicated Signal account.
In JSON-RPC mode, `signal-cli` reads requests from stdin and writes responses
and notifications as JSON lines on stdout (`signal-cli-jsonrpc(5)`).

Build the JSON-RPC messages directly with `serde_json` from the
`signal-cli-jsonrpc(5)` specification rather than copying `signal-cli`'s GPLv3
example client.

- Start it with `--ignore-attachments --ignore-avatars --ignore-stickers
  --ignore-stories`.

- Only a `dataMessage` reaches the bridge; receipts, typing, and sync messages
  are ignored.
  A chat is a group id or a direct sender.

- `sendTyping` while a turn runs, refreshed before its 15-second expiry, and
  stopped when the answer is sent.

- Signal does not render Markdown: convert headings, bold text, and code spans
  into plain text plus `textStyle` UTF-16 ranges, and split long responses
  across multiple messages.

- If `signal-cli` exits, shut down the plugin with an error rather than leaving
  the bridge running without a transport.

- [x] Tests on recorded JSON-RPC lines: an envelope from an allowed sender
      becomes a message, every other envelope kind is dropped, a long answer is
      split, styles land on the right UTF-16 ranges
