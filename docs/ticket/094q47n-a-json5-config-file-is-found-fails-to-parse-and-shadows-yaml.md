# A `.json5` config file is found, fails to parse, and shadows `.yaml`

- **Status**: Todo
- **Kind**: Bug
- **Authors**: jp
- **Date**: 2026-08-27

JP probes for configuration files with the extensions `toml`, `json`, `json5`,
`yaml`, `yml`, in that order (`crates/jp_config/src/util.rs:22` and
`crates/jp_config/src/fs.rs:18`).

The loader that parses them does not handle JSON5.
Its JSON format accepts `json` and `jsonc` only
(`crates/contrib/schematic/src/config/formats/json.rs:15-19`), so a
`config.json5` file is found by the probe and then fails with
`NoMatchingFormat`.

The shadowing is the worse half.
`json5` is probed before `yaml`, so an unparseable `config.json5` also hides a
perfectly good `config.yaml` sitting next to it.
The user sees a parse error naming a file they may not have known was being
read.

The two halves of the crate disagree about this.
`jp_config::fs::Format` *does* handle JSON5, via `serde_json5`
(`crates/jp_config/src/fs.rs:113`), which is presumably why the extension is in
the probe list at all.
So `ConfigFile` can read JSON5 while the implicit-loading path cannot.

## Options

1. **Drop `json5` from both probe lists.** Two constants, `util.rs:22` and
   `fs.rs:18`.
   A `.json5` file then goes unnoticed rather than breaking the directory it
   sits in.
   Smallest fix.
2. **Register a JSON5 format in schematic.** `serde_json5` is already a
   dependency.
   Makes the probe list honest instead of trimming it.
3. **Add `jsonc` to the probe list.** The reverse gap: `config.jsonc` parses
   fine but is never found, so it is only reachable through an explicit `--cfg
   @path.jsonc`.

Pick 1 or 2 depending on whether JSON5 is worth supporting; 3 closes the
opposite mismatch either way.

## Context

Recorded as a known caveat in RFD 079's "File extensions" section, and now
carried as a warning in `docs/configuration.md` under "File Formats".
Found again while checking that document against the code.
Deleting the warning from the user documentation is part of fixing this.
