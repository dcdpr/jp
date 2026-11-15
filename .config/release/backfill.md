Draft the release notes of JP {version}: one changelog fragment for each feature
or fix that people using JP would notice, describing it as {version} ships it.
The changes reached JP's `main` between {from} and {until}, and are listed
oldest first at the end of this message.

JP is a command-line tool for working with LLMs in a terminal: `jp query` asks a
model, conversations keep the history, and tools, attachments, configuration,
and plugins extend it.

{previous}

{code}

The notes describe {version} as a whole, not how it was built: a feature that
changed several times within the release gets one fragment, describing where it
ended up.

1. Read the list of changes first, and group them by what they work on: the
   command, flag, setting, or behavior they add, change, or fix.
   The changes that build one feature, with the fixes and reworks it got along
   the way, are one group.
   Their subjects and file lists show most groups; read a diff where they don't.
2. For each group, read what its changes did with `git_diff_commit`, a few files
   at a time, and the code they leave behind in the release's code.
   Files marked `generated` or `large` in the list rarely show what users see;
   read them only when nothing else does.
   Commit messages are hints, and can be wrong.
3. Decide whether people using JP would notice the group's work as it stands at
   the end of the release.
   Tests, CI, builds, internal documentation, RFDs, tickets, and JP's own
   development tooling (personas, `.config/jp/tools`, the justfile) aren't
   noticed, and neither is a feature added and removed again within the release.
   When unsure, write the fragment.
4. Before writing a fragment, name in one line the files and functions that show
   what it describes, so it can be checked against them.
5. Write one fragment per group with `changelog_add`, with `credits` naming
   every change in the group, copied from their headings.
6. When a change turns out to belong to a fragment you have written already,
   revise that fragment with `fs_modify_file` instead of writing another:
   describe the feature as it ends up, and add a `- **Credits**:` line for the
   change to the ones at the top of the file.
   Delete a fragment that no longer stands on its own with `fs_delete_file`,
   once its credits are in the fragment that covers it.

When you have worked through every change, list the ones you wrote no fragment
for, one line each with the reason.
