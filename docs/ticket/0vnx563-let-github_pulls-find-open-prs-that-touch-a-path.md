# Let github_pulls find open PRs that touch a path

- **Status**: Todo
- **Kind**: Feature
- **Authors**: jp
- **Date**: 2026-10-01
- **Label**: domain=tooling

`github_pulls` lists pull requests by state only
(`.config/jp/tools/src/github/pulls.rs`, `list`).
`github_code_search` searches the default branch only, so code that exists only
in an open PR is invisible to it.
Answering "which open PR adds `crates/jp_process`?" takes one `github_pr_diff`
call per open PR, and an assistant that skips some of them answers wrong.

That happened during T-0vm44ks: the search covered the code index and one PR's
file list, found nothing, and reported that no shared process runner existed.
PR #1175 added it.
The user found it with a shell loop over `gh api repos/dcdpr/jp/pulls/$n/files`.

Fix: add an optional `path` parameter to `github_pulls`, a path prefix.
With it set, the tool lists the PRs matching `state` and keeps those whose
changed files include a path under the prefix, naming the matching files for
each.
The file lists are paginated (100 per page, up to 3000 files), so every page is
read; a PR with more files than the API returns says so rather than being
reported as a non-match.

Test with a mocked client: two open PRs, one touching
`crates/jp_process/src/lib.rs`, filtered on `crates/jp_process`, returns only
that PR and that file.
