# Port the httpmock fork's record feature off serde_yaml

- **Status**: Done
- **Kind**: Chore
- **Authors**: jp
- **Date**: 2026-10-27
- **Label**: domain=tooling
- **Label**: package=jp_test

`serde_yaml` is deprecated, and its `unsafe-libyaml` backend is flagged
unmaintained by RUSTSEC-2024-0449.
JP's own YAML handling (`jp_config::fs`, schematic's `YamlFormat`) uses
`serde-saphyr`.

The remaining path into the lockfile is the `httpmock` fork
(`JeanMertz/httpmock`, branch `tweaks`): its `record` feature depends on
`serde_yaml` to write and replay recordings, and `jp_test` enables `record` for
the provider VCR fixtures.
`jp_test` is only ever a dev-dependency, so this never reaches a shipped binary.

`deny.toml` ignores RUSTSEC-2024-0449 until this is done.

Done when:

- the fork's `record` feature reads and writes recordings through `serde-saphyr`
  (or another maintained YAML crate), and existing fixtures under the `jp_llm`,
  `jp_github`, and `jp_openrouter` test directories still replay unchanged;
- `serde_yaml` and `unsafe-libyaml` are gone from `Cargo.lock`;
- the RUSTSEC-2024-0449 ignore is removed from `deny.toml`.

## Comments

-----

- **From**: jp
- **Date**: 2026-10-27T23:54:59Z

Done in JeanMertz/httpmock@fa299e9: the `record` feature reads recordings with
`serde_saphyr::from_multiple` and writes them with `serde_saphyr::to_string`.
`serde_yaml` and `unsafe-libyaml` are out of `Cargo.lock`, and the
RUSTSEC-2024-0449 ignore is removed from `deny.toml`.
Existing cassettes replay unchanged (`jp_llm`, `jp_github`, `jp_openrouter`,
`async-anthropic`).
