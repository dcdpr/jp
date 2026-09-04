# Rename `AccessPolicy::is_restricted` once a second resource axis is enforced

- **Status**: Todo
- **Kind**: Chore
- **Authors**: jp
- **Date**: 2026-09-04
- **Implements**: 076

`AccessPolicy::is_restricted()` reports on `fs` alone — it is
`!self.fs.is_empty()` — but its name reads as a statement about the whole
policy.
A policy carrying `env` deny rules and no `fs` rules answers `false`.

Today that is correct and harmless.
Its only production caller is `AccessPolicy::permits`
(`crates/jp_tool/src/access.rs:112`), which is fs-only, and the doc comment
already says "Whether **filesystem** access is restricted".
Nothing reads it as policy-wide.

The name becomes hazardous when a second axis gains a restriction predicate —
RFD 076's `net` and `env` consumers, or RFD 075's sandbox gating, where "is this
policy restricted?" would be a natural thing to ask before deciding whether to
build a profile at all.

## What to do, and when

At the point a second axis needs one:

- Rename to `is_fs_restricted()`, and add the sibling predicate the new axis
  needs.
- `jp_tool` is the wire crate that external tool binaries build against, so this
  is a public API break.
  Worth doing once, alongside a change that justifies it, rather than on its
  own.

Not worth doing before then: there is no caller to correct and no operation that
misbehaves, and a rename with no second predicate beside it just moves the same
question to a longer name.
