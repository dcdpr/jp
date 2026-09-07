# A GitHub re-import leaves a ticket's filename on the old title

- **Status**: Todo
- **Kind**: Bug
- **Authors**: jp
- **Date**: 2026-09-07
- **Implements**: 100

`store::edit` moves a ticket's file to the slug a new title produces, so the
filename never disagrees with the heading.
`store::import` does not: it writes title, description, and comments through
`render::replace_content` directly, because `edit` cannot touch comments.

An issue retitled upstream therefore refreshes into a ticket whose heading is
the new title and whose filename still carries the old slug.
Nothing breaks, but the file is no longer findable by the name it announces,
which is the whole reason `edit` renames.

The fix is to move the slug rule somewhere both paths reach: either a
`store::rename_to_match_title(dir, id)` that `import` calls after writing, or a
shared helper that takes the new title and returns the path the file belongs at.
