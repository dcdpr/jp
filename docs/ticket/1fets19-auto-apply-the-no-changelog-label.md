# Auto-apply the no-changelog label

- **Status**: Todo
- **Authors**: jp
- **Date**: 2026-11-08
- **Label**: domain=tooling
- **Label**: relevant-rfd=121
- **Label**: type=follow-up

The changelog gate ([RFD 121]) passes a pull request that adds a fragment or
carries the `no-changelog` label, so every pull request that changes nothing
users notice waits for someone with write access to add the label by hand; a
fork's author can't add it.
The gate should apply the label itself when a pull request opens, as a default a
person can override.
This was RFD 121's optional Phase 4, moved here so the RFD could be marked
Implemented without it.

## Design

A step in the gate, run before it evaluates, applies `no-changelog` when a pull
request opens:

- when the conventional type in the title is not user-facing (`build`, `ci`,
  `docs`, `refactor`, `test`), or
- when every path the pull request changes is non-shipping.

It never applies the label for the user-facing types (`feat`, `enhance`, `fix`,
`perf`).
A title without a conventional type, or with one in neither list, such as
`chore`, falls through to the paths alone.
It acts only on `opened`, so once a person adds or removes the label, that
choice stands.

The paths fall into three classes:

- **Shipping**, so an entry is required: `crates/jp_*/**`, `crates/plugins/**`,
  and the top-level files that package the runtime.
- **Non-shipping**, so the label applies: `docs/**`, `.github/**`, `.jp/**`,
  `.config/jp/tools/**`, `crates/internal/**`, `justfile`, `apps/macos/**` (the
  app isn't distributed), `changelog.d/**` (such as a correction to a released
  note), and test fixtures.
- **Ambiguous**, so an entry is required and a person decides: the top-level
  `Cargo.toml` and `Cargo.lock`, and `crates/contrib/**`.

Any path not listed requires an entry.
The classes live in the gate's workflow, beside the code that applies them,
rather than in `changelog.d/README.md` as RFD 121 planned: `jp release init`
writes that README into every workspace, and the classes are JP's.

The label has to be applied in the gate's own job: a label added with a
workflow's token starts no other workflow, so a separate labeling workflow would
leave the gate red from its first run.
Under `pull_request`, a fork's pull request gets a read-only token, which can't
add a label, so the gate moves to `pull_request_target`, as `merge-label.yml` on
`main` runs.
Under `pull_request_target` the job checks out the base branch and must never
run the pull request's code, so it reads the changed files, the added fragments,
and the labels through the API.
It reports a commit status rather than failing a job, as `merge-label.yml` does:
`pending` while a fragment or the label is missing, so the pull request waits
rather than turning red, and `failure` only for a title marking a breaking
change without an upgrade section.
Format validation, `just changelog-ci`, stays on `pull_request`, since it builds
the pull request's code.

## Acceptance criteria

- [ ] The gate runs on `pull_request_target` and reads files, fragments, and
      labels through the API, checking out none of the pull request's code.
- [ ] It reports a commit status: `pending` until a fragment or the label is
      there, `failure` only for a breaking title without an upgrade section.
- [ ] On `opened`, it applies `no-changelog` by the title's type or the path
      classes, and evaluates with the label applied.
- [ ] The path classes sit in the gate's workflow, with a comment saying the
      label is the default a person overrides.
- [ ] The required check on `main` becomes the status's context.

[RFD 121]: ../rfd/121-changelog-fragments-and-validation.md
