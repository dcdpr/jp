Write a highlight for the release notes of JP {version}: a short story about the
change at the end of this message, for people deciding whether to upgrade.
The release's changes reached JP's `main` between {from} and {until}.

{code}

The user guide for {version} is the part of the wiki under the page titled "User
Guide", in `{wiki}/`.
Read it with `wiki_list` and `wiki_show`, and leave it as it is.

1. Read the change: its diffs with `git_diff_commit`, the release's code, and
   the guide's page about it.
   Release notes and commit messages are hints, and can be wrong.

2. Write the highlight with `fs_create_file` as
   `{fragments}/{version}/highlights/<slug>.md`, where `<slug>` is its title in
   lowercase words joined by hyphens:

   ```markdown
   # <Title>
   
   - **Authors**: <author>
   - **Credits**: <credit>
   
   <What the feature lets people do and why that's useful, in a few short
   paragraphs, with a short example of using it, and what comes next for it
   when the release's RFDs or tickets plan it.>
   ```

   Give it an `Authors` line for each author and a `Credits` line for each
   credit listed at the end of this message, and describe the feature as
   {version} ships it.
   When a guide page covers the feature, link it as `[the guide](<link>)`, where
   `<link>` is `{guide}/` and the page's file name without `.md`.
