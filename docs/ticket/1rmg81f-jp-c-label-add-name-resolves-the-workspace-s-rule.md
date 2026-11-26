# jp c label add :name resolves the workspace's rule

- **Status**: Todo
- **Authors**: jp
- **Date**: 2026-11-25
- **Label**: client=cli
- **Label**: package=jp_cli
- **Label**: type=bug

`jp conversation label add :name` and `set :name` are meant to resolve the
`conversation.labels.name` rule from the targeted conversation's config.
They resolve it from the files + env config instead.

`Label::conversation_load_request` sets `config_conversation = Some(0)` for
`add` and `set` (`crates/jp_cli/src/cmd/conversation/label.rs`), and the comment
there says the rule "comes from that conversation's effective config, not the
workspace's".
But `Commands::apply_conversation_config` (`crates/jp_cli/src/cmd.rs`) returns
the partial unchanged for `Commands::Conversation`, so the request flag is
ignored and `ctx.config()` never includes the conversation's layer.

The difference shows when the conversation's rule differs from the files' rule:
a rule changed on the conversation with `jp config set --id`, or a rule edited
in a config file after the conversation was created.
Label rules are pinned per key, so a rule only the files declare resolves the
same either way.

`only_the_applying_verbs_layer_the_target_config` in `label_tests.rs` checks
only the request flag, not that the layer reaches the config.

- [ ] `apply_conversation_config` layers the conversation's config for `jp c
      label add` and `set`
- [ ] A test resolves `:name` against a conversation whose rule differs from the
      file layer's
