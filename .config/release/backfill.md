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

1. Read the list of changes first and group them by feature: the command, flag,
   setting, or behavior they add, change, or fix.
   Group all commits that build a single feature together with any follow-up
   fixes and reworks in the same release.
   Use commit subjects and file lists to form initial groups, and inspect diffs
   when a commit's scope is unclear.
2. For each group, inspect the diffs with `git_diff_commit` (a few files at a
   time) and read the resulting code in the release tree.
   Skip files marked `generated` or `large` unless necessary.
   Treat commit messages as hints and verify behavior against the code.
3. Decide whether the group's final state at the end of the release is
   user-visible.
   Omit tests, CI, builds, internal documentation, RFDs, tickets, JP's own
   development tooling (personas, `.config/jp/tools`, the justfile), and
   features that were both added and removed within the same release.
   When in doubt, write a fragment.
4. Before writing a fragment, name in one line the files and functions that show
   what it describes, so it can be checked against them.
5. Write one fragment per group with `changelog_add`: one or two short sentences
   naming the feature and what it lets people do, leaving its flags and options
   to its guide page.
   Its `credits` name the changes in the group that built or changed the
   feature, copied from their headings; leave out a change that only touched it
   in passing, such as one that made many features usable at once.
6. If a commit belongs to a fragment you already wrote, update that fragment
   with `fs_modify_file` instead of creating a duplicate: describe the feature's
   final state in the release and append a `- **Credits**:` line for the commit
   at the top of the file.
   If a fragment is superseded by another fragment that now includes its
   credits, delete the redundant file with `fs_delete_file`.

When you have worked through every change, list the ones you omitted, one line
each with the reason.
