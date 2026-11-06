# Vendor rustqual into crates/contrib, trimming dependencies

- **Status**: Todo
- **Authors**: jp
- **Date**: 2026-11-04
- **Label**: domain=tooling
- **Label**: type=task

Bring rustqual (MIT, <https://github.com/SaschaOnTour/rustqual>) into
`crates/contrib/rustqual`, trim the dependencies the workspace doesn't already
have, and hold it to the workspace's formatting and lints.
JP runs it today as a binary installed from crates.io (`_install-rustqual`,
pinned at 1.8.2 in the `justfile`).

Depends on T-0yh5frr landing first.

## Why

Several findings are wrong for any workspace, not only JP's:

- `BP-002` flags real formatting (`write!(f, "{}: {}", a, b)`, padding-aware
  delegation) along with newtype boilerplate;
- `NMS` and `TQ-002` can't see types or follow calls through traits;
- `IET` flags every serde `serialize`/`deserialize` pair;
- `SLM` ignores `#[expect(clippy::unused_self)]`;
- DRY and IOSP can't be turned off one rule at a time.

Fixing them in-tree is faster than a release cycle.
Upstream has been quiet since 1.8.3 (2026-09-24), with ten of the author's own
issues open.
T-0yh5j3r becomes "fix here, offer upstream".

## Rules for changes

- Nothing JP-specific goes into rustqual.
  Every change is generic and configurable, so it would make sense in any
  workspace.
  JP's policy (which rules gate, which paths they apply to) stays in
  `.config/rustqual/`.
- Upstreaming later is welcome but not a constraint.
  Stripping, reformatting and lint fixes may make it impractical, and that is
  accepted.

## Commits

1. Import 1.8.3 (`0dc1e18`) verbatim, with its `LICENSE`.
2. Trim its dependencies (below).
3. Reformat the whole crate with the workspace `rustfmt` and `comfort`.
4. Fix it under the workspace lints, with no crate-level `[lints]` relaxations.
   rustqual prints its reports directly; route that through one output point, as
   `comfort::cli_main` does.
5. Wire it in: workspace member, workspace dependency versions, `toml` 1 instead
   of 0.8.

Every later change to rustqual is its own commit, kept apart from changes to
JP's configs or to the code that calls rustqual.
Record the upstream commit in `Cargo.toml`, and keep a README section listing
what differs, as `crates/contrib/xct2cli` does.

## Dependencies

What gets cut is dependencies, not features. rustqual is a general tool, and a
feature JP doesn't use today is one another workspace does: the baseline
(`--save-baseline`, `--compare`), every output format, the architecture
dimension, `--init`, `--explain`.
A feature stays unless keeping it means a crate the workspace doesn't already
have.
Where a dependency can be upgraded or swapped for one the workspace has, do that
and keep the feature.

Against `Cargo.lock` on 2026-11-04:

| Crate                                                                                                    | In the workspace                    | Plan                                                                                                                                |
| -------------------------------------------------------------------------------------------------------- | ----------------------------------- | ----------------------------------------------------------------------------------------------------------------------------------- |
| `syn`, `quote`, `proc-macro2`, `clap`, `serde`, `serde_json`, `rayon`, `thiserror`, `walkdir`, `globset` | yes                                 | Keep, on the workspace versions.                                                                                                    |
| `toml`                                                                                                   | 1.x (rustqual uses 0.8)             | Upgrade.                                                                                                                            |
| `derive_more`                                                                                            | 2.x, transitively (rustqual uses 1) | Upgrade to 2.                                                                                                                       |
| `tempfile`, `proptest` (dev)                                                                             | yes                                 | Keep.                                                                                                                               |
| `colored`                                                                                                | no                                  | Swap for `anstyle` (already in through `clap`) or `crossterm`.                                                                      |
| `toon-encode` (`--format ai`)                                                                            | no                                  | Write the encoder for rustqual's one envelope shape, or drop TOON and keep `ai-json`. Decide once the size of the encoder is known. |
| `clap_complete` (`--completions`)                                                                        | no                                  | Keep. RFD 059 brings it in for `jp`'s own completions anyway.                                                                       |
| `notify` (`--watch`)                                                                                     | no                                  | Replace with a polling loop over file mtimes if that stays small, otherwise drop.                                                   |

Keep the rule cards: they supply the rule titles in the SARIF rules table, and
`--explain`.

## Integrate

- Add `crates/contrib/rustqual/**` to `exclude_files` in `config.toml` and
  `dry.toml`, so its source and fixtures add no findings and its markers don't
  count toward the suppression ratio.
- `rustqual --version` reports a hash of the crate's source (from a `build.rs`),
  so `qual`'s base-report cache, which is keyed on the version, invalidates when
  rustqual changes.
- CI and `just qual` build the workspace binary from the checkout being measured
  and put it on `PATH`, instead of installing from crates.io.
  `qual` runs that one binary over both the base and the head, so a rustqual
  change can't show up as findings appearing or disappearing.
  Remove `rustqual_version` and `_install-rustqual`.

## Next: fold `qual` into rustqual

After this ticket, and before any of the analyser fixes listed under *Why*.
Its own ticket or tickets; one commit per feature moved.

Almost everything `qual` does works around something rustqual can't do yet, or
is a generic feature rustqual should have:

| `qual`                                                 | In rustqual                                                                                                                                                                                                                          |
| ------------------------------------------------------ | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| Two passes, `[[pass]]` rule ownership, `exempt`        | A per-rule on/off switch in the config. That removes the second pass and `dry.toml` with it.                                                                                                                                         |
| `enforced`                                             | Rules that must stay at zero findings.                                                                                                                                                                                               |
| Diff by `(rule, file, symbol)`                         | Record each finding in the baseline, so `--compare` reports which findings are new rather than which counts or scores moved (T-0yh5j3r item 2). rustqual knows each function's qualified name, so `qual`'s `syn` symbol lookup goes. |
| Base analysed from a git revision, with rename mapping | An optional `--baseline-rev`, next to the baseline file, which stays the default. rustqual runs `git` with `std::process::Command`; it can't depend on `jp_process`.                                                                 |
| `guidance/<RULE>.md`                                   | A configurable guidance directory.                                                                                                                                                                                                   |
| Markdown summary; failing on skipped files             | Output format; a fail-closed run.                                                                                                                                                                                                    |

`qual`'s tests in `crates/internal/qual` are the spec for each feature.
Once they pass against rustqual, point CI (T-0yh5npc), `cargo_check` (T-0yh5pja)
and the burn-down loop (T-0yh5sy9) at rustqual, move `qual.toml`'s policy into
rustqual's config, and delete `qual`.

With `qual` gone, rustqual needs no library API: callers run the binary and read
its JSON output.
