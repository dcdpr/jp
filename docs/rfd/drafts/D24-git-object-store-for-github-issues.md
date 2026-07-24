# RFD D24: Git Object Store for GitHub Issues

- **Status**: Draft
- **Category**: Design
- **Authors**: rgrant <rgrant@contract.design>
- **Date**: 2026-07-19

## Summary

This RFD describes a local-first store for GitHub issues, kept as git objects in
the project's own `.git` under a dedicated ref namespace.
A scraper tool mirrors issues and their comments from GitHub through
`jp_github`; contributors sync the store with ordinary push and pull.
Each issue is a set of per-writer, append-only operation logs; issue state is
computed deterministically from their union, without merge conflicts and without
dropped data.

## Motivation

Contributors need offline access to the project's issues, and later the ability
to edit them locally (`issue append`) and build views over them (a kanban tool).
The consumers are our own jp-tools: `issue show` first, `issue append` and a
kanban view later.

In this version of the design, GitHub is an ingest source only: issues filed
there are scraped into the local store, but the tooling never writes back to
GitHub.
It is not the authority.
The local store is the source of truth, designed from day one for a world where
local edits are made concurrently on machines that do not know about each other.

Three approaches fail the requirements:

- **Checked-in files** cause merge conflicts for people working in worktrees,
  and dirty the code's evolution with their own commit history.
  They also limit the view of open issues available from branches.
- **A centralized per-user store** mixing all repositories (as radicle's
  `/storage/` does) is rejected for security reasons - it would require
  inventing a DSL for access control.
- **Last-write-wins merging** (git-bug's approach, in elaborated form) hides
  conflicts by silently dropping the losing write.

Storing issues as git objects in the repo's own `.git` — objects in the object
database, refs under a dedicated namespace, nothing in the worktree — avoids
all three.
A contributor's responsibility is to pull; after that they have everything they
need for offline work.

## Design

### What the user sees

```sh
# sync: fetch everyone's issue refs, push every issue ref you hold
jp-tools issue sync

# read a folded issue, offline
jp-tools issue show 42

# poke at the raw store with stock git
git for-each-ref refs/jp/issues/42/
git log refs/jp/issues/42/<writer-id>
```

Daily git work is unaffected: `git log` (HEAD), `git branch`, and default clones
never see the store.
The sync tool configures the `refs/jp/issues/*` fetch refspec.
Documentation explains the git integration using refs, and how to edit the git
config manually.

### Store layout

The store lives inside the `.git` of the repository whose issues it holds.
Each issue has one ref per writer:

```
refs/jp/issues/<number>/<writer-id>
```

- `<number>` is the GitHub issue number, or — in the later local-creation phase
  — a `jp_id`-formatted id for an issue born locally; the two forms are
  syntactically disjoint.
  The literal `meta` is reserved for the store-level metadata log.
  The store holds the repo's own issues only; a cross-repo mirror would get a
  sibling namespace and is out of scope.
- `<writer-id>` identifies one replica: one checkout of the repository — a
  clone, or a linked git worktree inside a clone — held by one signer.
  It is composed of two segments,
  `<unique-hash-of-signing-key>-<avatar-nickname>/<replica-id>`:
  the key hash binds the ref to a signing key (the fold rejects commits under a
  key-hash segment that are not signed by the matching key); the avatar nickname
  is a human-readable label with no authority; the replica id is a
  `jp_id`-formatted id minted per checkout on first write, stored in that
  checkout's own state under `.git/`, never checked in and never synced.
  The replica id exists because one signer works from several checkouts at
  once: if the signing key alone named the writer, two checkouts holding the
  same key would append to the same ref independently, and one of the two
  pushes would fail to fast-forward.
  A replica id per checkout gives each checkout its own chain, so every push
  stays a fast-forward.
  Every checkout of every clone is its own writer.

A ref is only ever written by its owner — one replica — so no two replicas
ever contend on a ref.
Every push is a fast-forward, and no merge commit exists anywhere in the store.

### What each commit contains

Each write is one standard commit object, used as an envelope:

- **tree**: a single blob, `ops.json` — the payload.
  It carries the operations of this write, a `seen_heads` field (defined below),
  and a format version.
  Tools read and write `ops.json` and nothing else; its versioned schema is the
  compatibility boundary between replicas, and changing it is a breaking change
  for every clone that holds copies.
- **parent**: the previous commit on this writer's chain (none for the first).
  Exactly one parent, always: every ref is a strictly linear chain.
- **author/committer**: the writer id as name, write timestamp.
- **message**: a one-line summary generated by the tool, e.g. `observe #42:
  set-state closed, add-comment 2140518390`.
  No human ever writes one; it exists so `git log` stays legible when debugging
  the store.
  Never parsed.

Causality between writers is recorded in the `ops.json` payload; commits never
have multiple parents.
`seen_heads` maps each foreign writer id to the newest op `id` this writer had
incorporated from that chain at write time.
The resolution protocol built on it is specified in the section "Computing issue
state".
Acknowledgments double as tamper evidence: a writer who rewrote history to drop
ops would leave other writers' `seen_heads` pointing at op ids that no longer
exist.
The fold checks for exactly this dangling reference; see the section "Computing
issue state".

### Operations

Operations are fine-grained, one vocabulary for scraped and (later) local
writes.
Each op carries a stable `jp_id`-formatted `id`, minted at creation; the id is
what `seen_heads` acknowledgments reference.

| Op | Target | Fold semantics |
| --- | --- | --- |
| `set-title` | issue | multi-value register |
| `set-body` | issue | multi-value register |
| `set-state` | issue | multi-value register |
| `add-label` / `remove-label` | issue | observed-remove set |
| `add-comment` | comment (by GitHub id) | grow-only set |
| `set-comment-body` | comment (by GitHub id) | multi-value register |
| `set-comment-visibility` | comment (by GitHub id) | multi-value register |
| `set-priority` | issue | multi-value register |
| `tombstone-issue` | issue | tombstone |
| `tombstone-comment` | comment (by GitHub id) | tombstone |
| `set-allowed-labels` | store metadata | multi-value register |

A register is one field of one target: ops of the same type on the same target
address the same register.
The fold semantics named in the table are defined in the section
"Computing issue state", after the causal order they depend on.

`set-comment-visibility` records comment collapsing — GitHub's
comment-minimization feature — as a register holding `visible` or `collapsed`
with a reason (off-topic, outdated, resolved, spam, ...).
Collapse is display muting, not deletion: the fold renders a collapsed comment
as a marker carrying its reason, and un-collapsing is an ordinary later write to
the same register.
The scraper emits this op when it observes a comment's minimized state change
upstream.

### Computing issue state

Issue state is computed by a *fold*: collecting the ops from every writer's log
and applying them in causal order.

**Causal order.** *Happens-before* is the smallest transitive relation built
from two edge kinds:

1. Chain order: an op happens-before every later op in its own chain.
2. Acknowledgment: when op C's `seen_heads` maps writer W to op id X, then X and
   every op before X in W's chain happen-before C.

Two ops are **concurrent** when neither happens-before the other.
This relation is the only input to conflict resolution.

**Resolution**, per the fold semantics in the op table:

- **Multi-value register** (title, body, state, comment body, priority): the
  register's state is the set of ops on it that do not happen-before another op
  on the same register.
  One op in that set yields a single value — the normal case.
  Two or more ops in that set — possible only for concurrent writes — are all
  part of the register's state, and `issue show` renders every one, ordered by
  op id.
  The register holds a single value again only when a later write acknowledges
  every op in that set — that write happens-after each of them, and the fold
  resolves the register to it.
  A resolving write is an ordinary write, so two concurrent resolving writes
  produce a new conflict containing only the resolving values; the same rule
  applies until one write acknowledges every live value.
- **Observed-remove set** (labels): a label is present when some `add-label` op
  for it does not happen-before a `remove-label` op for it.
  A remove affects only adds it acknowledged; a concurrent add survives.
  A removed label can be re-added by a later `add-label` op; removal is never
  permanent (the distinction from a two-phase set, where removal is final).
- **Grow-only set** (comments): the union of `add-comment` ops.
- **Tombstone** (deletes): once present, every fold hides the target
  from that point on; concurrent edits to the target stay in the log but are not
  displayed.
  Hiding is the entire mechanism; the bytes are never removed from the store.
  A tombstone is final: no op un-hides a tombstoned target.
  When a tombstone turns out to be wrong, the original author resubmits the
  content as a new issue or comment.

**Missing acknowledgments.** Before applying the resolution rules, the fold
checks every `seen_heads` reference.
A `seen_heads` entry can name an op id that exists nowhere in the local store.
A missing op id has two possible causes: a writer rewrote a chain and dropped
the op, or the local replica has not yet fetched the commits that carry the op.
No local check can tell the two causes apart, because sync is pairwise and
asynchronous: a replica can legitimately receive an acknowledgment of an op
before receiving the op.
The fold stops with a typed error naming the writer and the missing op id,
renders nothing, and tells the user to fetch from more remotes.
Fetching cures the innocent cause.
When fetching from every available remote still leaves the op id missing, or
when a refused non-fast-forward fetch or a ref-journal entry points at a
specific chain, the cause is a rewritten chain (see the section "Attack
Analysis: Chain Rewrite").
Running with `--fix-interactive` walks through resolution, including the option
to drop the offending ref.
A fresh clone has no prior ref values to compare against, so a fresh clone
detects rewrites only through missing acknowledgments, and a missing
acknowledgment is grounds to investigate, not proof of a rewrite.

**Procedure:**

1. Enumerate heads: `git for-each-ref refs/jp/issues/<number>/`.
2. Walk each linear chain, collect ops.
3. Run the missing-acknowledgment check; compute happens-before; apply the
   resolution rules.

The fold is order-independent across replicas: any two clones that have seen the
same commits compute the same state, regardless of how or in what order the
commits arrived.

### Sync

`issue sync` is fetch plus push, nothing else:

1. Fetch `refs/jp/issues/*` with a non-forcing refspec: every chain is
   append-only, so every legitimate update is a fast-forward.
   A non-fast-forward foreign ref means someone rewrote history; the tool
   refuses the update, keeps its local copy, and reports the ref.
2. Push every `refs/jp/issues/*` ref the local replica holds: the local writer's
   own refs, plus all foreign refs picked up by fetching.
   Pushing foreign refs spreads every writer's chain to every remote.
   If sync pushed only the local writer's refs, then a remote could be missing
   some writer's chain forever, because no contributor is obligated to push
   another writer's chain to that remote.
   A clone made from a remote that is missing a chain cannot compute issue
   state: surviving chains hold `seen_heads` entries that name op ids inside the
   missing chain, the fold cannot find those op ids, and the fold stops with an
   error.
   Pushing foreign refs is safe: each chain has exactly one writer and only
   grows, so two replicas pushing the same ref push the same tip, or one tip is
   an extension of the other.
   When the remote is already ahead on a ref, git rejects the push; the
   rejection is harmless, and the next fetch picks up the newer commits.

> [!CAUTION]
> A forcing refspec — the `+` prefix, as in `+refs/jp/issues/*` — disables
> git's fast-forward refusal, which is the store's integrity guard.
> A remote configured with a forcing refspec over `refs/jp/` lets a hostile or
> compromised peer silently overwrite good refs with rewritten history.
> `issue sync` always passes its own explicit non-forcing refspec on the `git
> fetch` command line, which overrides whatever refspecs the remote's config
> carries; it also refuses to run while any remote's configured `fetch` entry
> carries a forcing refspec covering `refs/jp/`.
> Never configure one manually.

`issue sync` also maintains a local ref journal as a second guard.
It sets `core.logAllRefUpdates=always` in the repository — informing the user
whenever this changes existing git configuration — so git journals every ref
update into reflogs recording each transition's old and new values, including a
hand-run `git fetch` with a forcing refspec that bypasses the tool entirely.
Every run starts by reading that journal, flags any `refs/jp/` transition that
failed to fast-forward, and offers to restore the journaled prior value.
The reflog entries also protect the overwritten commits from garbage collection
while they live, so the restore is always possible.

A remote serving older tips is harmless.
Refs never move backward — an older tip fails to fast-forward — and the newer
state is adopted on the next sync with any replica that has it.
Syncing with a participant who is behind carries no penalty, and which remotes
to fetch from stays the user's decision.

Two replicas may push the same ref, but a push race cannot corrupt a chain: only
the owning replica ever appends commits to a chain, so competing pushes carry
the same tip, or one tip is an extension of the other.
Within one replica, concurrent tool invocations serialize through git's own ref
locking — `update-ref` with the expected old value; on failure, re-read and
retry the append.
No legitimate non-fast-forward ref update exists anywhere in the system.

### Server-side receive gate

A git server that accepts pushes holds a replica of the store, but plain git
applies no special rules to `refs/jp/issues/*`: a pusher with write access can
force-push a rewritten chain, and the server accepts a rewritten chain that
every jp tool would refuse.
On servers whose operator can install hooks — self-hosted git, or GitHub
Enterprise Server — a `pre-receive` hook rejects any update to
`refs/jp/issues/*` that fails to fast-forward the ref's current value, contains
a commit with more than one parent, or contains a commit whose signature does
not match the key hash in the ref path.
The hook runs at push time the same checks the fold runs at read time.

github.com runs no user-supplied `pre-receive` hooks, and github.com branch
protections and rulesets cover `refs/heads/*` and `refs/tags/*` only.
A store whose shared remote is github.com has no receive gate: any collaborator
with write access can force-push a rewritten chain to the shared remote.
The client-side defenses still hold: every replica's non-forcing fetch refuses a
rewritten chain, the ref journal records a rewrite forced through by hand, and a
fresh clone cannot compute issue state, because surviving chains acknowledge ops
that the rewritten chain no longer carries.
The receive gate is extra hardening on servers that support hooks; the design
does not depend on the receive gate.

### The scraper

The scraper is one writer among several, recording observations from GitHub's
read-only interface.
Per issue it folds current local state, diffs the scraped issue and comments
against it, and emits ops only for fields that differ.
An unchanged issue writes nothing.
Commits carry provenance: `scraped_at` and the GitHub `updated_at` observed.

Phase 1 scrapes issues and their conversation comments (comment bodies are
editable on GitHub, hence `set-comment-body`).

The scraper also detects upstream deletions, under one rule: a tombstone
requires positive evidence of deletion, never absence from a possibly-incomplete
listing.
Edits are positive observations and are committed incrementally as scraped; a
deletion is an inference from absence, so deletion ops are emitted only after
the enumeration they are inferred from has run to completion, and only after a
direct GitHub API request for the missing item confirms the deletion (HTTP
404/410).
Concretely: each run enumerates the full issue list, and the comment id set of
each changed issue; a previously observed issue or comment missing from its
completed enumeration is requested individually from the API, and only an HTTP
404/410 yields the `tombstone-issue` or `tombstone-comment` op, carrying
scraper provenance.
A scrape that aborts mid-run (rate limit, network failure) therefore emits the
edits it observed and no deletions.

### Signed commits

Every writer signs its commits (`git commit-tree -S`; SSH-key signing via
`gpg.format=ssh` keeps the requirement to a key writers already have).
The scraper signs like any other writer.

Enforcement happens at read time, inside the fold — commits can arrive from any
remote or bundle, so no single point exists through which all writes pass, and
GitHub's signed-commit protections only cover branches.
The fold verifies each commit (`git verify-commit`); ops from unverifiable
commits are excluded from the computed state and surfaced as a warning.
A configuration option (on by default) escalates the warning to a hard failure.

Verification is required only from a configured cutoff.
A `verification_required = "<commit-hash>"` configuration field names a commit:
that commit and everything after it (in the causal order of the section
"Computing issue state") require verification, while commits before the cutoff
are exempt from the checks above.
With the field unset, no commit requires verification.
This lets a store adopt signing late without invalidating its earlier history.

The trust anchor stays **outside the repository**: a per-user allowed-signers
file (git's `gpg.ssh.allowedSignersFile` format) under the user's own
configuration, with keys exchanged out-of-band.
A checked-in list would let anyone who can push code appoint signers — the
artifact being verified must not control its own trust anchor.

The signing key is the authoritative identity, and the writer id embeds its
hash: the fold rejects commits that live under a key-hash segment but are not
signed by the matching key.
All replica refs under the same key hash belong to the same principal.

### Deletion

The store is append-only: no ref is ever rewritten and no object is ever
removed.
Every replica refuses a non-fast-forward update of a foreign ref, so an author
cannot hide a history rewrite — existing replicas detect the rewrite directly,
and on a fresh clone the fold stops on the dangling `seen_heads` references that
the rewrite leaves behind.

`tombstone-issue` and `tombstone-comment` are ordinary ops: they propagate
like any other, and every fold hides the target from that point on.
The hidden bytes remain in every clone, permanently.

Every writer trusted by the allowed-signers file is a full peer: any peer
may perform any op on any item, including a tombstone on an item another
peer created.
A tombstone is final; no op un-hides a tombstoned target.
When a tombstone turns out to be wrong, the original author resubmits the
content as a new issue or comment, and the new item can be tombstoned in
turn by the same rule.

Store-level metadata (the allowed-labels vocabulary) lives in its own log at
`refs/jp/issues/meta/<writer-id>`, using the same op machinery as an issue.

### Implementation: git plumbing subprocesses

All object and ref access shells out to the `git` binary through the existing
`ProcessRunner` abstraction in the tools crate:

- writes: `git hash-object -w --stdin`, `git mktree`, `git commit-tree`, `git
  update-ref`
- reads: `git for-each-ref`, `git rev-list`, one-shot `git cat-file --batch`
  with all requests written to stdin upfront

Rationale, in order of weight:

1. **Coexistence is correctness-critical.** The store lives inside repositories
   people care about.
   The git binary can never disagree with itself about locking, gc, packfile
   formats, or the repo's object format (SHA-256 repos are inherited for free).
2. **Scale does not justify a library.** Hundreds of issues, dozens of ops each;
   incremental scrapes write a handful of commits.
   The initial import is a one-time bulk write of a few thousand spawns.
3. **Zero new dependencies** in a project that runs cargo-vet, and the
   subprocess pattern — including `MockProcessRunner` tests and real-git
   integration tests — already exists in the tools crate.

This decision has a pre-agreed revision trigger: if computing state across all
issues (the kanban view) measures slow, the read path moves to the `gix` crate
(reading is its most mature half) while writes stay as plumbing.
The on-disk format is git's either way, so stored data does not change.

## Drawbacks

- Refs grow with issues × replicas and are permanent: a chain's ops are part of
  issue state, so refs cannot be pruned.
- The store only grows.
  Deleted content still occupies space in every clone forever.
- Reads assemble N writer heads per issue instead of walking one DAG, and
  cross-writer causality is invisible to `git log --graph` — only the fold can
  reconstruct it.
  Acceptable: the consumers are exclusively our own tools.
- Ops carry a small map of writer id to op id (the `seen_heads` field).
- Every writer whose commits fall under the `verification_required` cutoff must
  have commit signing configured, including scraper automation.
- Subprocess-based reads put a performance ceiling on computing state over many
  issues; the Implementation section names the measured trigger for moving reads
  to `gix`.

## Alternatives

- **Checked-in files** (e.g. `issues/*.json` in the worktree): merge conflicts
  in worktrees, issue churn pollutes code history.
  Rejected in Motivation.
- **A dedicated bare repository** owned by JP (or a radicle-style centralized
  store): breaks "pull and you have everything", and centralizing many
  repositories in one store is rejected for security reasons.
- **State snapshots instead of op logs**: snapshot merges have no principled
  answer to concurrent divergence; the format is a distributed contract that
  every collaborator's clone holds copies of, so migrating later means a
  coordinated flag-day.
  Op-log from day one.
- **Coarse `observe` ops** (full issue JSON per write): simpler to write, but
  pushes interpretation into the fold and makes local edits a second,
  differently-shaped op family.
  Fine-grained ops keep `issue append` symmetrical.
- **One shared ref per issue, merge-on-push**: with N replicas syncing pairwise
  at arbitrary times, shared-ref convergence mints bookkeeping merge commits at
  every divergent sync, and independent joins of the same heads themselves
  diverge.
  Per-writer refs eliminate the entire category.
- **Causality as commit parents** (multi-parent commits referencing foreign
  heads): structurally merge commits, which this design forbids; the
  `seen_heads` field carries the same information while keeping every chain
  linear.
- **git-bug / git-appraise**: closest prior art, same refs-in-repo approach, but
  git-bug's elaborated last-write-wins hides conflicts that drop data.
- **`gix` or `git2` instead of plumbing subprocesses**: see the rationale table
  in Design; a large vet surface (`gix`) or a C dependency (`git2`) buys speed
  the workload does not need, at coexistence risk the store cannot afford.
- **`git fast-import` for bulk writes**: a second command language to generate
  and debug; the write volume does not demand it.
  Reach for it only if initial import time annoys someone.

## Non-Goals

- Pull requests, review comments, and reactions.
- Local issue creation and editing (`issue append`).
  The op vocabulary and store format are built for it, but the write path is a
  later phase.
- The kanban tool and any state caching for it (the `set-priority` op is
  registered here; the tool that consumes it is not).
- Cross-repo mirroring.
- Moderation and per-key authorization: every trusted writer is a full peer.
  Authorization rules can be added later, evaluated at fold time, without
  changing stored data.
- Physical removal of store content: deletion only hides.

## Risks

- **Deleted content persists in every clone.** Every clone permanently holds
  every op ever synced, including content its author deleted.
  This is deliberate — the store is append-only — but it means true erasure
  (leaked credentials, legal demands) is impossible inside the system.
  The remedy for a leaked secret is rotating the secret.
- **Large payloads.** When `issue append` needs content too large for `ops.json`
  (logs, screenshots), the payload goes to the `.jp/blobs/` store of [RFD 066],
  referenced from the op by SHA-256.
  The signed op carries the checksum, so signature verification extends to the
  blob content.
  Consequence to accept: blobs travel with ordinary worktree commits, not with
  `issue sync` ref exchange, so an op can reference a blob its reader has not
  yet pulled.
  Details deferred to the append-phase RFD.

- **Withholding is undetectable.** A remote can serve truthful but stale refs:
  every commit validly signed, every update a fast-forward, and the newest ops
  simply absent.
  A reader served only by that remote sees an issue frozen in the past, and no
  mechanical check distinguishes withholding from ordinary propagation delay.
  Accepted as inherent: refs never move backward, the newer tip is adopted as
  soon as any replica that has it is fetched from, and the choice of remotes is
  the user's.
- **Verification cost.** `git verify-commit` per commit at read time is
  subprocess-heavy; verification results may need caching.
  Measure before optimizing.

## Attack Analysis: Chain Rewrite

Several of the mechanisms above — the ref journal, the receive gate, the
missing-acknowledgment rule — exist because of one concrete attack.
This section documents it and maps each defense to the design mechanism that
closes it.

Mallory is a developer with a git remote she controls.
She rewrites the history of one writer chain under `refs/jp/issues/42/`,
removing the commit that holds the plan for issue 42.
Alice fetches code and issue refs directly from Mallory's remote.
Bob runs the git server that the team otherwise shares, and Alice has push
access to it.

If Alice already holds the current value of the rewritten ref, her non-forcing
fetch refuses the update: a chain missing a commit fails to fast-forward.
If Alice is behind, or fetching these refs for the first time, she accepts
Mallory's version — a first fetch has no prior value to compare against, and
signatures authenticate authorship of the commits that are present without
proving that the set is complete.

Otherwise-normal git tooling exposes the attack at every point where it would
otherwise take hold or spread:

- **Fast-forward refusal** (the section "Sync"): every replica that already
  holds the honest ref refuses Mallory's rewrite outright.
- **The local ref journal** (the section "Sync"): if Alice bypasses the tool
  with a hand-run forcing fetch, the reflog records the non-fast-forward
  transition; the next `issue sync` flags it and offers to restore the journaled
  prior value.
- **Missing-acknowledgment detection** (the section "Computing issue state"):
  other writers' `seen_heads` still name the dropped op, so even a fresh clone
  — with no prior refs to compare against — refuses to render issue state; the
  fold stops with a typed error naming the missing op id.
  Mallory dropped the op from every copy she controls, but the op exists on no
  other remote either, so fetching from more remotes never cures the error, and
  the error hardens into evidence of a rewrite.
  `--fix-interactive` offers to drop the offending ref.
- **The server-side receive gate** (the section "Server-side receive gate"):
  spreading the corruption through the shared server requires a force push that
  Bob's `pre-receive` hook refuses.
  When the team's shared server is github.com, no hook runs and the force push
  succeeds; fast-forward refusal, the ref journal, and missing-acknowledgment
  detection are the remaining defenses.

A remote can also *withhold*: serve truthful but stale refs, every commit
validly signed and every update a fast-forward, with the newest ops simply
absent.
That is not a rewrite and no defense above fires; it is an accepted limit,
recorded under Risks, and the sync rules in the section "Sync" guarantee the gap
heals on the next sync with any replica that has the newer state.

## Implementation Plan

Each phase is independently reviewable and mergeable.

1. **Store primitives** in the tools crate: writer-id minting and storage,
   `ops.json` schema (versioned), commit read/write via `ProcessRunner`
   plumbing, ref enumeration.
   Unit-tested against `MockProcessRunner`, integration-tested against real temp
   repos.
2. **State computation (the fold)**: chain walking, causal ordering from
   `seen_heads`, the missing-acknowledgment check with its typed error and
   `--fix-interactive` resolution, deterministic tiebreak, per-field semantics
   (multi-value registers, observed-remove set, grow-only set).
   Pure logic over data fetched by phase 1; property-style tests for
   order-independence.
3. **Scraper** (`issue sync`, write side): scrape via `jp_github`, diff scraped
   state against computed local state, emit ops — including `tombstone-issue`
   / `tombstone-comment` for upstream deletions — and sign commits.
   Depends on phases 1–2.
4. **Sync** (`issue sync`, transport side): fetch refspec configuration,
   fast-forward pushes, refusal to run while any remote's configured `fetch`
   entry carries a forcing refspec covering `refs/jp/`,
   `core.logAllRefUpdates=always` setup and the ref-journal check, and a
   reference `pre-receive` hook for server operators.
5. **`issue show`**: compute and render one issue's state, including surfaced
   multi-value conflicts and unverified-writer warnings.
6. **Signature verification** during state computation: `verify-commit` against
   the per-user allowed-signers file, with the warn/enforce configuration option
   and the `verification_required` cutoff.
7. **Deletion** (`issue delete`): tombstone ops.

## References

- [RFD 066] — Content-Addressable Blob Store: content-addressed storage for
  conversation blobs, and the designated home for large payloads in the later
  `issue append` phase (see the section "Risks").

- [git-bug] — issues as git objects in refs, closest prior art.
- [git-appraise] — code review as git objects in refs.
- [radicle COBs] — collaborative objects as commit DAGs; this design borrows
  the op-log idea but rejects centralized per-user storage and multi-parent
  causality.

[RFD 066]: ../066-content-addressable-blob-store.md
[git-bug]: https://github.com/git-bug/git-bug
[git-appraise]: https://github.com/google/git-appraise
[radicle COBs]: https://radicle.xyz/guides/protocol#collaborative-objects
