# Add a workspace section to jp doctor

- **Status**: Todo
- **Authors**: jp
- **Date**: 2026-11-24
- **Label**: client=cli
- **Label**: package=jp_cli
- **Label**: package=jp_workspace
- **Label**: type=enhancement
- **Parent**: T-1n2tnwv

A `## Workspace` section in `jp doctor`'s report, starting with the workspace
ID: a missing or malformed `.jp/.id`.

The check reads the ID file itself and lives in `jp_workspace`, returning typed
findings the way `RecoveryBackend::diagnose` does for conversations.
It cannot rely on opening the workspace: `Workspace::open_read_only` replaces a
malformed ID with a random one today (T-055mxpv), and once that is fixed,
opening fails instead.
Either way doctor must still run and report the problem.

A finding without a fix suggests the edit or command that deals with it, as the
conversation section does.

- [ ] Reports a missing or malformed workspace ID, with the file's location
- [ ] Runs when the ID is malformed, without minting or writing an ID
- [ ] Rows use the report's table: target, problem, command
