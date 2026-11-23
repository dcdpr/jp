# Add jp doctor

- **Status**: In Progress
- **Authors**: jp
- **Date**: 2026-11-18
- **Label**: client=cli
- **Label**: package=jp_cli
- **Label**: type=feature
- **Parent**: T-1n2tnwv

Check the `jp` installation, user-global and workspace configuration, and
conversation storage, reporting each problem grouped by section, and apply a fix
only when the user names it.

Plain `jp doctor` is read-only.
It skips the steps every other command runs before its own code (the one-time
trash restore, the startup scan and its cleanup of leftover `.tmp` files and
import staging directories, the scan-cache write), so it reports the store as it
is rather than as those steps leave it.
`jp doctor --fix <name>` applies one named fix to every finding it covers, and
writes nothing else.

Checks and fixes live with the data they cover: conversation checks and fixes
behind `RecoveryBackend` in `jp_storage`, workspace checks in `jp_workspace`,
config checks in `jp_config`.
Each check returns typed findings; a finding with a fix carries the fix's name.
`jp doctor` runs the checks, groups the findings, and applies the fixes it is
told to, with no fixing logic of its own.

- [x] `jp doctor` runs without the pre-run step: no trash restore, no startup
      scan, no load-failure report, and nothing written to disk
- [x] Findings are typed and grouped by section; each section prints the exact
      `jp doctor --fix <name>` command for the findings it can fix, and a
      suggestion (a command or an edit) for those it cannot
- [x] `jp doctor --fix <name>` applies that fix to every finding it covers: asks
      once before writing, takes each conversation's lock, skips one another
      process holds and names it in the error the run ends with, and only
      reports what it would do under `--no-persist`
- [x] Fix `rebuild-metadata`: rebuilds a missing `metadata.json` from the events
      and the directory name
- [x] Reports leftover `.tmp` files and import staging directories, which the
      startup scan of any other command removes
- [ ] Validates the archive partition (`conversations/.archive/`), which startup
      validation never reads, and reports archived conversations that fail to
      load
- [x] Lists directories under `conversations/` whose name is not a conversation
      ID (`ValidationResult::unrecognized`), which the load-failure report
      leaves out, suggesting `rm -r <path>` for each
- [x] The load-failure report printed after every command
      (`jp_cli::load_failure::render_text`) lists at most 5 failures again, and
      above that ends with "Run `jp doctor` to list them"
- [ ] Workspace and config sections: start with the checks the conversation
      section's structure makes easy (missing or malformed workspace ID, config
      files that fail to parse), and leave the rest to follow-ups

## Comments

-----

- **From**: jp
- **Date**: 2026-11-23T23:11:20Z

Removed the two unsent-draft items.
A conversation with no turns and a query draft beside it loads fine: it is not
an error or a warning, so `jp doctor` does not report it.
Such conversations show in `jp c ls` like any other, and `jp c rm` removes them.
