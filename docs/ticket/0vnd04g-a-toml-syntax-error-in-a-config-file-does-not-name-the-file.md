# A TOML syntax error in a config file does not name the file

- **Status**: Todo
- **Kind**: Bug
- **Authors**: jp
- **Date**: 2026-10-01
- **Label**: package=jp_config
- **Label**: package=schematic
- **Label**: type=bug

A config file with broken TOML syntax produces an error that says what is wrong
and where in the file, but not which file:

```text
Configuration error: TOML parse error at line 1, column 11
  |
1 | [assistant
  |           ^
unclosed table, expected `]`
```

Input that hits it: a user saves `.jp/config.toml`, a `.jp.toml`, or a file
reached through `extends` halfway through an edit.
JP loads several config files per run, so "line 1, column 11" without a path
leaves the user guessing which one.
`jp` startup reports it this way, and so does a plugin's `read_config` with
`reload: true` (T-0vmm72x).

## Cause

`TomlFormat::parse`
(`crates/contrib/schematic/src/config/formats/toml.rs:25-27`) parses in two
steps.
A syntax error from `toml::Deserializer::parse` is wrapped as
`ConfigError::Handler(HandlerError(error.to_string()))`.
Only the second step, deserializing into the config type, returns a
`ParserError`.

`ConfigLoader::map_parser_error`
(`crates/contrib/schematic/src/config/loader.rs:259-267`) fills in the file
location only for `ConfigError::Parser`, so the `Handler` variant passes through
without one.
A type error in the same file does name it (`Failed to parse <location>.`).

JSON and YAML do not have this gap: both send syntax errors through
`ParserError`, so the loader attaches the location.

## Fix

Return a `ParserError` for the syntax step too, carrying the TOML diagnostic as
its message, so `map_parser_error` attaches the location the same way it does
for type errors.
Pin the full message for a malformed file in a schematic test, and check that
the `read_config` reload test in
`crates/jp_cli/src/cmd/plugin/dispatch_tests.rs`
(`a_reload_of_an_unparseable_config_file_is_an_error`) picks up the file name.
