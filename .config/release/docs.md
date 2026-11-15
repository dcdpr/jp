Write JP's user guide as JP {version} ships it.
The release's changes reached JP's `main` between {from} and {until}; the ones
this conversation is about are at the end of this message.

{code}

The user guide is the part of JP's wiki under the page titled "User Guide", in
`{wiki}/`.
The wiki tools work on {version}'s wiki: read it with `wiki_list` and
`wiki_show`, and change it with `wiki_create` and `wiki_edit`.
When no page is titled "User Guide", create it first, as a hub.

1. Read the changes: their diffs with `git_diff_commit`, a few files at a time,
   and the release's code where a diff doesn't show how a feature is used.
   Release notes and commit messages are hints, and can be wrong.
2. Find the guide pages the changes affect.
   Update them, or write a page under the User Guide for a feature no page
   covers yet.
   Describe each feature as {version} ships it, not as JP does today.
3. Finish with the step the section at the end of this message asks for.
