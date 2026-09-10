# RFD 090: Anthropic Subscription Auth with Credential Fallback

- **Status**: Accepted
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-07-03
- **Tracking Issue**: [\#875]

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
jp provider auth login llm.anthropic                   # stored as "default"
jp provider auth login llm.anthropic --name work       # stored as "work"
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
If the missing identity is recovered later (see Credential resolution), the
duplicate check runs at that point.

Inspect and remove stored credentials without touching the store by hand:

```sh
jp provider auth list                                  # profiles, accounts, expiry, cooldowns
jp provider auth logout llm.anthropic --name work      # remove one
```

`jp provider auth list` shows each profile's credential type and state: valid,
expired, needing re-login, cooling down (with the scope, account or model
family, and the cooldown's expiry), or unverified.
An unverified profile is additionally marked usable or needing re-login,
depending on whether account attribution is enforced (see Request
authentication) and whether identity recovery has failed.
There is no manual cooldown-reset command, deliberately: a cooldown clears on
its own, or via logout and re-login.

Credentials are user-global, so auth commands work anywhere: `jp provider auth`
bypasses workspace discovery entirely, the same startup exception `jp init`
uses.
`list` and `logout` require no TTY and are safe to script.
Browser login needs a reachable localhost callback or an interactive paste; when
neither is possible, it fails with a diagnostic naming both options.
Setup tokens are read from stdin, never from process arguments, which leak into
shell history and `ps` output.
Normal LLM commands never start a login implicitly; a missing or expired profile
is an error naming the exact `jp provider auth login` command that fixes it.

Configure the credential chain, in fallback order:

```toml
[providers.llm.anthropic]
auth = ["subscription:personal", "subscription:work", "api_key"]
```

Each entry names how the request is billed, and optionally which credential of
that kind to use.
`api` and `sub` are accepted as shorthand for the two kinds, and both are
written back in full.

- `api_key` resolves via `api_key_env`, exactly as today.
  `api_key_env` may map several named keys, which `api_key:<name>` selects
  between.
- `subscription` (bare) resolves the sole stored credential;
  `subscription:<name>` names one.
  Profile names are case-sensitive.
- A chain entry selects a stored profile; it does not assert the credential's
  mechanism.
  The same entry keeps working when a profile migrates from a static setup token
  to browser OAuth.
- A bare kind with zero or with multiple credentials to choose between, and
  `<kind>:<name>` naming one that does not exist, are preflight errors naming
  the fix; an empty `auth` list, duplicate entries, and unrecognized items are
  config validation errors.
- The default is `["api_key"]`: existing setups behave identically.
- Across config layers, `auth` replaces as a whole — it never appends.
  Merging two chains element-wise produces an order nobody wrote; replacement
  keeps every effective chain one that some layer spelled out in full.
  `JP_CFG_PROVIDERS_LLM_ANTHROPIC_AUTH` overrides it like any other key.

Chain contents are governed by the ordinary config trust model: a workspace
config can rewrite the chain, exactly as it can rewrite `api_key_env` or the
model selection today.
A user who wants their chain to be authoritative pins it in a layer that
outranks workspace files: the user-workspace config or the environment variable.
Listing `api_key` in the effective chain is the authorization to continue on API
billing when earlier entries are exhausted; there is no additional prompt or
setting.

When the active subscription account runs out of tokens, JP prints a one-line
notice — `subscription limit reached (personal) — continuing with work` — and
retries the request with the next credential in the chain.
The fallback is automatic but never silent: a credential switch can change what
the request costs, and the notice is the audit trail.
The notice reports the switch; it does not certify how either side is billed.
A subscription account with paid usage credits enabled can incur paid usage even
under a `profile` entry — Anthropic tracks that as an overage status alongside
the usage window — so JP never promises that OAuth traffic is covered by the
allowance.
The notice is chrome on stderr per [RFD 048]; under `--format json` it renders
as NDJSON on stderr like all chrome.

The auth commands are provider-owned UX: they migrate with the provider into its
command plugin (as `jp anthropic login`, `list`, `logout`) once the
command-plugin protocol supports running without a workspace and prompting for
input.
Until then they live in `jp_cli`; their behavior is the contract, their location
is not.

### Credential store

A user-global store holding one versioned JSON document, keyed by category,
provider, and profile:

```json
{
  "version": 1,
  "credentials": {
    "llm": {
      "anthropic": {
        "personal": {
          "type": "oauth",
          "access_token": "…",
          "refresh_token": "…",
          "expires_at": "2026-07-03T12:00:00Z",
          "account_id": "…",
          "email": "jean@example.com",
          "cooldowns": {}
        },
        "ci": {
          "type": "token",
          "token": "…",
          "account_id": null,
          "email": null,
          "cooldowns": {}
        }
      }
    }
  }
}
```

A credential is a tagged variant.
`oauth` carries a refreshable token pair and expiry; `token` is a static bearer
token (the Phase 1 `claude setup-token` output) with no refresh flow and no
expiry JP can inspect.
`jp provider auth list` reports the variant, so a static token is never
misdescribed as expired or refreshable.

`version` is the migration discriminator: a store written by a newer schema is
rejected with a clear error rather than partially read, and a migration rewrites
the file atomically under the same lock as any other mutation.
Credentials nest under a category (`llm`) and a provider (`anthropic`) rather
than a flat dotted key, so listing one category (every LLM credential, say) is a
direct lookup instead of key-prefix parsing, and future non-LLM categories slot
in without a schema break.
Only the `anthropic` provider under `llm` is implemented.
As a paper check against a second provider: OpenAI Codex OAuth issues the same
material (access/refresh/expiry plus account identity), so the `oauth` variant
and the `llm` category fit it unchanged; any misfit a real implementation
surfaces is absorbed by a version bump in the RFD that extends this one.

All mutations (refresh, cooldown marking) happen under a file lock, because
refresh tokens rotate: two processes racing a refresh can invalidate each
other's tokens.
The lock alone is not enough: a mutation reloads the store and rechecks its
precondition after acquiring it, so a process holding a pre-rotation snapshot
cannot submit an already-rotated refresh token and falsely mark a healthy
profile as needing re-login.
Writes are atomic — the document is replaced whole, never partially — so a
crash mid-refresh cannot lose a rotated refresh token.
The lock-mutate-persist cycle lives in the `jp_credentials` crate:
`CredentialStore` owns the semantics every backend shares — the document
encoding, the schema-version check, and the mutation cycle — while storage
backends implement the `keyring-core` store interface, holding the document as a
single entry.
A file-based store is the baseline backend, and the macOS keyring (Phase 4)
swaps in behind the same interface.
A file-based backend must replace its file atomically (temp file and rename) and
create it with `0600` permissions; if `keyring-core`'s bundled file store does
not provide both, JP's file persistence implements the store interface itself.
The mutation lock is a `ResourceLocker` (`jp_storage`), the same file-based
locking primitive conversation locks are built on, and stays file-based for
every store backend, since keyring stores provide no locking.
A refresh rejected by the token endpoint marks the profile as needing re-login;
resolution skips it with a notice and continues down the chain.

`cooldowns` persists quota cooldowns across invocations, keyed by the window
Anthropic reports as exhausted: `five_hour` and `seven_day` cover the whole
account, while `seven_day_opus` and `seven_day_sonnet` cover one model family.
A single profile-wide timestamp would let an exhausted Opus window block the
same account's Haiku title generation.
Resolution skips a profile only when a cooldown scope matching the requested
model is in the future.
Persisting reset timing means a fresh `jp` invocation resolves straight past the
exhausted scope instead of burning a failed request rediscovering it.

The store is core-owned and reached only through its API.
Providers in core call `jp_credentials` directly; command plugins — and
provider plugins, once those exist — reach the same operations through host
protocol messages, so the on-disk format is never a cross-binary contract.
The API is the versioning contract between JP and its plugins; the schema
`version` above guards the document itself.

### Credential resolution

Resolution is a provider concern.
The Anthropic provider owns its credential policy: it reads the `auth` chain
from its config, walks it per request, refreshes expired access tokens, and
decides when an entry is skipped, retried, or abandoned.
Core owns none of that vocabulary — every provider is constructed the same way,
from its config alone, and the turn loop, tasks, and every other call site are
credential-unaware.
What core owns is the credential store (see Credential store): the provider
loads profiles and records outcomes — cooldowns, re-login markers, recovered
identity — through the `jp_credentials` API.

Resolution runs in two stages, both inside the provider:

1. **Preflight**, synchronous and local, at provider construction, against a
   store snapshot.
   Hard failures are config-shaped: an unparseable chain, an entry that maps to
   no stored profile, or a malformed stored record.
   Everything else is per-entry state, and preflight fails only when no entry in
   the chain could possibly resolve: one broken profile does not block a chain
   whose purpose is to fall past it.
   This is the existing `provider::preflight` seam, uniform across providers: a
   provider that cannot possibly authenticate fails before commands start
   side-effectful work, exactly as one with an unset `api_key_env` does.
2. **Resolution**, asynchronous, before each request the provider sends.
   Walks the chain, skips entries per the table below, refreshes an expired
   access token, and sends the request with the credential it lands on: an API
   key or a bearer token.
   The construction snapshot serves preflight only; a refresh re-reads the store
   under the lock before acting (see Credential store).

Every credential condition has exactly one outcome:

| Condition                                    | Detected at       | Outcome                                       |
| -------------------------------------------- | ----------------- | --------------------------------------------- |
| Empty chain, duplicate or unrecognized entry | config validation | error                                         |
| Entry maps to no stored profile              | preflight         | error                                         |
| Stored record malformed                      | preflight         | error                                         |
| No chain entry can possibly resolve          | preflight         | error                                         |
| Profile marked needs-re-login                | resolution        | skip, notice                                  |
| Profile unverified, attribution optional     | resolution        | resolve normally                              |
| Profile unverified, attribution required     | resolution        | recover identity; on failure skip, notice     |
| Cooldown scope matches requested model       | resolution        | skip                                          |
| `api_key` environment variable unset         | resolution        | skip, notice                                  |
| Refresh rejected by the token endpoint       | resolution        | mark needs-re-login; skip, notice             |
| Credential refused mid-request (`401`/`403`) | streaming         | mark needs-re-login; advance chain, notice    |
| Subscription-window exhaustion               | streaming         | record scoped cooldown; advance chain, notice |
| API billing exhaustion                       | streaming         | advance chain, notice                         |
| Access token expired                         | streaming         | refresh, retry same credential                |
| Transient rate limit (plain 429)             | streaming         | retry same credential with backoff            |
| Chain exhausted                              | any stage         | terminal error                                |

Two rows pin down behavior worth naming.
A missing `api_key` environment variable is a skip in a multi-entry chain, so a
missing key cannot block subscription use; under the default `["api_key"]` chain
the skip exhausts the chain and preflight fails exactly as today.
Cooldown recording applies only to stored profiles: `api_key` has no store
entry, so an exhausted key is retried once per invocation and falls through,
with the retry rejected at admission before any tokens are billed.

Unverified profiles resolve according to the attribution requirement Phase 1
fixes (see Request authentication).
If `account_uuid` proves optional, an unverified profile resolves normally:
`device_id` and `session_id` are JP-derived and need no account identity, and
`account_uuid` is omitted; the reference implementation's metadata shape already
tolerates its absence.
If attribution proves required, resolution attempts one identity recovery via
the Claude CLI bootstrap endpoint, the same call login uses.
That endpoint rejects setup tokens (measured; see Phase 1), so a setup-token
profile can never be attributed and stays unverified for its lifetime.
A recovered UUID is persisted under the store lock, and the duplicate-account
check that login skipped runs at that point.
When recovery fails, the profile is non-resolvable: skipped with a notice naming
the exact `jp provider auth login` command while another entry remains, terminal
when the chain exhausts.

Because the provider resolves per request, every request a turn issues — across
tool-execution cycles, and equally for title generation, summarization, or any
other purpose — lands on a usable credential, and an access token expiring
between requests is absorbed by a refresh rather than surfacing as an error.
An expiry surfacing mid-stream is a refresh-and-retry on the same credential,
not a chain advance.
A task whose provider fails preflight skips its work softly, exactly as title
generation does today.
Profile names, chain position, and account identity never leave the provider:
the store API speaks in categories, providers, and profiles, but no other core
component consumes them.
When request enforcement requires account attribution (see Request
authentication), the resolved profile's account UUID is request data the
provider embeds in `metadata.user_id`.
Resolution outcomes a user must see — a skipped profile, a credential switch —
are emitted as notice events on the provider's event stream, generic stream
vocabulary any provider can use, and rendered as chrome on stderr per [RFD 048].

Credential policy travels with the provider: when a provider moves out of core
into a plugin, its resolution logic, wire mechanics, and login UX move with it,
and the credentials API — reachable in-process today, over the plugin protocol
then — is the only seam left behind.

### Quota fallback

Three failure semantics matter here; the current classifier represents two:

- **Transient rate limit.** A plain HTTP 429, today's `RateLimit`; retried in
  place with backoff.
  Unchanged by this design.
- **API billing exhaustion.** Credit-balance and billing errors on per-token
  accounts, today's `InsufficientQuota`; currently terminal.
- **Subscription-window exhaustion.** A subscription account hitting a rolling
  usage window.
  Nothing represents this today: `looks_like_quota_error` matches billing
  phrases, and the Anthropic classifier maps ordinary 429s to `RateLimit`.

Subscription limits are reported in response headers, not in the error body
(measured; see Phase 1).
A `429` carrying `anthropic-ratelimit-unified-representative-claim` or
`anthropic-ratelimit-unified-overage-status` is a quota rejection; a `429`
without them is not, and Claude Code's own client says so in as many words.
That distinction is the classifier: it separates exhaustion from ordinary
capacity throttling and from a rejected request fingerprint, which arrive with
the same status and an opaque body.

The headers also carry what the cooldown needs:

| Header                                             | Use                                                                                  |
| -------------------------------------------------- | ------------------------------------------------------------------------------------ |
| `anthropic-ratelimit-unified-status`               | `allowed`, `allowed_warning`, `rejected`                                             |
| `anthropic-ratelimit-unified-representative-claim` | the exhausted window: `five_hour`, `seven_day`, `seven_day_opus`, `seven_day_sonnet` |
| `anthropic-ratelimit-unified-reset`                | when that window resets, as Unix seconds                                             |
| `anthropic-ratelimit-unified-overage-status`       | whether paid spillover can serve the request                                         |
| `retry-after`                                      | present only when rejected with no overage available                                 |

JP never chooses paid spillover.
A quota rejection is treated the same whether `overage-status` says spillover is
available or exhausted: the cooldown is recorded and the chain advances.
Paid usage happens only because the user listed `api_key` in the chain, which
keeps one consent model rather than two.
The limit of that guarantee: extra usage is an account-level setting, so an
account with it enabled may have requests served past the window and billed as
overage without any rejection for JP to react to.
JP cannot prevent that; it only refuses to select it.

A `401` or `403` is the other credential-scoped failure: the credential itself
was refused, so no amount of retrying helps.
Anthropic distinguishes two cases worth naming, both of which mark the profile
as needing re-login: `OAuth token has been revoked`, and `OAuth authentication
is currently not allowed for this organization`.
Any other `401`/`403` is treated the same way, since a credential the provider
won't accept cannot serve the request whatever the reason.

Exhaustion becomes conditionally retryable via credential switch:

1. A request fails at admission with an exhaustion kind: subscription-window
   exhaustion under a stored profile, or billing exhaustion under any chain
   entry.
   The provider classifies the rejection from the headers above, reading the
   exhausted scope and its reset instant — distinct from `retry-after`, whose
   backoff semantics don't fit a subscription cooldown.
   When a rejection arrives without reset timing, a fixed 30-minute cooldown
   applies, scoped to the whole account.
   The costs are asymmetric, so the fixed value biases short: a too-short
   cooldown wastes one admission-rejected request per expiry (no tokens are
   billed), a too-long one silently spends per-token money while a recovered
   allowance sits idle.
   Observed reset timing dominates whenever available, capped at the longest
   known window (seven days) so a misparsed timestamp cannot brick a profile; a
   cooldown clears on its own or via logout and re-login.
2. The provider records the scoped cooldown through the store API (for stored
   profiles; `api_key` has no entry to record against) and emits the switch
   notice as an event.
   The recorded cooldown is what routes the next resolution past the spent
   profile, so a mid-turn switch and a fresh invocation share one code path.
3. The provider re-resolves the chain and re-sends the request immediately — no
   backoff sleep; this is not a transient error, and the new credential is
   usable now.
   Model metadata (context sizes, structural details) is retained across the
   switch; access is not guaranteed, and a model authorization or unknown-model
   error under the fresh credential is terminal, with an error naming the
   credential and the model.
4. If the chain is exhausted, the original error is terminal, exactly as today.

Every credential in the chain is expected to have access to the models the
conversation uses.
JP does not switch models on a credential's behalf, and it does not probe the
chain for a credential that can serve the model: "this account lacks the model"
and "this model does not exist" are indistinguishable on the wire (both answer
404), and walking the chain on that ambiguity would burn every credential on a
doomed request whenever a model name is simply wrong.

The switch is invisible to the turn loop: it receives a provider and a stream,
and the notice event is the only trace a switch leaves in the event flow.

Fallback applies to every request the provider sends: the query streaming loop,
conversation edit and summarize, inquiry collection, title generation.
Persisted cooldowns route any invocation past exhausted profiles, and an
admission-time rejection advances the chain wherever it occurs — there is no
separate in-flight machinery to scope.

Anthropic enforces quota at request admission and lets an in-flight stream
finish (it goes "into debt" rather than cutting mid-stream), so fallback
normally happens on a clean request boundary.
Should a stream ever die mid-flight anyway, the existing retry flow already
handles it: partial content is flushed to the `ConversationStream`, the turn
loop rebuilds the thread including that content, and the fresh stream — now on
the fallback credential — continues from there.
No new recovery machinery is needed.

### Request authentication

OAuth requests differ from API key requests by far more than the auth header.
The reference implementations send a Claude Code request fingerprint; at the
pinned oh-my-pi revision, the provider module sends for OAuth tokens:

- **Headers**, built by [`buildAnthropicHeaders`][oh-my-pi-headers]:
  `Authorization: Bearer <token>` instead of `x-api-key`; an `anthropic-beta`
  set including `oauth-2025-04-20` plus Claude Code betas
  (`claude-code-20250219`, `interleaved-thinking-2025-05-14`,
  `context-management-2025-06-27`, and more); client identity, a Claude Code
  user agent plus `x-app: cli`; client platform and version headers
  (`X-Stainless-*`, `anthropic-client-platform`, `anthropic-client-version`);
  and per-request identifiers (`x-client-request-id`, plus an optional Claude
  Code session-id header).
- **Request metadata**: `metadata.user_id` carrying `{device_id, session_id,
  account_uuid}`: the active credential's account UUID, a stable device
  identifier, and a per-session UUID.
- **System content**: a system prompt beginning with Claude Code's identity
  line, plus a billing block whose `cch` attestation is a hash computed over the
  serialized request body.
  The identity line is enforced (Phase 1), so JP sends it and immediately
  follows it with a block naming it as a transport artifact and pointing at JP's
  own prompt, which keeps a foreign identity from standing as an instruction;
  the attestation is not sent at all.
- **Behavioral constraints**: an output-token clamp (64k) applied to OAuth
  requests even when the model's ceiling is higher.

How much of this fingerprint Anthropic enforces is unknown; measuring it is a
Phase 1 deliverable, run top-down: start from the complete fingerprint above and
remove elements until the minimum accepted request remains, rather than starting
minimal and reading the absence of immediate errors as proof.
The policy decision, recorded here: JP sends the fingerprint that established
harnesses have verified Anthropic accepts, including the system-prompt identity
line if enforcement requires it, and prefers the smallest fingerprint that works
reliably, dropping impersonation elements Phase 1 shows to be unnecessary.
Identifying as JP is preferred wherever Anthropic accepts it; matching Claude
Code where it does not is an accepted trade-off, made explicitly in this RFD
rather than inside the `async-anthropic` fork.

Two groups carry their own rules.
If identity metadata proves required, JP sends pseudonymous identifiers of its
own in the required shape (a device identifier derived from a JP-local install
ID, never Claude Code's), and the resolved credential carries the account UUID
as a narrowly typed attribution field (see Credential resolution); Phase 1
records exactly which identity metadata is transmitted.
Profiles without a stored account UUID follow the unverified-profile rules in
Credential resolution.
The output clamp is behavior, not authentication: JP adopts it only if Phase 1
shows enforcement, and then as a documented cap on OAuth `max_tokens`, never as
a hidden side effect of bearer mode.

The `async-anthropic` fork the project already maintains grows a bearer-auth
mode implementing the fingerprint as named, documented constants.
The bearer-auth mode emits the OAuth headers itself; user-configured
`beta_headers` merge separately and can neither remove nor duplicate them, and
none of this is sent with API key auth.

### OAuth flow mechanics

The login flow is the one Claude Code uses, well-documented by multiple
open-source implementations:

1. Generate a PKCE verifier/challenge and random state.
2. Open `https://claude.ai/oauth/authorize` with Claude Code's client ID,
   requesting `user:inference` plus `user:profile` for account identification
   (`user:inference` is only granted via this endpoint; the platform console
   endpoint issues API-key-management tokens only).
3. Capture the callback on a localhost port, or accept a pasted redirect URL.
4. Exchange the code at `https://api.anthropic.com/v1/oauth/token`.
5. Store `{access, refresh, expires, account_id, email}`.

The requested scope set is deliberately smaller than Claude Code's own grant,
which also asks for `org:create_api_key`, `user:sessions:claude_code`,
`user:mcp_servers`, and `user:file_upload`: capabilities JP has no use for, each
widening what a leaked refresh token can do and what the browser consent screen
asks the user to approve.
JP borrows a client ID it does not own, so the authorize endpoint may refuse the
reduced set; whether the reduction holds is a Phase 3 measurement, not a
promise.
`user:profile` is not optional within that set: the bootstrap identity call
requires it (see Phase 1), so dropping it would leave every profile unverified.
If Anthropic requires the full Claude Code scope set for this client ID, JP
requests it, and this RFD gains a one-line justification per extra capability,
recorded as an accepted risk before Phase 3 ships.

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
  On macOS the file is a regression against Claude Code, which uses the
  Keychain; the Phase 4 keyring backend closes that gap, gating other processes'
  access behind Keychain ACLs.
- **Maintenance of an undocumented flow.** The OAuth endpoints, client ID, and
  headers are Claude Code implementation details, not published API.
  Anthropic can change them without notice, and JP owns the breakage.
  The request fingerprint deepens this: every impersonated element is one more
  surface that can silently change or become an enforcement check.
  If identity metadata proves required, JP sends a stable pseudonymous device
  identifier with every OAuth request; that is a privacy-relevant behavior this
  RFD makes explicit rather than inherits silently.

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

- **OAuth for other providers.** Only Anthropic is implemented.
  The store schema is versioned and namespaced so a later RFD can extend it;
  OpenAI Codex OAuth follows as its own RFD that `Extends` this one.
  Genericism is argued on paper (see Credential store), not claimed proven from
  a single implementation.

  The store API is provider-agnostic — categories, providers, and profiles are
  just keys — while credential policy is provider-owned by construction: each
  provider's chain semantics, refresh flow, and error vocabulary live in its own
  module and move with it into a plugin.
  The second provider adds its own policy without touching shared code; the
  store API is the only shared surface, and it is the same one plugins consume.

- **Multi-account routing UX.** Ordering in the `auth` chain is the only routing
  mechanism.
  Per-conversation account pinning, usage-based routing, and similar are out of
  scope.

- **Keyring backends beyond macOS.** Phase 4 adds the macOS keyring backend.
  Linux secret-service and Windows Credential Manager stores exist behind the
  same `keyring-core` interface and can follow, but are not part of this design.

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
- **System prompt enforcement.** Inference with OAuth tokens requires the system
  prompt to begin with Claude Code's identity line; Phase 1 measured the
  enforcement, so JP prepends it.
  The line cannot be removed, but its effect can be bounded: JP follows it with
  a block naming it as a transport artifact and pointing at the prompt that
  follows.
  Measured: the enforcement check tolerates that insertion, and the model then
  identifies as JP without remarking on the contradiction.
  The residual risk is that this is a prompt-level mitigation, not a guarantee
  — a future model may weigh the identity line differently — and that API key
  requests carry neither block, so the same conversation can behave differently
  across a credential switch.
- **Thinking signatures across accounts.** Signatures minted under one account
  may be rejected under another after a fallback switch.
  The existing stale-signature recovery (strip and retry) should absorb this;
  confirm during manual testing.
- **Quota error shape.** The quota headers are taken from Claude Code's client
  source, not from a live rejection JP observed.
  If the names or values have drifted, exhaustion degrades to an ordinary rate
  limit: JP retries in place, records no cooldown, and never switches
  credentials — the safe direction, since the opposite would spend money over a
  misread header.
  A rejection without reset timing falls back to the fixed 30-minute
  account-scoped cooldown.

## Implementation Plan

### Phase 1: bearer auth, store, and setup-token login

Bearer mode in the `async-anthropic` fork; the credential store with file
locking, versioned schema, and tagged credential variants; chain preflight and
resolution in the `jp_credentials` crate; `jp provider auth login llm.anthropic
--setup-token` reading a long-lived token from `claude setup-token` on stdin,
stored as a static `token` credential, with identity recovery attempted at login
and the profile stored unverified when it fails; `jp provider auth list` and `jp
provider auth logout`, so the store is never manageable only by hand-editing.
Phase 1 carries three measurement deliverables the rest of the design is fixed
against: the minimum request fingerprint Anthropic accepts, measured top-down
from the complete inventory in Request authentication (headers, identity
metadata, system content and attestation) and recording which identity metadata
is transmitted; whether the bootstrap endpoint accepts setup tokens for identity
recovery; and the observed wire shape of a subscription-limit failure (status,
body, headers).
Independently reviewable and useful on its own.

**Measured: the complete fingerprint is accepted.** A setup-token credential on
a single-entry `["subscription"]` chain (no fallback to mask a failure)
completes a turn against `claude-sonnet-5`, with the headers, the OAuth beta
set, and the Claude Code identity line as inventoried above.
The trim runs from there, one element per request, keeping whatever Anthropic
still accepts without it.

**Measured: the system-prompt identity line is enforced.** Dropping it — the
first trim, since it is the element with a behavioral cost rather than only a
maintenance one — makes every request fail, so JP keeps sending it, followed by
a block that names it as a transport artifact (see Request authentication).
The check tolerates that insertion, and the model identifies as JP rather than
as Claude Code.

The rejection does not identify itself: Anthropic answers `429 rate_limit_error`
with an opaque `"message": "Error"` body, not a `401` and not a message naming
the check.
The remaining trims are read against that signature, because a rejected
fingerprint is otherwise indistinguishable from ordinary throttling, and a
subscription window that expires mid-campaign would read as a trim result.
It also constrains Phase 2: status alone cannot separate a fingerprint
rejection, a transient rate limit, and subscription-window exhaustion, so the
classifier keys on the error body.

**Measured: the bootstrap endpoint rejects setup tokens.** It answers HTTP 403
`permission_error` with `scope requirement any_of(user:ccr_inference,
user:profile)`, and `claude setup-token` mints a token scoped to
`user:inference` alone.
Identity recovery is therefore unavailable for setup-token profiles: they are
stored unverified, which the design already treats as a usable state, and the
duplicate-account check never runs for them.
This also fixes a scope requirement for Phase 3: `user:profile` is what makes
attribution possible, so the reduced scope set must keep it.

The token reaches JP by paste, not by pipe: `claude setup-token` is an
interactive session, and every stage of a shell pipeline starts at once, so a
downstream JP would read the stream before a token exists while the command
still needs the terminal for its own prompts.
A non-interactive source (a clipboard tool, a file) pipes fine.

**Measured: subscription limits are reported in response headers.** Read from
Claude Code's own client source rather than captured from a live rejection, so
the header names are authoritative but the values should be confirmed against
the first real limit JP sees.

A `429` is a quota rejection only when it carries
`anthropic-ratelimit-unified-representative-claim` or
`anthropic-ratelimit-unified-overage-status`; Claude Code treats a `429` without
them as "not a quota limit" and surfaces whatever the API said.
That predicate is what separates exhaustion from capacity throttling and from
the fingerprint rejection measured above, all three of which are `429`
`rate_limit_error`.
The `representative-claim` values are the cooldown scopes the store records, and
`anthropic-ratelimit-unified-reset` carries the reset instant as Unix seconds,
so reset timing needs no separate usage endpoint.
`seven_day` is the longest window, which is where the seven-day cooldown cap
comes from.

The same source confirms that `overage-status` governs paid spillover on
Anthropic's side: a subscription account with extra usage enabled keeps serving
requests past its window and bills them, which is why JP never claims a
`profile` entry is free (see Drawbacks).

### Phase 2: quota fallback

The subscription-window exhaustion classification, keyed on the quota headers
Phase 1 identified; retryable-via-credential-switch handling; scoped cooldown
persistence; the fallback notice; and no-backoff retry.
Depends on Phase 1.

### Phase 2c: provider-owned credential policy and the core store API

Moves credential policy into the Anthropic provider and reshapes the store into
the core-owned API described in Credential store.
Chain preflight and resolution move from `jp_credentials` into the provider;
in-flight switching moves from the CLI's stream-retry path into the provider's
request admission; the switch notice becomes a provider-emitted notice event;
provider construction returns to config-only, uniform across providers; and the
shell's per-call-site credential resolution is deleted.
The store adopts the `keyring-core` store interface behind the existing
lock-mutate-persist cycle.
User-visible behavior is unchanged, except that fallback now applies to every
request the provider sends rather than only the query streaming loop.
It is numbered `2c` although it runs before Phase 2b: 2b was already named when
this phase was added, and existing phase references keep their meaning.
Depends on Phases 1 and 2 (it relocates what they built); Phases 2b, 3, and 4
build on it.

### Phase 2b: subscription-state awareness

Phase 2 reacts to a spent allowance after a request fails.
The same headers Anthropic sends on *every* response carry enough to act before
that, and Claude Code's client shows which of them are load-bearing:

- **Warning states.** `anthropic-ratelimit-unified-status` reports
  `allowed_warning` alongside the window that is filling up, and separate
  `-5h-utilization` / `-7d-utilization` headers carry the fraction consumed with
  a `-surpassed-threshold` marker.
  Surfacing that as chrome tells the user their allowance is nearly gone while
  they can still choose what to spend it on — far better than discovering it
  mid-turn.
- **Explaining a refusal.** `-overage-disabled-reason` names why paid spillover
  could not serve the request (`out_of_credits`, org- or seat-level caps).
  That belongs in the terminal error rather than being flattened into "limit
  reached", so a user whose organization capped spending is told so instead of
  guessing.
  This reports on spillover; it does not select it (see Quota fallback).
- **Recording state from successful responses.** Cooldowns are written today
  only when a request fails.
  Reading the same headers off a `200` lets JP record a window's utilization and
  reset without burning a rejected request to discover it.

This phase is optional in the sense that Phase 2 is correct without it, and
valuable because it converts the subscription from something JP reacts to into
something it can report on.
It is numbered `2b` rather than `3` so the phase numbers already referenced
elsewhere in this document keep their meaning.
Depends on Phases 2 and 2c.

**Measured: the header names, from Claude Code's own client.** The per-window
headers are abbreviated where the window names elsewhere are spelled out:
`anthropic-ratelimit-unified-{5h,7d}-utilization` carries the fraction consumed,
`-{5h,7d}-reset` the per-window reset (distinct from the top-level `-reset`),
and `-{5h,7d,overage}-surpassed-threshold` marks a crossed warning threshold —
its presence is the signal, its value the threshold.
`-overage-disabled-reason` reports one of thirteen enumerated reasons
(`out_of_credits`, `org_level_disabled`, `seat_tier_zero_credit_limit`, …).
JP normalizes the abbreviations to the spelled-out window names so one
vocabulary reaches the user.

The client also computes a *time-relative* early warning of its own when the
server sends no threshold (warn at 90% utilization within 72% of the 5-hour
window, and three tiers for the weekly one).
JP does not: that is client-side policy layered on the provider's own signal,
and surfacing only what the provider flags keeps the notice trustworthy.

**A spent window is readable from a response that succeeded.** `status:
rejected` accompanies a `200` whenever paid extra usage covered the request, so
the drawback this design records as unpreventable — usage billed past the
window with no rejection to react to — is detectable one request after it
starts.
JP records the scoped cooldown at that point, so the next resolution moves off
the profile instead of billing against it again, and surfaces a notice naming
the window.
The request that revealed it is still billed; that part is unavoidable.

### Transport: reading a successful response's headers

The quota headers ride on every response, but JP's Anthropic client reached them
only on failures: `reqwest-eventsource` yields a unit `Open` event and exposes
the `Response` only in its error variants.
Phase 2b therefore rests on a transport change, made as its own step: the
streaming path issues the request directly and parses the body with
`eventsource-stream` (the SSE parser `reqwest-eventsource` itself wraps), which
makes the response — and its headers — available on success.

The change drops SSE-level reconnection, deliberately.
Anthropic's message endpoint sends no SSE event ids, so a reconnect cannot
resume: it issues a fresh completion whose content restarts from the beginning,
which is the wrong operation at that layer.
Nothing consumed it in practice — every error was yielded to the caller, which
abandoned the stream — while the orphaned reconnect still fired 15 seconds
later and paid for a completion nobody read.
Recovery stays where it belongs: the turn loop flushes partial content, rebuilds
the thread with it as assistant prefill, and continues on a fresh stream.
The `cerebras` and `llamacpp` providers had already reached the same conclusion
and configure `retry::Never`.

One related fix: the buffered `get`/`post` paths retried *every* `429` with a
15-second floor, including quota rejections, so a spent subscription stalled for
the better part of a minute before the credential switch could act.
They now retry only capacity throttling.

### Phase 3: PKCE browser login

The full browser login flow: PKCE, browser, localhost callback, paste fallback,
refresh-on-expiry.
The phase lands in two halves.
The token exchange and refresh logic is pure library code in the provider's auth
module and lands first: refresh-on-expiry is what per-request resolution needs
for `oauth` profiles, however the tokens were obtained.
The login command ships in the provider's command plugin (`jp anthropic login`),
which waits on two command-plugin protocol capabilities specified separately:
running without a workspace, and prompting for input.
Until the plugin ships, setup-token login through the in-core CLI remains the
login path.
Phase 3 measures whether the authorize endpoint grants the reduced
`user:inference` plus `user:profile` scope set for Claude Code's client ID; if
not, the full-set fallback and its per-capability justifications land in this
RFD before the phase ships (see OAuth flow mechanics).
Depends on Phases 1 and 2c; independent of Phase 2.

**Measured: the endpoints moved.** The token endpoint is
`https://platform.claude.com/v1/oauth/token`, not the `api.anthropic.com` path
named above, and the authorize endpoint Anthropic's client opens is
`https://claude.com/cai/oauth/authorize`, which redirects to the `claude.ai`
path named above.
JP sends what the client sends.
Both token requests carry a JSON body, not a form encoding, and the paste
fallback redirects to `https://platform.claude.com/oauth/code/callback`.

**Measured: an access token is refreshed five minutes before it expires.**
Anthropic's client treats a token inside that window as already expired, which
keeps one from lapsing between being resolved and being used; JP uses the same
buffer.

**Still unmeasured: whether the reduced scope set is granted.** That needs a
live authorize round-trip, which arrives with the browser flow.
Until then the reduced set is what JP requests and the full-set fallback remains
the documented contingency.

### Phase 4: macOS keyring store backend

The macOS Keychain behind the `keyring-core` store interface
(`apple-native-keyring-store`), replacing the file store on macOS and restoring
parity for users migrating from Claude Code there.
The backend stores the same versioned JSON document as a single entry, so
cooldowns and re-login state move with the secrets; the store's `ResourceLocker`
lock file keeps serializing mutations, since the Keychain provides no locking.

Existing stores migrate once, under the file lock: read the file store, write
the Keychain item, read it back and verify, switch the backend marker, then
remove the file best-effort and warn when removal fails.
The single item write makes partial migration impossible; on any failure JP
stays on the file backend and says so.
The file remains the backend on Linux and Windows.
Depends on Phases 1 and 2c; independent of Phases 2 and 3.

## References

- [Claude Code client source (de-obfuscated)][claude-code-source] — quota
  headers, error classification, OAuth handling
- [Anthropic OAuth flow reference implementation (oh-my-pi)][oh-my-pi]
- [oh-my-pi request fingerprint (`buildAnthropicHeaders`)][oh-my-pi-headers]
- [OpenClaw OAuth concepts][openclaw] — token sink, refresh rotation, profile
  routing
- [CLIProxyAPI][cliproxy] — proxy-based alternative
- [Using Claude Code with your Pro or Max plan][claude-plans]

[RFD 048]: 048-four-channel-output-model.md
[\#875]: https://github.com/dcdpr/jp/issues/875
[claude-code-source]: https://github.com/alex000kim/claude-code
[claude-plans]: https://support.claude.com/en/articles/11145838-using-claude-code-with-your-pro-or-max-plan
[cliproxy]: https://github.com/router-for-me/CLIProxyAPI
[oh-my-pi]: https://github.com/can1357/oh-my-pi/blob/75bdb20212871221406e119745136edcb2197653/packages/ai/src/registry/oauth/anthropic.ts
[oh-my-pi-headers]: https://github.com/can1357/oh-my-pi/blob/75bdb20212871221406e119745136edcb2197653/packages/ai/src/providers/anthropic.ts
[openclaw]: https://docs.openclaw.ai/concepts/oauth
