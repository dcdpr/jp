# `transform` merge attributes never run on JP's config path

- **Status**: Todo
- **Kind**: Chore
- **Authors**: jp
- **Date**: 2026-08-24

A `#[setting(transform = ...)]` declaration has no effect in JP.

Transforms are applied by `PartialConfig::finalize`, which is called from
exactly one place — `ConfigLoader::load`
(`crates/contrib/schematic/src/config/loader.rs:127`).
JP never calls it: `jp_config::util::load_partial_at_path` uses
`loader.load_partial(&())`, and resolution goes through
`AppConfig::from_partial_with_defaults`.
So a declared transform is silently dead.

`finalize` is not usable as-is either.
It applies defaults, then the partial, then env vars in one shot, but JP already
applies env separately (`load_envs`) and defaults separately, and the defaults
step deliberately *fills* rather than *merges* (that is what `FillDefaults`
exists for).
Calling `finalize` would double-apply env and route defaults through the merge
strategies, so an appending default would append to itself.

## Current state

\#887 removed both existing uses rather than leaving them dead:

- `AppConfig::config_load_paths` and `AnthropicConfig::beta_headers` both
  declared `transform = util::vec_dedup` on top of `merge = append_vec`.
  Both now use `internal::merge::append_vec_dedup`, which does the append and
  the dedup in one step, and `util::vec_dedup` is deleted.

So there are no `transform` declarations left in `jp_config`.
The hazard is that the next one added will silently do nothing.

## Options

1. **Remove `transform` support from `schematic_macros`.** Turns a silent no-op
   into a compile error.
   Cheapest, and matches the "fail loudly" direction \#887 took with invalid
   `#[setting]` keys.
2. **Apply transforms on JP's path.** Generate a `transform_values()` on the
   partial that runs only the transforms, and call it from
   `jp_config::util::build`.
   Keeps the feature, needs macro work and a decision about where in the
   pipeline it runs.

Option 1 is the smaller change and nothing currently needs the feature.

## Context

Found in \#887 while writing a merge test for `config_load_paths` — the first
draft asserted the resolved config was deduplicated and failed.
Disclosed in review comment 3659114885.
