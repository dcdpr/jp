# A JSON --cfg naming one map entry clears the whole map

- **Status**: Done
- **Authors**: jp
- **Date**: 2026-11-07
- **Label**: domain=plugins
- **Label**: package=jp_config
- **Label**: type=bug

A root-level JSON `--cfg` that names one entry of a map replaces the whole map,
dropping the entries earlier layers set:

```sh
jp --cfg '{"plugins":{"command":{"ticket":{"options":{"assistant":"Ada via jp"}}}}}' …
```

leaves every plugin without its `run` policy, so a non-interactive turn refuses
all plugin-served tools (`Plugin not admitted; its tools are left out.`).
The dotted form, `--cfg plugins.command.ticket.options.assistant=Ada via jp`,
keeps them.

`KvAssignment::root_json` documents the opposite: a root JSON object is meant to
work like the dotted keys it spells out.
The JSON reaches `plugins.command` with no key left, and `assign_to_entry`
treats that as the whole-collection form, where `Strategy::Set` calls
`map.clear()` before assigning the named entries
(`crates/jp_config/src/assignment.rs`).

Found through `jp serve chat`, whose turns lost the ticket, RFD, and wiki tools;
the bridge now sends dotted keys (`the_bridge_sets_one_key_per_cfg_entry`).
`a_dotted_plugin_option_keeps_the_run_policy` in `jp_config` pins the dotted
form.

- [x] Decide whether a map reached through a root JSON object merges its
      entries, as `root_json`'s doc says, while `KEY:={...}` keeps replacing
- [x] Test the JSON form keeps other plugins' `run`

## Comments

-----

- **From**: jp
- **Date**: 2026-11-07T22:09:28Z

Decided: the operator applies only to the key it is written on.
`KvAssignment` carries a `KeySource` (`Written` / `Spelled`); keys spelled out
of an object, and the root object's empty key, are `Spelled`.
Only `:=` on a `Written` key clears a map or lets a strategy wrapper replace
outright, so:

- `{"plugins":{"command":{"ticket":…}}}` and `plugins:={"command":{…}}` merge
  into the map.
- `plugins.command:={…}` still replaces it.
- A `{ value, strategy }` wrapper reached through an object (map or list)
  combines under its strategy, as in a config file.
- Plain lists inside the object still replace; `null` still clears.

Tests in `crates/jp_config/src/assignment_tests.rs`, from
`a_root_object_naming_one_map_entry_keeps_the_others` onward.
`docs/configuration.md` documents the map/list behaviour of the JSON form.

Not handled: a conversation that ran a turn with the JSON form before this fix
may have recorded the dropped map keys as unsets (`report_dropped_keys` in
`delta.rs`), and those stay in its stream.
