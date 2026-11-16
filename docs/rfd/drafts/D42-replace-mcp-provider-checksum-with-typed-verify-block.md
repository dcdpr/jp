# RFD D42: Replace MCP Provider Checksum with Typed Verify Block

- **Status**: Draft
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-05-22
- **Summary**: Replaces `[providers.mcp.*].checksum` with a tagged `verify`
  block that distinguishes binary hash verification from launcher version
  pinning.

## Summary

This RFD replaces `[providers.mcp.*].checksum` with a tagged `verify` block that
names the artifact to verify.
Hashing `command` alone only protects static binaries; launcher commands (`uvx`,
`npx`, and `docker run`) resolve the tool at runtime and pin versions through
`arguments`.

## Motivation

`[providers.mcp.<name>].checksum` hashes `config.command` (see
[`verify_file_checksum`][verify-impl]) to verify that the MCP server binary has
not changed on disk.
That works when `command` is a static binary, but breaks when `command` is a
launcher that resolves the tool at runtime.

For example, a user configured `kagimcp` with a checksum:

```toml
[providers.mcp.kagi]
command = "/Users/jean/.cargo/bin/uvx"
arguments = ["kagimcp"]
checksum.algorithm = "sha256"
checksum.value = "8ff70dc528c434469b43a1b05f752f46d8abe41c010edcbff6e5f3cc3131f2f3"
type = "stdio"
variables = ["KAGI_API_KEY"]
```

This configuration has two problems:

1. `checksum.value` hashes the `uvx` runtime binary, not `kagimcp`.
2. `uvx kagimcp` resolves to the latest PyPI release on each run.
   When upstream published a breaking change, `uvx` pulled it silently even
   though the config looked pinned.

`checksum` conflates two distinct concerns:

- **Identity pinning**: running a specific version of a tool.
- **Content verification**: refusing to run if a file's bytes do not match an
  expected hash.

The two coincide only when `command` is the artifact itself.
For launcher commands they diverge, and `checksum` gives a false impression that
the tool version is pinned.

### Threat Model

MCP server execution faces three distinct threats:

| Threat                                                     | Mitigation                                                                                                         |
| ---------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------ |
| Supply-chain attack (malicious version published upstream) | Package-manager integrity checks: pinned versions, wheel hashes and `uv.lock`, npm integrity hashes, image digests |
| Local tampering with an on-disk artifact                   | File hash verification, when the artifact has a stable path on disk                                                |
| Accidental upgrade to a broken upstream version            | Version pinning in launcher arguments (identity, not bytes)                                                        |

`checksum` partially addresses local tampering (only when `command` is the
artifact), gives the illusion of addressing supply-chain attacks, and does
nothing for accidental upgrades.
In practice, users hit accidental upgrades, which are solved by pinning the
version in the launcher's `arguments` rather than hashing a file.

## Design

### User-Facing Configuration

`[providers.mcp.<name>].verify` is a tagged enum with three variants.
`type = "command"` hashes the `command` binary, preserving the behavior of
`checksum`:

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

Omitting `verify` performs no verification.

For the `kagimcp` example, the fix is pinning the version in `arguments` rather
than configuring `verify`:

```toml
[providers.mcp.kagi]
command = "/Users/jean/.cargo/bin/uvx"
arguments = ["kagimcp@0.1.5"]    # ← uvx's own version-pinning syntax
type = "stdio"
variables = ["KAGI_API_KEY"]
```

`verify` checks file bytes on disk; the launcher pins package identity.

### Internal Representation

`StdioConfig.checksum` is replaced by `verify: Option<VerifyConfig>`.
`VerifyConfig` is a `#[derive(Config)]` tagged enum using the same
representation as `McpProviderConfig` (`#[config(rename_all = "snake_case",
serde(tag = "type"))]`):

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

`CommandVerify` and `FileVerify` reuse the existing `AlgorithmConfig` (`Sha256`
/ `Sha1`) enum.
`verify_file_checksum` already accepts an arbitrary path, so dispatching on
`VerifyConfig` only selects which path to pass.

### Documentation

The `[providers.mcp.<name>]` section preamble will note that version pinning for
launcher commands belongs in `arguments` rather than `verify`, with examples for
`uvx`, `npx`, and `docker run`.

## Drawbacks

- **Breaking change to an existing config key.** `[providers.mcp.*.checksum]`
  already appears in user configs and dotfiles (Hyrum's Law).
  Accepting `checksum` as a deprecated alias softens the transition but does not
  eliminate the migration cost.

- **Version pinning still happens outside `verify`.** `verify` does not pin
  launcher packages; users must pin versions in `arguments` using each
  launcher's native syntax (such as `arguments = ["kagimcp@X.Y"]`).
  Documentation must make this clear so users looking to pin a tool version do
  not reach for `verify`.

- **`type = "none"` duplicates omitting the field.** The variant exists only to
  record an explicit opt-out in config files, which adds surface area without
  new runtime behavior.

## Alternatives

### Keep `checksum` and Document Its Limits

Keep `checksum` as-is, document that it only hashes the `command` binary, and
direct users to launcher-native version pinning for everything else.

This is the smallest change, but it leaves the misleading key name in place:
`checksum` on a `uvx` provider still looks like it pins the tool, even when the
documentation says otherwise.

### Ecosystem-Aware `verify` Variants

Add variants such as `type = "uv_package"`, `type = "npm_package"`, and `type =
"docker_image"` that understand each ecosystem's package identity and optional
integrity hash format.
JP would parse `command + arguments`, confirm that the named package and version
match, and verify the on-disk artifact using the ecosystem's own integrity
checks.

This approach is the most capable, but is rejected for now:

- **Maintenance cost.** Supporting each ecosystem is an ongoing maintenance
  contract: cache layouts shift, hash formats evolve, and ecosystems do not
  share conventions.
  The variant list would grow with every new launcher (Cargo, Go, Bun, and
  others).
- **Misplaced complexity (Tesler's Law).** Package-manager syntax is complexity
  users already know; re-encoding it in JP's config moves that complexity into
  JP without reducing it.
- **Scope creep (Zawinski's Law).** JP is a CLI tool, not a binary distribution
  platform.

This direction is deferred unless user confusion persists (see Risks and Open
Questions).

## Non-Goals

- **Verifying launcher-resolved packages.** JP is not a package manager.
  Verifying the contents of artifacts that a launcher resolves at runtime (PyPI
  wheels, npm tarballs, container layers) is the launcher's job.

- **A custom DSL for identity pinning.** `verify` describes bytes, not package
  identity.
  Version pinning belongs in `arguments` using the launcher's native syntax
  (`uvx kagimcp@0.1.5`, `npx pkg@1.2.3`, `docker run image@sha256:...`).

- **Preventing unpinned launcher arguments.** A user who writes `arguments =
  ["kagimcp"]` without a version still gets the latest release on each run.
  `verify` does not paper over unpinned arguments.

## Risks and Open Questions

- **Deprecation window for `checksum`.** `checksum` is in active use in user
  configs.
  Phase 1 accepts it as a deprecated alias with a `tracing::warn!`, and Phase 2
  removes it.
  We need to decide how long Phase 1 runs before removal.

- **Whether to keep `type = "none"`.** The default position is to keep it so
  users can document an explicit opt-out.
  The counterargument is to drop it under YAGNI, since omitting `verify` is
  equivalent.

- **When to revisit ecosystem-aware verification.** We should reconsider
  ecosystem-aware variants only if multiple users report the same
  identity-pinning confusion for the same ecosystem and the recommended fix has
  the same shape each time.
  Until then, ecosystem-native pinning in `arguments` is sufficient.

- **Path stability for `type = "file"`.** Neither `uvx` nor `npx` guarantees a
  stable cache layout across versions.
  `type = "file"` is primarily useful for self-managed installations with fixed
  paths (such as a downloaded JAR or a `uv tool install` entry point), and the
  documentation should state this constraint.

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

This phase can merge independently.

### Phase 2: Remove `checksum`

- Remove the legacy field and its dispatch from `jp_mcp`.
- Document the removal in the changelog.

This phase depends on Phase 1 shipping in at least one release first.

## References

- [`verify_file_checksum`][verify-impl]: current implementation
- [`StdioConfig`][stdio-config]: current field shape
- [`uv` tool spec syntax][uv-tool-spec]: version pinning for `uvx`

[stdio-config]: ../../../crates/jp_config/src/providers/mcp.rs
[uv-tool-spec]: https://docs.astral.sh/uv/concepts/tools/#package-specification
[verify-impl]: ../../../crates/jp_mcp/src/client.rs
