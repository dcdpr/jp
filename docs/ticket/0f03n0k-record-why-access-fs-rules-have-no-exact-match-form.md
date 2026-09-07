# Record why access.fs rules have no exact-match form

- **Status**: Todo
- **Kind**: Chore
- **Authors**: jp
- **Date**: 2026-09-07
- **Implements**: 076
- **Label**: package=jp\_tool
- **Label**: type=task

Every `access.fs` rule is a recursive prefix: a rule at `docs/ticket` covers the
directory and everything under it, and there is no way to scope a rule to
exactly one path.
So "you may write files in `docs/ticket` but never remove the directory itself"
cannot be said.

This came up while closing the hole where moving `docs` walked past a deny on
`docs/ticket`.
An entry-versus-contents axis looked like the fix and is not: the matcher only
looks up the tree from the target, so a deny at `docs/ticket` is never consulted
when the target is `docs`, whatever capabilities the rule carries.
What closed that hole was asking the policy about the subtree, which is now [RFD
076]'s "Operations that reach a subtree".

The repository's own `.ignore` is the argument against adding the axis anyway.
Gitignore needs the directory entry un-ignored before its contents can be, so
the whitelist there spells out both (`!apps/`, then `!apps/**`) fourteen times
over, and the doubling exists because git prunes the walk, not because anyone
wanted the distinction.
Forgetting the entry line under-includes silently.
In a deny-oriented language the same slip leaves a subtree governed by a broader
rule, which is a grant nobody intended.

So: no exact-match rules until something actually needs to say the sentence
above.
This ticket exists so the next person to notice the gap finds the reasoning
instead of rediscovering it, and closes as soon as a real use case turns up to
argue with.

[RFD 076]: ../rfd/076-tool-access-grants.md
