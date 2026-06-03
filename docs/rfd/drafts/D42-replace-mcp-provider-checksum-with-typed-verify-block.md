# RFD D42: Replace MCP provider checksum with typed verify block

- **Status**: Draft
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-05-22

## Summary

Replace the `checksum` field on `[providers.mcp.*]` with a tagged-enum `verify`
block that names what it verifies.
The current field hashes `command`, which is the wrong artifact when `command`
is a runtime that resolves the real tool at execution time (`uvx`, `npx`,
`docker run`, …).
Identity pinning is moved out of scope for `verify` and documented as belonging
in `arguments`, using the launcher's own version syntax.

## Motivation

The current `[providers.mcp.<name>].checksum` field validates the SHA of
`config.command` (see [`verify_file_checksum`][verify-impl]).
The implicit claim is "this MCP server's binary won't change underneath you."
The shape holds for static binaries; it breaks the moment `command` is a
launcher that resolves the actual tool at runtime.

Concrete example.
A user pinned `kagimcp` like this:

```toml
[providers.mcp.kagi]
command = "/Users/jean/.cargo/bin/uvx"
arguments = ["kagimcp"]
checksum.algorithm = "sha256"
checksum.value = "8ff70dc528c434469b43a1b05f752f46d8abe41c010edcbff6e5f3cc3131f2f3"
type = "stdio"
variables = ["KAGI_API_KEY"]
```

Two problems compound:

1. `checksum.value` hashes `uvx`, not `kagimcp`.
   The hash protects the runtime, not the tool the user actually cares about.
2. `uvx kagimcp` resolves to the latest PyPI release on each run.
   When upstream shipped a breaking change, the user got it silently — the
   config looked pinned but wasn't.

The field conflates two concerns:

- **Identity pinning** — "run this specific version of this tool."
- **Content verification** — "refuse to run if the bytes differ from this
  hash."

They coincide only when `command` *is* the artifact (a static binary).
For every launcher-style command they diverge, and the field today silently
produces a false sense of pinning.

### Threat model

Being explicit about what JP defends against shapes the answer:

| Threat                                                     | Who handles it                                                                                                 |
| ---------------------------------------------------------- | -------------------------------------------------------------------------------------------------------------- |
| Supply-chain attack (malicious version published upstream) | The ecosystem's own integrity story: pinned versions + wheel hashes + `uv.lock`, npm integrity, image digests. |
| Local tampering of an on-disk artifact                     | A real file hash, if the artifact has a stable path.                                                           |
| Accidental upgrade to a broken upstream version            | Version pinning. Pure identity, not bytes. **The actual problem hit above.**                                   |

The field today partially addresses (2), pretends to address (1), and doesn't
address (3) at all.
Most of what users hit in practice is (3), and (3) is solved by *pinning the
launcher arguments*, not by hashing anything.

## Design

### User-facing

The top-level shape is a tagged enum, with `type = "command"` preserving today's
behavior:

```toml
# Equivalent to today's checksum field — hashes the `command` binary.
[providers.mcp.example.verify]
type = "command"
algorithm = "sha256"
value = "..."
```

```toml
# Hash an arbitrary file. Useful when the real artifact lives somewhere other
# than `command` (a downloaded JAR, a launcher's resolved cache entry with a
# stable path, etc.).
[providers.mcp.example.verify]
type = "file"
path = "/opt/example/server.jar"
algorithm = "sha256"
value = "..."
```

```toml
# Explicit opt-out. Documentary value: "I considered this and chose not to
# verify." Distinct from omitting the field, which means "unset / no opinion."
[providers.mcp.example.verify]
type = "none"
```

Omitting `verify` entirely remains valid and means the same as today: no
verification is performed.

For the kagimcp case, the recommended fix is **pinning in `arguments`**, not in
`verify`:

```toml
[providers.mcp.kagi]
command = "/Users/jean/.cargo/bin/uvx"
arguments = ["kagimcp@0.1.5"]    # ← uvx's own version-pinning syntax
type = "stdio"
variables = ["KAGI_API_KEY"]
```

The `verify` block is for *bytes*; identity is the launcher's job.

### Internal shape

`StdioConfig.checksum` becomes `verify: Option<VerifyConfig>`.
`VerifyConfig` is a `#[derive(Config)]` tagged enum, following the same pattern
as `McpProviderConfig` (`#[config(rename_all = "snake_case", serde(tag =
"type"))]`):

```rust
#[derive(Debug, Clone, PartialEq, Config)]
#[config(rename_all = "snake_case", serde(tag = "type"))]
pub enum VerifyConfig {
    /// Hash the `command` binary. Current behavior.
    #[setting(nested)]
    Command(CommandVerify),

    /// Hash an arbitrary file at `path`.
    #[setting(nested)]
    File(FileVerify),

    /// Explicit opt-out.
    None,
}

pub struct CommandVerify { algorithm: AlgorithmConfig, value: String }
pub struct FileVerify    { path: PathBuf, algorithm: AlgorithmConfig, value: String }
```

`AlgorithmConfig` (the existing `Sha256` / `Sha1` enum) is reused.
`verify_file_checksum` already takes an arbitrary path; the enum dispatch
chooses which path to feed it.

### Documentation

The section preamble for `[providers.mcp.<name>]` gains a short note: identity
pinning for launcher-style commands belongs in `arguments`, not `verify`, with
examples for `uvx`, `npx`, and `docker run`.

## Drawbacks

- **One-way door on a user-facing config key.** `[providers.mcp.*.checksum]`
  appears in user configs and personal dotfiles; renaming it is Hyrum's Law
  territory.
  A migration helper softens this but doesn't eliminate it.

- **The user's actual problem is one step removed from this RFD.** Version
  pinning isn't directly addressed by `verify`; it's redirected to `arguments`
  
  - ecosystem-native syntax.
    A reader who came in expecting "pin my kagimcp version" needs to be told
    that the answer is `arguments = ["kagimcp@X.Y"]`, not anything inside
    `verify`.
    The documentation has to make this obvious; otherwise the next user trips on
    the same trap.

- **`type = "none"` is arguably redundant** with omitting the field.
  Including it adds a variant for documentary intent only.
  Reasonable people will disagree on whether that's worth the surface area.

## Alternatives

### A — Keep `checksum`, document its limits

Restrict `checksum` to its existing behavior, document that it only verifies the
`command` binary, and point users at launcher-native version pinning for
everything else.

Cheapest change.
Doesn't fix the misleading name — `checksum` on a `uvx` command still *looks*
like it pins the tool.
The naming continues to lie even with documentation.

### C — Ecosystem-aware verify variants

`type = "uv_package"`, `type = "npm_package"`, `type = "docker_image"`.
Each variant knows its ecosystem's package identity and (optionally) its
integrity-hash format.
JP would parse `command + arguments`, confirm the named package/version matches,
and verify the on-disk artifact through the ecosystem's own integrity story.

Most capable.
Rejected for now:

- **Cost of abstraction.** Each ecosystem is a maintenance contract — cache
  layouts shift, hash formats evolve, the three ecosystems aren't aligned.
  The variant list grows monotonically as new launchers appear (Cargo, Go, Bun,
  …).
- **Tesler's Law.** The complexity moves from JP-knows-package-managers to
  user-knows-package-managers.
  The latter is where users already are.
- **Zawinski's Law.** JP is a CLI tool, not a binary distribution platform.

Deferred, with a condition for flipping (see Open Questions).

## Non-Goals

- **JP is not a package manager.** Verifying the content of artifacts that a
  launcher resolves at runtime (PyPI wheels, npm tarballs, container layers) is
  out of scope.
  That's the launcher's job.

- **No DSL for identity pinning.** `verify` describes bytes, not identity.
  Version pinning happens in `arguments` using whatever the launcher accepts
  (`uvx kagimcp@0.1.5`, `npx pkg@1.2.3`, `docker run image@sha256:…`).

- **No automatic resolution of "latest version" issues.** A user who writes
  `arguments = ["kagimcp"]` with no version still gets latest.
  `verify` does not paper over that.

## Risks and Open Questions

- **Migration churn.** `checksum` exists in user configs today.
  Phase 1 accepts it as a deprecated alias with a `tracing::warn!`; Phase 2
  removes it.
  Need to decide how long Phase 1 runs.

- **`type = "none"` — keep or drop?** Worth a quick read by a fresh pair of
  eyes.
  Default position: keep, because it documents intent.
  Reasonable counter-position: drop, because YAGNI and omitting the field is
  equivalent.

- **When (if ever) do we move to alternative C?** Concrete condition: more than
  one user reports the same identity-pinning confusion *for the same ecosystem*,
  and the right recommendation each time is the same shape.
  Until then, ecosystem-native pinning wins.

- **Does `type = "file"` have stable paths to point at in practice?** For `uvx`
  the cache layout is not part of `uv`'s contract; for `npx` similar.
  This variant is most useful for self-managed installations (downloaded JAR,
  `uv tool install`-ed entry point, etc.).
  Documentation should be honest about that.

## Implementation Plan

### Phase 1: Introduce `verify`, accept `checksum` as a deprecated alias

- Add `VerifyConfig` enum and `verify: Option<VerifyConfig>` field on
  `StdioConfig` in `jp_config`.
- Update `jp_mcp::client::launch_*` to dispatch on `verify` variants when
  present; fall back to `checksum` when only the legacy field is set.
- Emit `tracing::warn!("[providers.mcp.*].checksum is deprecated; use .verify
  instead")` when the legacy field is observed.
- Update doc comments on the partial / resolved types per the `jp_config`
  doc-comment guide.
- Update the configuration reference page with the new shape and a migration
  note.

Mergeable independently.

### Phase 2: Remove `checksum`

- Remove the legacy field and its dispatch from `jp_mcp`.
- Document the removal in the change-log.

Depends on Phase 1 having shipped for at least one release.

## References

- [`verify_file_checksum`][verify-impl] — current implementation
- [`StdioConfig`][stdio-config] — current field shape
- [`uv` tool spec syntax][uv-tool-spec] — version pinning for `uvx`

[stdio-config]: ../../../crates/jp_config/src/providers/mcp.rs
[uv-tool-spec]: https://docs.astral.sh/uv/concepts/tools/#package-specification
[verify-impl]: ../../../crates/jp_mcp/src/client.rs
