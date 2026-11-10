# Release Pipeline

- **Status**: In Progress
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-11-03
- **Label**: tracking-rfd=122
- **Label**: type=tracking

Tracking ticket for [RFD 122].

## Implementation plan

- [x] **Add release model, credits, and rendering**: Adds the release model,
      crediting for GitHub, GitLab, and no forge, and the Markdown views to the
      `changelog` crate, with `changelog render`.
      Adds `jp release render` with the `forge` and `headings` options.
      Depends on RFD 121 Phases 2 and 3.
- [x] **Implement cut, pick, and finalize**: Delivers versions from tags and
      from the commits that start releases, `bump`, and release branches and
      notes branches made in the local repository, with the `git push` that
      publishes them printed.
      Also covers picks with fragment moves and copies onto the notes branch,
      patch releases, and tagging with the release notes as the tag's message.
      Configures JP's `bump`, and runs JP's checks on pushes to `release/*`.
      Depends on Phase 1.
- [x] **Add due, status, and the release reminder**: Adds `jp release due`, `jp
      release status`, and `just release-due`, plus JP's daily workflow that
      opens a reminder issue, which the tag of the next minor or major release
      closes.
      Depends on Phase 2.
- [x] **Add release pages to the site and retire jilu**: Adds releases as a `jp
      vite` workflow: snapshot entries, loader, release notes and upgrade guide
      pages, the index, and the "Unreleased" page.
      Installs the `changelog` binary in the docs build, builds the site from
      `main` alone and again when a release's tag is pushed, switches the
      navigation to "Releases", and retires `jilu`.
      Depends on Phase 1 and RFD 116's site phase.
- [x] **Cut the past, and abandon**: Adds `cut --at`, files fragments by the
      changes their `Credits` name, starts release branches from trees without
      `changelog.d/`, and adds `abandon`.
      `cut` tells the person releasing to merge the notes branch into `main`.
      Depends on Phase 2.
- [x] **Build releases**: Adds `build` with the `build` option and `--quick`,
      the build directory and its checks, and `status` naming finalized releases
      without a publishable build.
      `--remote` and `builders` land with a project's first remote builder.
      Depends on Phase 2.
- [x] **Links and drafts**: Adds the `repository` option wherever notes are
      rendered and in `jp vite`'s snapshot, and draft pages for releases in
      preparation under `jp vite serve`.
      Depends on Phase 4.

The repository settings the plan names, protecting `release/*` branches and `v*`
tags, are [T-1f2jmb7].

## Comments

-----

- **From**: jp
- **Date**: 2026-11-07T18:41:58Z

The checklist follows the RFD's Implementation Plan as amended during
implementation, not the one this ticket was filed from:

- `cut`, `pick`, and `finalize` make their branches and tags in the local
  repository and print the `git push` that publishes them, rather than pushing
  and opening change requests through the forge, so `jp_github`'s pull-request
  calls and the `change_request_labels` option are gone.
- `jp release review`, the fifth phase, was implemented and then dropped:
  running `jp query` with a persona does the same job and can also make the
  edits.
  Its item is removed rather than left ticked.
- `jp release status` joined the reminder's phase, and a release gets its pages
  on the site once it is tagged.

-----

- **From**: jp
- **Date**: 2026-11-10T09:54:12Z

Reopened: RFD 122 is back to Accepted for three new phases, which come from
releasing JP's history with the same commands ([RFD 123]).

- **Cut the past, and abandon**: `cut --at` cuts at an earlier commit of `main`,
  with fragments filed by the changes their `Credits` name, and `abandon` undoes
  a release nobody has published, so a trial can back out and try again.
- **Build releases**: `build` runs the project's build command and checks its
  archives, checksums, and manifest, so a release ends with binaries built on
  the releasing machine; publishing them is a separate plugin.
- **Links and drafts**: the `repository` option, because a rehearsal clone's
  `origin` is not the forge, and draft pages for releases in preparation under
  `jp vite serve`.

[RFD 122]: ../rfd/122-release-pipeline.md
[RFD 123]: ../rfd/123-distributing-the-jp-binary.md
[T-1f2jmb7]: 1f2jmb7-protect-release-branches-and-release-tags-on-github.md
