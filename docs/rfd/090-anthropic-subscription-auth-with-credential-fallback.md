# RFD 090: Anthropic Subscription Auth with Credential Fallback

- **Status**: Accepted
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-07-03

## Summary

JP gains support for Anthropic's subscription plans (Claude Pro/Max) via the
OAuth flow used by Claude Code, alongside the existing API key auth.
Credentials are stored as named profiles in a user-global credential store, and
each provider resolves an ordered credential chain: when a subscription account
exhausts its quota, JP automatically falls back to the next credential in the
chain — another subscription account, or the per-token API key — and continues
the conversation without interruption.

## Motivation

JP only supports Anthropic's platform API, authenticated with an API key and
billed per token.
Anthropic also sells fixed-price subscriptions with a token allowance.
Users who hold both should be able to spend the allowance first and overflow to
per-token billing, without babysitting the switch.

Supporting this requires more than a second header: OAuth credentials are
*stateful*.
They live outside config, expire, refresh over the network, and can be
temporarily exhausted.
Today's auth model — a static env var name in config, read synchronously at
provider construction — cannot represent any of that.
If we do nothing, subscription holders pay twice: once for the allowance they
cannot use through JP, and again per token.

## Design

### What the user sees

Log in once per subscription account:

```sh
jp auth login anthropic                    # default profile
jp auth login anthropic --profile work     # named profile
```

The command opens the browser for Anthropic's OAuth consent, captures the
callback on localhost (with a paste-the-redirect-URL fallback for headless
machines), and stores the resulting tokens.
The token response may include the account UUID and email; when absent, JP
recovers them from the Claude CLI bootstrap endpoint using the fresh access
token.
The command confirms which account was linked and refuses to store the same
account under two profiles, keyed on the account UUID — never email alone.
A profile without a resolved account UUID — even when an email was recovered —
is stored as unverified with a warning, and duplicate detection is skipped for
it.

Inspect and remove stored credentials without touching the store by hand:

```sh
jp auth list                               # profiles, accounts, expiry, cooldowns
jp auth logout anthropic --profile work    # remove a profile
```

`jp auth list` shows each profile's state: valid, expired, needing re-login,
cooling down, or unverified.
There is no manual cooldown-reset command, deliberately: a cooldown clears on
its own, or via logout and re-login.

Configure the credential chain, in fallback order:

```toml
[providers.llm.anthropic]
auth = ["oauth:personal", "oauth:work", "api_key"]
```

- `api_key` resolves via `api_key_env`, exactly as today.
- `oauth` (bare) resolves the sole configured profile; `oauth:<name>` names one.
  Profile names are case-sensitive.
- Bare `oauth` with zero or with multiple configured profiles, and
  `oauth:<name>` naming a profile not in the store, are resolution errors naming
  the fix; an empty `auth` list, duplicate entries, and unrecognized items are
  config validation errors.
- The default is `["api_key"]`: existing setups behave identically.
- Across config layers, `auth` replaces as a whole — it never appends.
  An appending merge would let a workspace config silently add paid fallback
  beneath a user's subscription-only global config.
  `JP_CFG_PROVIDERS_LLM_ANTHROPIC_AUTH` overrides it like any other key.

Chain order is the consent model: listing `api_key` after an OAuth profile is
explicit authorization to continue on per-token billing when subscriptions are
exhausted.
There is no additional prompt or setting.

When the active subscription account runs out of tokens, JP prints a one-line
notice — `subscription limit reached (personal) — continuing with work` — and
retries the request with the next credential in the chain.
The fallback is automatic but never silent: switching from fixed-cost to
per-token billing is a money decision, and the notice is the audit trail.
The notice is chrome on stderr per [RFD 048]; under `--format json` it renders
as NDJSON on stderr like all chrome.

### Credential store

A user-global JSON file (`credentials.json` in JP's user data directory, created
with `0600` permissions), keyed by provider and profile:

```json
{
  "anthropic": {
    "personal": {
      "type": "oauth",
      "access_token": "…",
      "refresh_token": "…",
      "expires_at": "2026-07-03T12:00:00Z",
      "account_id": "…",
      "email": "jean@example.com",
      "exhausted_until": null
    }
  }
}
```

The store is provider-agnostic in shape but Anthropic-only in implementation.
All mutations (refresh, exhaustion marking) happen under a file lock, because
refresh tokens rotate: two processes racing a refresh can invalidate each
other's tokens.
Writes are atomic: mutate under the lock, write a temp file, and rename it over
the store, so a crash mid-refresh cannot lose a rotated refresh token.
A refresh rejected by the token endpoint marks the profile as needing re-login;
resolution skips it with a notice and continues down the chain.

`exhausted_until` persists quota cooldowns across invocations.
Subscription limits reset on rolling windows and the quota error carries reset
timing; recording it means a fresh `jp` invocation resolves straight past the
exhausted profile instead of burning a failed request rediscovering it.

### Credential resolution

Resolution is a shell concern.
A resolver in `jp_cli` walks the configured chain before provider construction:
skip profiles whose `exhausted_until` is in the future, refresh the access token
if expired (under the file lock), and hand the provider a resolved credential —
either an API key or a bearer token.
`jp_llm` providers stay free of storage and refresh I/O.
Provider construction takes the config plus the resolved credential:
`TryFrom<&AnthropicConfig>`, which reads the env var today, is replaced by a
constructor like `Anthropic::new(config, credential)`; env lookup leaves
`jp_llm` entirely.
The shell retains the identity of the credential it resolved (profile name and
kind) and pairs it with any stream error at retry time; credential identity
never crosses the provider boundary.

This sits upstream of the provider SDKs, so it survives a later migration of the
streaming layer unchanged: whatever builds the request receives auth material as
data.

### Quota fallback

`StreamErrorKind::InsufficientQuota` exists and `looks_like_quota_error` already
matches Anthropic's quota error text.
Today the kind is terminal.
It becomes conditionally retryable:

1. A request fails with `InsufficientQuota` while an OAuth profile is active.
   The provider's error classifier populates an optional quota-reset timestamp
   on the `StreamError` — distinct from `retry_after`, whose backoff semantics
   don't fit a subscription cooldown.
2. The stream retry module in `jp_cli` marks the profile exhausted in the store
   (using the reset timestamp), prints the notice, and returns a
   credential-switch outcome, distinct from a plain retry.
3. The turn loop re-resolves the chain, rebuilds the provider from the new
   credential, and re-enters the streaming phase immediately — no backoff
   sleep; this is not a transient error, and the new credential is usable now.
   Model details are retained across the switch; they are account-independent.
4. If the chain is exhausted, the error is terminal, exactly as today.

The turn loop therefore receives a provider source — the resolver plus the
provider config — rather than a pre-built provider.

In-flight fallback is scoped to the query streaming loop.
Other LLM call sites (conversation edit, summarize, inquiry collection, title
generation) resolve the chain at provider construction, so a persisted cooldown
routes them past exhausted profiles; a quota error mid-request there remains
terminal, exactly as today.

Anthropic enforces quota at request admission and lets an in-flight stream
finish (it goes "into debt" rather than cutting mid-stream), so fallback
normally happens on a clean request boundary.
Should a stream ever die mid-flight anyway, the existing retry flow already
handles it: partial content is flushed to the `ConversationStream`, the turn
loop rebuilds the thread including that content, and the fresh stream — now on
the fallback credential — continues from there.
No new recovery machinery is needed.

### Request authentication

OAuth requests differ from API key requests in two headers: `Authorization:
Bearer <token>` instead of `x-api-key`, plus `anthropic-beta: oauth-2025-04-20`.
The `async-anthropic` fork the project already maintains grows a bearer-auth
mode.
The bearer-auth mode emits the OAuth beta header itself; user-configured
`beta_headers` merge separately and can neither remove nor duplicate it, and the
header is never sent with API key auth.

### OAuth flow mechanics

The login flow is the one Claude Code uses, well-documented by multiple
open-source implementations:

1. Generate a PKCE verifier/challenge and random state.
2. Open `https://claude.ai/oauth/authorize` with Claude Code's client ID and the
   inference scopes (`user:inference` is only granted via this endpoint; the
   platform console endpoint issues API-key-management tokens only).
3. Capture the callback on a localhost port, or accept a pasted redirect URL.
4. Exchange the code at `https://api.anthropic.com/v1/oauth/token`.
5. Store `{access, refresh, expires, account_id, email}`.

Refresh uses the same token endpoint with `grant_type: refresh_token`.
The exchange and refresh logic is pure (types in, types out); the callback
server, browser opening, and store I/O form the thin imperative shell around it.

## Drawbacks

- **Prompt cache misses on fallback.** Prompt caching is scoped to the account.
  The first request after a credential switch misses every cache breakpoint and
  pays full input-token price on the entire conversation history.
  One-time per switch; subsequent requests re-cache.
- **JP owns secrets on disk.** Today JP never stores credentials; this design
  introduces a token file JP must protect and users must know about.
  Until tool sandboxing exists, any same-user process — including JP's own
  approved tools — can read it.
  `0600` limits exposure on Unix; Windows relies on default user-profile ACLs,
  matching what Claude Code does there.
- **Maintenance of an undocumented flow.** The OAuth endpoints, client ID, and
  headers are Claude Code implementation details, not published API.
  Anthropic can change them without notice, and JP owns the breakage.

## Alternatives

- **Proxy (e.g. CLIProxyAPI) plus `base_url` override.** Works today with zero
  JP code, but requires a separate daemon, keeps tokens outside JP, and cannot
  do per-request fallback across JP's credential chain.
  Remains available as a workaround; not a feature.
- **Reuse Claude Code's credential store.** No login flow to build, but the
  format is undocumented and platform-specific (file on Linux, Keychain on
  macOS), and refresh-token rotation means JP and Claude Code would invalidate
  each other's sessions.
  Could become an optional import source later; unfit as the primary mechanism.
- **Adopt an existing OAuth crate.** The candidates have single-digit-to-low
  double-digit download counts.
  Credentials handling is the wrong place for an unproven dependency, and the
  flow is ~300 lines.

## Non-Goals

- **OAuth for other providers.** The store schema and chain are
  provider-agnostic by construction, but only Anthropic is implemented.
  OpenAI Codex OAuth has the same shape and can follow as its own RFD.
- **Multi-account routing UX.** Ordering in the `auth` chain is the only routing
  mechanism.
  Per-conversation account pinning, usage-based routing, and similar are out of
  scope.
- **OS keychain storage.** The file-based store matches JP's threat model (the
  user's own machine) and what Claude Code itself does on Linux.
  A keyring backend can be added behind the same store interface later.
- **Sharing tokens with Claude Code.** JP's login is independent.
  Users who log the same account into both tools may see one tool's session
  invalidated by the other's refresh; that is inherent to Anthropic's token
  rotation, not solvable here.

## Risks and Open Questions

- **Policy risk.** Using subscription tokens outside Claude Code relies on
  Anthropic's tolerance, currently reported second-hand as "allowed", not
  published policy.
  Anthropic has previously cut off third-party tools using this flow.
  The feature can break by fiat; API key auth remains the supported path.
- **System prompt enforcement.** Historically, inference with OAuth tokens
  required the system prompt to begin with Claude Code's identity line.
  If this is enforced, JP must decide — explicitly, in this RFD before
  acceptance — whether prepending that line is acceptable.
  This is a policy call, not an engineering one.
  Phase 1 exists to answer it empirically before further investment.
- **Thinking signatures across accounts.** Signatures minted under one account
  may be rejected under another after a fallback switch.
  The existing stale-signature recovery (strip and retry) should absorb this;
  confirm during manual testing.
- **Quota error shape.** The reset-timing field on quota errors needs verifying
  against the live API; if absent, `exhausted_until` falls back to a fixed
  conservative cooldown.

## Implementation Plan

### Phase 1: bearer auth, store, and setup-token login

Bearer mode in the `async-anthropic` fork; the credential store with file
locking and profile schema; chain resolution in `jp_cli`; `jp auth login
anthropic --setup-token` accepting a pasted long-lived token from `claude
setup-token`; `jp auth list` and `jp auth logout`, so the store is never
manageable only by hand-editing.
This exercises the full request path — including the system-prompt and
signature questions above — before the browser flow is built.
Independently reviewable and useful on its own.

### Phase 2: quota fallback

Reclassify `InsufficientQuota` as retryable-via-credential-switch, exhaustion
marking with persisted cooldown, the fallback notice, and no-backoff retry.
Depends on Phase 1.

### Phase 3: PKCE browser login

The full `jp auth login anthropic` flow: PKCE, browser, localhost callback,
paste fallback, refresh-on-expiry.
Depends on Phase 1; independent of Phase 2.

## References

- [Anthropic OAuth flow reference implementation (oh-my-pi)][oh-my-pi]
- [OpenClaw OAuth concepts][openclaw] — token sink, refresh rotation, profile
  routing
- [CLIProxyAPI][cliproxy] — proxy-based alternative
- [Using Claude Code with your Pro or Max plan][claude-plans]

[RFD 048]: 048-four-channel-output-model.md
[claude-plans]: https://support.claude.com/en/articles/11145838-using-claude-code-with-your-pro-or-max-plan
[cliproxy]: https://github.com/router-for-me/CLIProxyAPI
[oh-my-pi]: https://github.com/can1357/oh-my-pi/blob/75bdb20212871221406e119745136edcb2197653/packages/ai/src/registry/oauth/anthropic.ts
[openclaw]: https://docs.openclaw.ai/concepts/oauth
