# Let a long-running plugin re-read the workspace configuration

- **Status**: Todo
- **Kind**: Feature
- **Authors**: jp
- **Date**: 2026-10-01
- **Label**: domain=plugins
- **Label**: package=jp_cli
- **Label**: package=jp_plugin

A plugin gets the resolved configuration once, in `init`, and `read_config`
answers from that same value for the plugin's whole lifetime: `run_plugin`
serializes `ctx.config()` when the plugin starts
(`crates/jp_cli/src/cmd/plugin/dispatch.rs:308-331`), and `handle_read_config`
walks that cached JSON (`dispatch.rs:1565-1566`, `2134`).

A plugin that runs for minutes or hours, such as a dev server host, cannot see a
configuration change made while it runs.
The only way it could find out is to watch config files itself.
That means duplicating JP's load order in the plugin: user-global,
`.jp/config.toml`, the `.jp.toml` chain, user-workspace, and every file reached
through `extends`.
The copy breaks the moment JP's config locations change.

Input that hits it: RFD 116's `jp vite` keeps a settings snapshot for the site.
A user edits `plugins.command.ticket.options.dir` while the dev server runs;
without a reload, the snapshot stays on the old directory until someone runs `jp
vite sync` by hand.

## Proposal

1. `read_config` takes `reload: true`.
   The host re-runs its normal load (files, env, `extends`, the invocation's
   `--cfg`) and answers with the fresh result.
   The plugin polls and compares.
   Nothing outside the host needs to know where config lives.
2. Later, the host pushes a `config_changed` message.
   That needs the loader to record which files a load actually read, including
   `extends` targets, which nothing records today.
   RFD 060 (Config Explain) wants the same per-layer source information.

Step 1 is enough for RFD 116.

## Related

- Draft D36 (Live Workspace View for Long-Running Plugin Hosts) fixes the same
  staleness for conversation data.
  It does not cover configuration.
- Once this exists, a plugin hosting the dev server could also route the site's
  writes through itself, so reads and writes share one configuration by
  construction.
  That needs a plugin to run another plugin through the host, which is a
  separate change and not worth designing until a long-running host exists.

## Comments

-----

- **From**: jp
- **Date**: 2026-10-01T10:21:07Z

Step 1 implemented: `read_config` accepts `reload: true` (protocol 10).
The host re-runs the startup load (files, `extends`, env, the invocation's
`--cfg`) through `ConfigPipeline`, answers with the result, and caches it for
later plain `read_config` calls.
A failed load answers with `error` and keeps the last good config.
Step 2 (`config_changed` push) is not started.
