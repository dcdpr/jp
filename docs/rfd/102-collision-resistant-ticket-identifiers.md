# RFD 102: Collision-Resistant Ticket Identifiers

- **Status**: Implemented
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-08-15
- **Extends**: [RFD 100]
- **Summary**: Replaces sequential ticket counters with collision-resistant,
  time-bucketed Crockford base-32 ticket IDs (`T-02wt0kx`).

## Summary

This RFD replaces sequential ticket counters (`docs/ticket/.counter`) with
collision-resistant, time-ordered Crockford base-32 identifiers (`T-02wt0kx`) so
tickets created across parallel worktrees and clones never collide.

## Motivation

`.counter` requires a coordination point that ticket creation does not have.
Parallel git worktrees read the same counter value and assign the same number,
and separate clones do the same across machines.

Renumbering on merge does not work either.
The assistant routinely references ticket IDs in commit messages and pull
request descriptions, and IDs in pushed commits cannot be rewritten.
A ticket ID must be final at creation time without coordinating with other
checkouts, which rules out sequential counters.

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
| Expiry    | past the final bucket, allocation fails rather than wrapping or widening          |
| Ordering  | lexicographic by bucket; sequential local creations are always ordered            |

The alphabet excludes `i`, `l`, `o`, and `u` so an ID read off a screen during a
call is unambiguous.
IDs are fixed-width and the alphabet's ASCII order matches its value order, so
plain string comparison sorts by creation time.

The `T-` prefix makes the ID a distinct token: `T-[0-9a-z]{7}` does not match
English prose, whereas a bare `T` prefix would match words like *Tuesday* and
*Testing*.
This makes `git grep` and auto-linking ticket references in RFDs and other
tickets reliable.

Exact subcommand names take precedence over ID parsing.
Because `o` maps to `0`, the seven-character subcommands `comment` and `promote`
also parse as bare IDs; the CLI treats the first argument as a bare ID only when
it does not match a subcommand.

## Storing the ID Only in the Filename

A ticket's ID lives only in its filename, never inside the document: the heading
is `# Title`, and a reply header is `- **Re**: #1` rather than `- **Re**:
T-02wt0kx#1`.

Storing the ID in both the filename and the document body lets the two copies
drift out of sync, especially when someone resolves a merge conflict by hand
before running collision repair.
With the ID in the filename alone, `jp ticket refresh` and `jp ticket migrate`
are simple file renames that do not need to rewrite the file contents.

This differs from RFDs, which carry their number in both the filename and the `#
RFD 102:` heading.
RFD numbers are permanent by policy and rarely change, whereas ticket IDs are
reassigned whenever a collision is resolved.

The trade-off is that a ticket's contents carry no ID when pasted into a chat or
quoted in an issue, and `git grep T-02wt0kx` finds references to the ticket
rather than the ticket file itself.

**The time component is not a contract.** Callers must not extract the timestamp
from an ID or depend on how IDs are generated.
Only the alphabet and the seven-character width are contractual, as they appear
in filenames, CLI arguments, `.board.json`, and site routes.

## Allocation

To allocate an ID, compute the current 5-second time bucket, draw the
two-character random tail uniformly, and redraw if the resulting ID already
exists in `docs/ticket/`.

When the computed bucket equals the bucket of the highest existing local ID,
increment that ID's tail instead of drawing randomly.
Multiple `ticket_create` calls in one turn execute milliseconds apart and land
in the same bucket; incrementing the tail keeps them in creation order, so a
ticket that references an earlier one never sorts above it.
This matches ULID's monotonic generation rule for same-millisecond timestamps.
If the tail overflows, advance to the next bucket and draw a fresh random tail.

If the highest local ID sits in a future bucket (for example, from a ticket
merged from a machine with a fast clock), ignore it and use the computed bucket.
Otherwise, a single skewed clock would drag every subsequent local timestamp
forward.

## Changes to RFD 100

|            | Before                 | After                                 |
| ---------- | ---------------------- | ------------------------------------- |
| ID form    | `T0042`                | `T-02wt0kx`                           |
| Filename   | `0042-slug.md`         | `02wt0kx-slug.md`                     |
| Allocation | monotonic counter file | time bucket + random, local monotonic |
| `.counter` | authority              | deleted                               |
| Ordering   | strict                 | to the 5-second bucket                |
| Reuse      | never                  | not guaranteed                        |
| Heading    | `# T0042: Title`       | `# Title`                             |
| Reply      | `- **Re**: T0042#1`    | `- **Re**: #1`                        |

Deleting a ticket no longer retires its ID.
Because the local check only sees files that currently exist in `docs/ticket/`,
a later ticket created in the same bucket can draw the deleted ID again, and CI
cannot catch the reuse because only one file claims the ID at a time.

Everything else in RFD 100 remains unchanged: the file format, the metadata
block, comment references (`T-02wt0kx#1`), the board, and GitHub import.

## Drawbacks

Collisions are possible, though rare: two clones creating a ticket in the same
5-second bucket have a 1-in-1,024 chance of drawing the same ID.

CI reports the duplicate on the pull request that introduces it.
Because the colliding ticket is always on an unmerged branch, its commits are
still rewritable and every reference to the ID on that branch belongs to the new
ticket.
`jp ticket refresh` assigns a fresh ID and rewrites only the references
introduced on the branch (`git diff main...HEAD`), leaving the rest of the
repository pointing to the already-merged ticket on `main`.
This requires branches to be up to date with `main` before merging; otherwise,
both pull requests can pass CI independently and merge the duplicate ID.

Finally, `T-02wt0kx` is harder to read than `T0042`, which is the trade-off for
decentralized allocation.

## Alternatives

ULID (26 characters) and UUIDv7 (36 characters) both provide time-ordered,
collision-resistant IDs, but they are much longer than the nine-character ID
proposed here.
A purely random opaque ID drops the time component, losing the chronological
sorting that keeps `ls docs/ticket/` and the board in creation order.

## Implementation Plan

1. **ID, allocation, and migration.** Make `TicketId` a fixed-width base-32
   newtype with lenient parsing.
   Update `store::allocate_id` to compute the bucket, draw or increment the
   tail, and stop reading or writing `.counter`.
   Migrate the four existing tickets in the same change: all four landed in
   `e4f8b96` at `2026-08-10T08:30:28Z` (bucket `005zd`), so they take sequential
   tails `T-005zd00` through `T-005zd03`.
   Grep for and update in-tree references to the old IDs (including
   `.board.json`); references in existing commit messages stay dangling.
   Migrating in the same commit avoids supporting both formats at once.

2. **Site readers.** Update `docs/.vitepress/config.mts:188`,
   `loaders/ticket-shared.mjs:29,157`, and the `^# T\d+:` title regex, which
   currently gate on the four-digit form and would otherwise drop every ticket
   after migration.
   Update `loaders/rfd-shared.mjs` to recognize ticket links by the new filename
   shape.

3. **Duplicate detection and repair.** Make duplicate ticket IDs a hard error in
   `store::list` and in the docs build, matching the existing duplicate check
   for RFD IDs.
   Add `jp ticket refresh` to assign a fresh ID and rewrite the references
   introduced on the current branch.

4. **In-flight branches.** Add `just ticket-migrate` to convert tickets left in
   the old format on a branch, deriving each bucket from the file's git add-date
   so ordering against tickets already on `main` holds.
   Make the docs build reject `NNNN-slug.md` filenames outright so a branch
   merged without running migration fails CI rather than losing the ticket
   silently.

5. **Amend RFD 100.** Update the ID form, filename pattern, allocation rules,
   and non-reuse claim.

## References

- [RFD 100]: The ticket format this RFD extends.
- [ULID]: Specification for the monotonic same-instant generation rule.

[RFD 100]: 100-in-repo-ticket-tracking.md
[ULID]: https://github.com/ulid/spec
