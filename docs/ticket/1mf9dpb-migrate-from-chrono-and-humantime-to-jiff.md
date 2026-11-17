# Migrate from chrono and humantime to jiff

- **Status**: Todo
- **Authors**: jp
- **Date**: 2026-11-17
- **Label**: domain=tooling
- **Label**: type=task

RustSec marks both `chrono` (RUSTSEC-2026-0543) and `humantime`
(RUSTSEC-2026-0542) as unmaintained.
Both advisories recommend `jiff`.
`deny.toml` ignores the two advisories so `cargo deny` passes in CI until the
migration lands.

Usage at time of filing:

- `chrono` is a workspace dependency used by 24 workspace members: most `jp_*`
  crates, the internal crates (`dash`, `doc`, `ticket`), the command plugins
  (`rfd`, `serve-chat`, `serve-web`, `ticket`, `wiki`), `bookworm`, and
  `.config/jp/tools`.
- `humantime` is used by `jp_cli`, `jp_config`, and `jp-serve-chat`.

Persisted timestamps (conversation metadata, events, tickets, wiki pages) are
serialized through chrono's serde impls, so the migration has to keep the
on-disk format readable: same RFC 3339 shape, including fractional seconds where
they are written today.
The macOS app parses `lastActivatedAt` from `jp_ffi` as text, so the FFI JSON
format is part of the contract too.

The workspace may still pull `chrono` or `humantime` in transitively after the
direct uses are gone; check `cargo tree -i` before dropping the ignores.

- [ ] Replace `humantime` with `jiff` in `jp_cli`, `jp_config`, and
      `jp-serve-chat`, keeping the accepted duration syntax in config and CLI
      flags
- [ ] Replace `chrono` with `jiff` across the workspace, with tests pinning the
      serialized timestamp format
- [ ] Remove the RUSTSEC-2026-0542 and RUSTSEC-2026-0543 entries from
      `deny.toml`
