Draft the changelog fragments of JP {version}: one for each change listed below
that people using JP would notice.
The changes reached JP's `main` between {from} and {until}, and are listed
oldest first at the end of this message.

JP is a command-line tool for working with LLMs in a terminal: `jp query` asks a
model, conversations keep the history, and tools, attachments, configuration,
and plugins extend it.

{code}

For each change, oldest first:

1. Read what it changed with `git_diff_commit`, a few files at a time.
   Files marked `generated` or `large` in the list rarely show what users see;
   read them only when nothing else does.
2. Where the diff doesn't show what users see, read the code around it in the
   release's code: the command it adds, the flag it reads, the message it
   prints.
   The commit message is a hint, and can be wrong.
3. Decide whether people using JP would notice the change.
   Tests, CI, builds, internal documentation, RFDs, tickets, and JP's own
   development tooling (personas, `.config/jp/tools`, the justfile) aren't
   noticed.
   When unsure, write the fragment.
4. Before writing a fragment, name in one line the files and functions that show
   the change, so it can be checked against them.
5. Write the fragment with `changelog_add`, with `credits` naming every listed
   change it describes, copied from their headings.
   Several changes that build one feature share one fragment.

When you have worked through every change, list the ones you wrote no fragment
for, one line each with the reason.
