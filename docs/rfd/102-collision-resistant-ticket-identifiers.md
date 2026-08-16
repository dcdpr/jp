# RFD 102: Collision-Resistant Ticket Identifiers

- **Status**: Accepted
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-08-15
- **Extends**: [RFD 100]

## Summary

Ticket ids move from a sequential counter to a time-ordered random identifier:
`T-02wt0kx`.
The `docs/ticket/.counter` file is deleted.

## Motivation

`.counter` is a coordination primitive, and ticket creation has no coordination
point.
Parallel git worktrees each see the same counter value and hand out the same
number; once testers file tickets from their own clones, the same happens across
machines.

Renumbering at merge is not a fix.
An id that has reached a pushed commit message cannot be rewritten, and the
assistant references ticket ids in commits and pull request descriptions
routinely.
An id must therefore be final the moment it is created, in a checkout that
cannot see its siblings — which rules out any allocator that counts.

## Format

```
T - 0 2 w t 0 k x
    └───┬───┘ └┬┘
        │      └─── random: 2 × base-32, 1,024 values
        │
        └────────── time: 5 × base-32, zero-padded
                     5-second bucket since 2026-08-10T00:00:00Z
                     33,554,432 buckets, exhausts December 2031
```

|           |                                                                                   |
| --------- | --------------------------------------------------------------------------------- |
| Canonical | `T-02wt0kx`                                                                       |
| Filename  | `docs/ticket/02wt0kx-slug.md`                                                     |
| Alphabet  | Crockford base-32, lowercase: `0123456789abcdefghjkmnpqrstvwxyz`                  |
| Input     | `T-02wt0kx`, `T02wt0kx`, `02wt0kx`, any case; `i`/`l` map to `1`, `o` maps to `0` |
| Expiry    | past the final bucket, allocation refuses rather than wrapping or widening        |
| Ordering  | lexicographic, to the bucket; sequential local creations are always ordered       |

The alphabet excludes `i`, `l`, `o`, and `u` so an id read off a screen during a
call is unambiguous.
Every position is fixed-width and its ASCII order matches its digit order, so
plain string comparison sorts by creation time.

The `T-` prefix makes the id a distinct token: `T-[0-9a-z]{7}` matches nothing
in English prose, where a bare `T` prefix would match *Tuesday* and *Testing*.
That is what makes `git grep` and auto-linking ticket references from RFDs and
other tickets reliable.

Exact subcommand names win over id parsing.
`comment` and `promote` are seven characters that fold onto the alphabet, so
both parse as ids; the bare-id alias applies only when the first argument is not
a subcommand.

**The time component is not a contract.** Nothing may parse it back out of an id
or depend on how ids are generated.
The alphabet and the seven-character width *are* contractual: they appear in
filenames, CLI arguments, `.board.json`, and site routes.

## Allocation

Draw the two random characters uniformly and redraw if the resulting id already
exists in `docs/ticket/`.

When the computed bucket equals the bucket of the highest id already present,
increment the previous id's tail instead of drawing.
Five `ticket_create` calls in one turn execute milliseconds apart and land in
one bucket; incrementing keeps them in creation order, so a ticket that
references an earlier one never sorts above it.
This is the same guarantee ULID gives for same-millisecond generation.
If the tail cannot increment, advance to the next bucket and draw fresh.

If the highest local id sits in a *future* bucket — a ticket merged from a
machine with a fast clock — ignore it and use the computed bucket.
Otherwise one skewed clock drags every subsequent local timestamp forward.

## What changes in RFD 100

|            | Before                 | After                                 |
| ---------- | ---------------------- | ------------------------------------- |
| Id form    | `T0042`                | `T-02wt0kx`                           |
| Filename   | `0042-slug.md`         | `02wt0kx-slug.md`                     |
| Allocation | monotonic counter file | time bucket + random, local monotonic |
| `.counter` | authority              | deleted                               |
| Ordering   | strict                 | to the 5-second bucket                |
| Reuse      | never                  | not guaranteed                        |

A deleted ticket's id is no longer retired.
The local check only sees files that exist, so a later creation in the same
bucket can redraw it, and CI cannot catch that — only one file ever claims the
id.

Everything else in RFD 100 stands: the file format, the metadata block, comment
references (`T-02wt0kx#1`), the board, and GitHub import.

## Drawbacks

Collisions become possible instead of impossible.
Two clones can draw the same id in the same 5-second bucket; at 1,024 values
that is rare.

CI reports the duplicate on the pull request that would introduce it, so the
loser is always an unmerged branch: its commits are still rewritable and every
reference to the id on that branch is unambiguously its own.
`jp ticket refresh` assigns a fresh id and rewrites what the branch introduced
(`git diff main...HEAD`) — not the whole repository, where the token already
means the winning ticket.
This depends on branches being up to date before merging; otherwise both land
and neither CI run ever sees the pair.

`T0042` is nicer to read than `T-02wt0kx`.
That is the price of the property that matters more.

## Alternatives

ULID (26 characters) and UUIDv7 (36) both solve this and are longer than the
whole id proposed here.
A plain random opaque id drops the time component, and with it the ordering that
makes `ls docs/ticket/` and the board read in creation order.

## Implementation Plan

1. **Id, allocation, and migration.** `TicketId` becomes a fixed-width base-32
   newtype with lenient parsing; `store::allocate_id` computes the bucket, draws
   or increments the tail, and no longer reads or writes `.counter`.
   The four existing tickets migrate in the same change: all landed in `e4f8b96`
   at `2026-08-10T08:30:28Z`, which is bucket `005zd`, so they take sequential
   tails `T-005zd00` through `T-005zd03`.
   Grep for the old ids first — `.board.json` carries at least one.
   Splitting this in two leaves the repository in a mixed format needing
   transitional parsing worth more than it saves.
   Commit messages that already name the old ids stay dangling.

2. **Site readers.** `docs/.vitepress/config.mts:188`,
   `loaders/ticket-shared.mjs:29,157`, and the `^# T\d+:` title match all gate
   on the four-digit form and drop every ticket after migration.
   `loaders/rfd-shared.mjs` recognizes ticket links by the old filename shape.

3. **Duplicate detection and repair.** Two files claiming one id is a hard error
   from `store::list` and from the docs build, which already aborts on duplicate
   RFD ids and gains the same check for tickets.
   `jp ticket refresh` assigns a fresh id and rewrites the references its branch
   introduced.

4. **In-flight branches.** `just ticket-migrate` converts any ticket left in the
   old shape on a branch, deriving each bucket from the file's git add-date so
   ordering against tickets already on `main` holds.
   The docs build rejects a `NNNN-slug.md` filename outright, so a branch that
   merges without running it fails rather than losing the ticket silently.

5. **Amend RFD 100.** Id form, filename pattern, allocation, and the non-reuse
   claim.

## References

- [RFD 100] — the ticket format this extends.
- [ULID] — the monotonic same-instant generation rule.

[RFD 100]: 100-in-repo-ticket-tracking.md
[ULID]: https://github.com/ulid/spec
