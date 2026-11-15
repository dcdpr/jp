# Build and host the site of each release

- **Status**: Todo
- **Authors**: jp
- **Date**: 2026-11-15
- **Label**: domain=tooling
- **Label**: relevant-rfd=123
- **Label**: type=feature

The release backfill gives every release's tag the wiki and user guide as they
were for that release (RFD 123, "JP's history"), so each release's site can be
built from its tag.
Nothing builds or hosts those sites yet: `jp.computer` shows `main` only.

- [ ] `just build-docs X.Y.Z` builds release X.Y.Z's site: today's site setup
      (`jp vite`, `docs/.vitepress/`) around the tag's `docs/`, the way
      `build.sh` builds old code with today's script.
- [ ] The navigation shows only the sections the tag's tree has: no RFDs,
      tickets, or wiki where the release had none.
- [ ] A tag from before the site existed (#120, 2025-06-04) has no front page;
      the build supplies one, linking the release's user guide and its release
      pages on the main site.
- [ ] A release's site links its release notes on the main site, where they are
      corrected: release branches hold no `changelog.d/`.
- [ ] CI builds every release's site and hosts it under `jp.computer/vX.Y.Z/`.
- [ ] Until a release's site is hosted, its `/vX.Y.Z/` paths fall back to the
      current site, since release notes link guide pages there.
- [ ] Every page of a release's site, except the latest stable release's, shows
      a banner naming the release and linking the latest one.
- [ ] `jp.computer` serves the latest stable release's site, and `main`'s moves
      to `jp.computer/unreleased/` or similar.

Release notes live on `main`, so the release pages come from `main`'s build
whichever site the root serves; where they're served from once it serves a
release is still open.
