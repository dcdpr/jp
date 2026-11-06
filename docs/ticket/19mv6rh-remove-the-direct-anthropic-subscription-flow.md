# Remove the direct Anthropic subscription flow

- **Status**: Todo
- **Kind**: Chore
- **Authors**: jp
- **Date**: 2026-10-28
- **Label**: domain=llm
- **Label**: llm-provider=anthropic
- **Label**: package=jp_cli
- **Label**: package=jp_config
- **Label**: package=jp_llm
- **Label**: type=task

`providers.llm.anthropic.subscription_flow = "direct"` sends JP-stored Claude
Pro/Max OAuth tokens straight to the Messages API.
Anthropic does not sanction that use, and RFD 110 already made the Claude Code
(ACP) route the default.
Remove the direct flow.
Anyone who still wants it can ship it as an unofficial plugin once providers are
pluggable.

## Scope

- `jp_config`: the `subscription_flow` setting and `SubscriptionFlow`
  (`providers/llm/anthropic.rs`), their tests, and the config schema snapshots.
- `jp_llm`: the `SubscriptionFlow::Direct` arms of credential resolution
  (`provider/anthropic/resolve.rs`), the Claude Code identity line and its
  override (`CLAUDE_CODE_IDENTITY_LINE`, `IDENTITY_LINE_OVERRIDE` in
  `provider/anthropic.rs`), and `provider/anthropic/oauth.rs` if nothing on the
  ACP path uses it.
- `jp_cli`: `--direct` and the direct `--setup-token` path of `jp provider llm
  auth login` (`cmd/provider.rs`).

## To decide while doing it

- Stored conversations and config files that set `subscription_flow = "direct"`
  lose the field on load, so their `subscription` entries resolve through Claude
  Code instead.
  Warn when the field is dropped rather than switching routes silently.
- JP-stored Anthropic OAuth credentials become unused: leave them, or remove
  them on `jp provider llm auth logout`.
- RFD 110 describes `direct` as an explicit option; add a note to it.
