# Govern attachment handlers with an access policy

- **Status**: Todo
- **Authors**: jp
- **Date**: 2026-11-25
- **Label**: package=jp_attachment
- **Label**: package=jp_attachment_file_content
- **Label**: package=jp_cli
- **Label**: package=jp_config
- **Label**: package=jp_tool
- **Label**: relevant-rfd=076
- **Label**: type=pre-rfd

Tools resolve every path through an `access` block (RFD 076): `access.fs` rules,
compiled by `jp_cli::access::compile_fs` and enforced by
`jp_tool::access::resolve_path`.
Attachment handlers have no equivalent.
The only thing between an attachment and the filesystem, the network, or a
subprocess is whatever each handler checks for itself.

What each handler does today:

- `file`: confines reads to the workspace with a lexical `strip_prefix` in
  `build_attachment`.
  That check passes symlinks that point outside the workspace, and paths that
  decode to `..` (found in review of #1393). #1393 replaces it with
  `resolve_path(root, path, None, Resolve::Target)`, so `file` attachments are
  workspace-only with no exceptions.
- `cmd`: runs any command the URL names.
- `http`/`https`: fetches any host.
- `mcp`, `gh`, `bear`, `ag`: reach whatever their client is configured for.

Attachments don't only come from `--attach` typed at the terminal.
They also come from committed config files (`.jp/config.toml`, `.jp.toml`
chains, persona and skill files that a pulled branch can change), from stored
conversation deltas, and from `--cfg` edits.
D43 rejected putting attachments under `access.fs` ("Unified attachment and tool
access policy") on the grounds that typing `--attach` is the consent action.
That reasoning covers the CLI, not attachments loaded from a file the user
didn't write.

## Questions for the RFD

- Does one `access` block cover attachments, reusing `AccessPolicy` and its
  `fs`/`net`/`env` axes, or does each handler declare which axes it uses?
- Where does it live: `conversation.attachments.access`, per handler type, or
  per attachment entry?
  How does it layer with workspace and user config?
- Can workspace config grant an attachment access outside the workspace, or does
  only user config (or an approval, as with external `access.fs` rules) allow
  that?
  How does this relate to D56's `external:` attachments and D03?
- What is the default with no block: today's behaviour (workspace-only for
  `file`, unrestricted for the rest), or default-deny for config-sourced
  attachments that need network or process access?
- How does `Handler` receive the policy: an argument to `add`/`get`, or a
  context struct like `jp_tool::Context`?

Out of scope: workspace confinement for `file`, which is a bug fix and lands in
#1393 with `access: None`.
The RFD would pass `Some(policy)` to the same resolver, so that fix is its
foundation.
