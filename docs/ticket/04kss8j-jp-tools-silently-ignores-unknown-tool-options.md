# jp-tools silently ignores unknown tool options

- **Status**: Todo
- **Kind**: Bug
- **Authors**: jp
- **Date**: 2026-08-18

`jp-tools` reads per-tool settings from a free-form `options` map and drops
anything it does not recognise.
`Tool::option_or` returns the default both when the key is missing and when the
value fails to deserialize, so an option the running binary does not implement
is indistinguishable from one that was never set.

The failure that surfaced this: `cargo_check` and `cargo_test` were configured
with `options.root` pointing at a second cargo workspace, but the installed
binary predated that option.
Both ran against the default workspace and reported success.
The results were green, plausible, and about the wrong code.
The only thing that caught it was recognising the test count from an earlier
run.

## Why this is worse than an ordinary silent default

A verification tool's whole job is to answer "is this code correct?".
When it silently answers about *different* code, every conclusion drawn from it
is wrong, and nothing in the output hints at it.

The skew is easy to hit: tool config lives in the repository and updates on
pull, while the binary updates only on `just install-tools`.

## Existing precedent

`web_fetch` already parses its settings into a typed `WebFetchOptions`
(`.config/jp/tools/src/web/fetch/options.rs`) rather than reaching into the map
directly.
Generalising that pattern is most of the work.

## Directions to investigate

- Give each tool or tool group a typed options struct, deserialized once at
  dispatch, replacing ad-hoc `option_or` lookups.
- `#[serde(deny_unknown_fields)]`, so a typo or an option meant for a newer
  binary is a loud error rather than a no-op.
- Decide the failure mode deliberately: reject the call, or run and report the
  ignored keys.
  Silence should not be one of the choices.
- Have tools state the effective setting they acted on (for the cargo tools, the
  workspace root), so a misdirect is visible in the result itself.
  This is the cheapest mitigation and is independent of the typed-options work.
- Consider whether tool configs should declare a minimum binary version, since
  typed options alone still cannot distinguish "unknown" from "not yet
  implemented" in a useful message.

## Scope

`jp_config` forwards unknown options untouched by design, which is correct: it
cannot know each tool's schema.
The gap is that the receiving end discards them just as quietly.
The fix belongs in `.config/jp/tools`, which is developer tooling rather than
shipped surface.
