# RFD 090: Anthropic Subscription Auth with Credential Fallback

- **Status**: Accepted
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-07-03
- **Tracking Issue**: [\#875]
- **Extended by**: [RFD 110]
- **Summary**: Adds Anthropic OAuth subscription login, a multi-profile
  credential store, and automatic cooldown fallback across subscription and
  API-key credentials.

## Summary

This RFD adds OAuth authentication for Anthropic subscription plans (Claude Pro
and Max) alongside API key authentication.
Credentials are stored as named profiles in a user-global credential store, and
each provider resolves an ordered credential chain that automatically falls back
to the next subscription profile or API key when a quota cooldown occurs.

## Motivation

JP only supports Anthropic's platform API, authenticated with an API key and
billed per token.
Anthropic also sells fixed-price subscriptions with a token allowance.
Users who hold both should be able to spend the subscription allowance first and
overflow to per-token billing without switching credentials manually.

Supporting this requires more than a second header because OAuth credentials are
*stateful*.
They live outside config, expire, refresh over the network, and can be
temporarily exhausted.
A static environment variable name in config, read synchronously at provider
construction, cannot represent any of that.
Without subscription support, users who hold both plans pay twice: once for the
subscription allowance they cannot use through JP, and again per token.

## Design

### CLI and configuration

Log in once per subscription account:

```sh
jp provider auth login llm.anthropic                   # stored as "default"
jp provider auth login llm.anthropic --name work       # stored as "work"
```

The command opens the browser for Anthropic's OAuth consent, captures the
callback on localhost (with a redirect-URL paste fallback for headless
machines), and stores the resulting tokens.
If the token response omits the account UUID and email, JP recovers them from
the Claude CLI bootstrap endpoint using the fresh access token.
The command confirms the linked account and refuses to store the same account
under two profiles, keyed on the account UUID rather than email alone.
A profile without a resolved account UUID (even when an email was recovered) is
stored as unverified with a warning, and duplicate detection is skipped for it
until the missing identity is recovered during Credential resolution.

Inspect and remove stored credentials without editing the store by hand:

```sh
jp provider auth list                                  # profiles, accounts, expiry, cooldowns
jp provider auth logout llm.anthropic --name work      # remove one
```

`jp provider auth list` shows each profile's credential type and state: valid,
expired, needing re-login, cooling down (with the scope, account or model
family, and cooldown expiry), or unverified.
An unverified profile is also marked usable or needing re-login, depending on
whether Request authentication enforces account attribution and whether identity
recovery has failed.
There is no manual cooldown-reset command: a cooldown clears when it expires or
when the user logs out and logs back in.

Credentials are user-global, so `jp provider auth` bypasses workspace discovery,
using the same startup exception as `jp init`.
`list` and `logout` require no TTY and are safe to script.
Browser login requires a reachable localhost callback or an interactive paste;
when neither is available, it fails with a diagnostic naming both options.
Setup tokens are read from stdin rather than process arguments, which would leak
into shell history and `ps` output.
Normal LLM commands never start a login implicitly; a missing or expired profile
fails with an error naming the `jp provider auth login` command that fixes it.

Configure the credential chain in fallback order:

```toml
[providers.llm.anthropic]
auth = ["subscription:personal", "subscription:work", "api_key"]
```

Each entry names the billing kind and, optionally, which credential of that kind
to use.
`api` and `sub` are accepted as shorthand for `api_key` and `subscription`, and
both normalize to the full name.

- `api_key` resolves via `api_key_env`.
  When `api_key_env` maps multiple named keys, `api_key:<name>` selects one.
- `subscription` (bare) resolves the sole stored subscription profile;
  `subscription:<name>` selects a named profile.
  Profile names are case-sensitive.
- A chain entry selects a stored profile without asserting the credential's
  mechanism, so the same entry keeps working if a profile migrates from a static
  setup token to browser OAuth.
- A bare kind with zero or multiple credentials to choose from, or
  `<kind>:<name>` naming a missing profile, fails at preflight with an error
  naming the fix.
  An empty `auth` list, duplicate entries, and unrecognized items fail config
  validation.
- The default is `["api_key"]`, preserving existing behavior.
- Across config layers, `auth` replaces the previous list as a whole and never
  appends.
  Merging two chains element-wise would produce an order no layer specified;
  replacement ensures that the effective chain was written in full by one layer.
  `JP_CFG_PROVIDERS_LLM_ANTHROPIC_AUTH` overrides it like any other key.

The `auth` chain follows the standard config trust model: a workspace config can
override the chain, just as it can override `api_key_env` or model selection.
Users who want their chain to be authoritative can pin it in a higher-precedence
layer, such as the user-workspace config or
`JP_CFG_PROVIDERS_LLM_ANTHROPIC_AUTH`.
Listing `api_key` in the effective chain authorizes continuing on per-token API
billing when earlier entries are exhausted, with no additional prompt or
setting.

When the active subscription profile hits a usage limit, JP prints a one-line
notice (`subscription limit reached (personal), continuing with work`) and
retries the request with the next credential in the chain.
Fallback is automatic but never silent, because switching credentials can change
what the request costs.
The notice records the switch without certifying how either credential is
billed: a subscription account with paid usage credits enabled can still incur
per-token charges under a `profile` entry (which Anthropic tracks as an overage
status alongside the usage window), so JP never promises that OAuth traffic is
covered by the subscription allowance.
The notice is rendered as chrome on stderr per [RFD 048], or as NDJSON on stderr
under `--format json`.

The auth commands are provider-owned UX.
They live in `jp_cli` until the command-plugin protocol supports running without
a workspace and prompting for input, at which point they move into the
provider's command plugin as `jp anthropic login`, `list`, and `logout`.

### Credential store

The user-global store holds one versioned JSON document, keyed by category,
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

Each credential is a tagged variant.
`oauth` carries a refreshable token pair and expiry; `token` is a static bearer
token (the Phase 1 `claude setup-token` output) with no refresh flow and no
expiry JP can inspect.
`jp provider auth list` reports the variant so a static token is never
misdescribed as expired or refreshable.

`version` is the schema migration discriminator: JP rejects a store written by a
newer schema with a clear error rather than reading it partially, and migrations
rewrite the file atomically under the same lock as any other mutation.
Credentials nest under a category (`llm`) and a provider (`anthropic`) rather
than a flat dotted key, so listing one category is a direct lookup instead of
key-prefix parsing, and future non-LLM categories fit without a schema break.
Only the `anthropic` provider under `llm` is implemented here.
As a check against a second provider, OpenAI Codex OAuth issues the same fields
(access token, refresh token, expiry, and account identity), so the `oauth`
variant and `llm` category fit it unchanged; any mismatch a future
implementation surfaces is handled by bumping `version` in the extending RFD.

All mutations (token refresh and cooldown marking) run under a file lock because
refresh tokens rotate, and two processes racing a refresh can invalidate each
other's tokens.
After acquiring the lock, a mutation reloads the store and rechecks its
precondition so a process holding a pre-rotation snapshot cannot submit an
already-rotated refresh token and falsely mark a healthy profile as needing
re-login.
Writes are atomic, replacing the entire document at once so a crash mid-refresh
cannot lose a rotated refresh token.
The lock-mutate-persist cycle lives in the `jp_credentials` crate:
`CredentialStore` owns the document encoding, schema-version check, and mutation
cycle, while storage backends implement `CredentialBackend` to read and write
the document as a single opaque unit.
A file-based backend is the baseline, replacing its file atomically via a
temporary file and rename and creating it with `0600` permissions; the macOS
Keychain (Phase 4) swaps in behind the same trait by adapting a `keyring-core`
store.
The mutation lock is a `ResourceLocker` (`jp_storage`), the same file-based
locking primitive used for conversation locks, and remains file-based for every
backend because keyring stores provide no locking.
When the token endpoint rejects a refresh, the store marks the profile as
needing re-login; resolution skips it with a notice and continues down the
chain.

`cooldowns` persists quota cooldowns across invocations, keyed by the window
Anthropic reports as exhausted: `five_hour`, `seven_day`, and `overage` (the
paid extra-usage allowance) cover the whole account, while `seven_day_opus` and
`seven_day_sonnet` cover a single model family.
A single profile-wide timestamp would let an exhausted Opus window block Haiku
title generation on the same account.
Resolution skips a profile only when a cooldown scope matching the requested
model is still in the future.
Persisting reset timing lets a fresh `jp` invocation skip the exhausted scope
immediately instead of burning a failed request to rediscover it.

Core owns the store and exposes it only through its API.
In-core providers call `jp_credentials` directly, while command plugins (and
future provider plugins) reach the same operations through host protocol
messages, so the on-disk format is never a cross-binary contract.
The API is the versioning boundary between JP and its plugins, while `version`
guards the document itself.

### Credential resolution

Resolution is a provider concern.
The Anthropic provider owns its credential policy: it reads the `auth` chain
from its config, walks it per request, refreshes expired access tokens, and
decides when an entry is skipped, retried, or abandoned.
Core owns none of that vocabulary: every provider is constructed from its config
alone, and the turn loop, tasks, and other call sites remain credential-unaware.
Core owns only the credential store (see Credential store): the provider loads
profiles and records cooldowns, re-login markers, and recovered identities
through the `jp_credentials` API.

Resolution runs in two stages inside the provider:

1. **Preflight** runs synchronously and locally at provider construction against
   a store snapshot.
   Hard failures are config errors (an unparseable chain, an entry that maps to
   no stored profile, or a malformed stored record) or a chain where no entry
   can possibly resolve; one broken profile does not block a chain that has
   another usable entry.
   This uses the existing `provider::preflight` seam: a provider that cannot
   authenticate fails before commands start side-effectful work, just as when
   `api_key_env` is unset.
2. **Resolution** runs asynchronously before each request the provider sends.
   It walks the chain, skips entries per the table below, refreshes an expired
   access token, and sends the request with the selected API key or bearer
   token.
   The construction snapshot serves preflight only; a refresh re-reads the store
   under the lock before acting.

Every credential condition has a single outcome:

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

Two rows warrant specific note.
First, an unset `api_key` environment variable is skipped in a multi-entry
chain, so a missing key cannot block subscription use; under the default
`["api_key"]` chain, that skip exhausts the chain and fails preflight as before.
Second, cooldown recording applies only to stored profiles: `api_key` has no
store entry, so an exhausted key is retried once per invocation, rejected at
request admission before any tokens are billed, and skipped for the next entry.

Unverified profiles resolve according to whether Request authentication requires
account attribution (measured in Phase 1).
If `account_uuid` is optional, an unverified profile resolves normally with
`account_uuid` omitted (`device_id` and `session_id` are JP-derived and need no
account identity, and the reference implementation's metadata shape tolerates
its absence).
If attribution is required, resolution attempts one identity recovery via the
Claude CLI bootstrap endpoint, using the same call as login.
Because that endpoint rejects setup tokens (measured in Phase 1), a setup-token
profile can never be attributed and stays unverified for its lifetime.
When recovery succeeds, the UUID is persisted under the store lock and the
duplicate-account check that login skipped runs at that point.
When recovery fails, the profile cannot resolve: it is skipped with a notice
naming the `jp provider auth login` command while another entry remains, or
becomes a terminal error when the chain is exhausted.

Because the provider resolves per request, every request a turn issues (across
tool-execution cycles, title generation, summarization, and other tasks) lands
on a usable credential, and an access token expiring between requests is
refreshed rather than surfacing as an error.
An expiry surfacing mid-stream triggers a refresh and retry on the same
credential rather than advancing the chain.
A task whose provider fails preflight skips its work softly, matching title
generation.
Profile names, chain position, and account identity never leave the provider:
the store API speaks in categories, providers, and profiles, and no other core
component consumes them.
When request enforcement requires account attribution, the provider embeds the
resolved profile's account UUID in `metadata.user_id`.
User-visible resolution outcomes (a skipped profile or a credential switch) are
emitted as notice events on the provider's event stream and rendered as chrome
on stderr per [RFD 048].

Credential policy travels with the provider: when a provider moves out of core
into a plugin, its resolution logic, wire mechanics, and login UX move with it,
leaving the credentials API (in-process in core, over the plugin protocol in a
plugin) as the only seam.

### Quota fallback

Three failure semantics matter here; the existing classifier represents two:

- **Transient rate limit.** A plain HTTP 429 (`RateLimit`), retried in place
  with backoff.
  Unchanged by this design.
- **API billing exhaustion.** Credit-balance and billing errors on per-token
  accounts (`InsufficientQuota`).
- **Subscription-window exhaustion.** A subscription account hitting a rolling
  usage window.
  `looks_like_quota_error` matches billing phrases, and the Anthropic classifier
  maps ordinary 429s to `RateLimit`, so subscription-window exhaustion requires
  its own classification.

Subscription limits are reported in response headers rather than the error body
(measured in Phase 1).
A `429` carrying `anthropic-ratelimit-unified-representative-claim` or
`anthropic-ratelimit-unified-overage-status` is a quota rejection; a `429`
without them is not, matching Claude Code's client classifier.
This header check separates quota exhaustion from ordinary capacity throttling
and from a rejected request fingerprint, both of which also return `429` with an
opaque body.

The headers also carry the parameters needed to record a cooldown:

| Header                                             | Use                                                                                             |
| -------------------------------------------------- | ----------------------------------------------------------------------------------------------- |
| `anthropic-ratelimit-unified-status`               | `allowed`, `allowed_warning`, `rejected`                                                        |
| `anthropic-ratelimit-unified-representative-claim` | the exhausted window: `five_hour`, `seven_day`, `seven_day_opus`, `seven_day_sonnet`, `overage` |
| `anthropic-ratelimit-unified-reset`                | when that window resets, as Unix seconds                                                        |
| `anthropic-ratelimit-unified-overage-status`       | whether paid spillover can serve the request                                                    |
| `retry-after`                                      | present only when rejected with no overage available                                            |

JP never selects paid spillover.
A quota rejection is treated the same whether `overage-status` reports spillover
as available or exhausted: the cooldown is recorded and the chain advances.
Paid usage happens only when the user lists `api_key` in the chain, keeping a
single consent model.
However, because extra usage is an account-level setting on Anthropic's side, an
account with it enabled may have requests served past the window and billed as
overage without returning a rejection.
JP cannot prevent that server-side spillover; it only refuses to opt into it.

A `401` or `403` is a credential-scoped failure: the provider refused the
credential itself, so retrying with it cannot succeed.
Anthropic returns two specific messages that mark the profile as needing
re-login: `OAuth token has been revoked` and `OAuth authentication is currently
not allowed for this organization`.
Any other `401` or `403` is treated the same way, since a rejected credential
cannot serve the request regardless of the reason.

Exhaustion becomes conditionally retryable via a credential switch:

1. A request fails at admission with subscription-window exhaustion (under a
   stored profile) or billing exhaustion (under any chain entry).
   The provider classifies the rejection from the headers above, reading the
   exhausted scope and its reset timestamp (distinct from `retry-after`, whose
   backoff semantics do not fit a subscription cooldown).
   When a rejection arrives without reset timing, a fixed 30-minute
   account-scoped cooldown applies.
   Because the failure costs are asymmetric, the fallback duration biases short:
   a cooldown that expires early wastes one admission-rejected request (billing
   no tokens), whereas a cooldown that lasts too long spends per-token API
   credits while a recovered subscription allowance sits idle.
   When available, the server's reset timestamp takes precedence, capped at
   seven days (the longest known window) so a malformed timestamp cannot lock
   out a profile indefinitely.
2. The provider records the scoped cooldown through the store API (for stored
   profiles; `api_key` has no store entry) and emits the switch notice event.
   The recorded cooldown routes the next resolution past the exhausted profile,
   so a mid-turn switch and a fresh invocation share one code path.
3. The provider re-resolves the chain and re-sends the request immediately
   without backoff, since the error is not transient and the next credential is
   usable immediately.
   Model metadata (context sizes, structural details) is retained across the
   switch; if the new credential lacks access to the model or the model is
   unknown, the request fails terminally with an error naming the credential and
   the model.
4. If the chain is exhausted, the original error is returned as a terminal
   failure.

Every credential in the chain is expected to have access to the models the
conversation uses.
JP neither switches models on a credential's behalf nor probes the chain for a
credential that can serve the model: "this account lacks the model" and "this
model does not exist" both return HTTP 404, and walking the chain on a 404 would
try every credential whenever a model name is mistyped.

Fallback applies to every request the provider sends: the query streaming loop,
conversation edit and summarize, inquiry collection, and title generation.
The switch is invisible to the turn loop, which receives a provider and a stream
with the notice event as the only trace in the event flow.
Persisted cooldowns route subsequent invocations past exhausted profiles, and an
admission-time rejection advances the chain wherever it occurs, with no separate
in-flight machinery.

Anthropic enforces quota at request admission and lets an in-flight stream
finish (going into debt rather than cutting off mid-stream), so fallback
normally happens on a clean request boundary.
If a stream dies mid-flight anyway, the existing retry flow handles it: partial
content is flushed to the `ConversationStream`, the turn loop rebuilds the
thread with that content, and the fresh stream continues on the fallback
credential.

### Request authentication

OAuth requests differ from API key requests by more than the authentication
header.
Reference implementations send a Claude Code request fingerprint; at the pinned
oh-my-pi revision, the provider module sends the following for OAuth tokens:

- **Headers**, built by [`buildAnthropicHeaders`][oh-my-pi-headers]:
  `Authorization: Bearer <token>` instead of `x-api-key`; an `anthropic-beta`
  set including `oauth-2025-04-20` and Claude Code betas
  (`claude-code-20250219`, `interleaved-thinking-2025-05-14`,
  `context-management-2025-06-27`, and others); client identity headers (a
  Claude Code user agent and `x-app: cli`); client platform and version headers
  (`X-Stainless-*`, `anthropic-client-platform`, `anthropic-client-version`);
  and per-request identifiers (`x-client-request-id` and an optional Claude Code
  session-id header).
- **Request metadata**: `metadata.user_id` carrying `{device_id, session_id,
  account_uuid}` (a stable device identifier, a per-session UUID, and the active
  credential's account UUID).
- **System content**: a system prompt beginning with Claude Code's identity
  line, plus a billing block whose `cch` attestation is a hash computed over the
  serialized request body.
  Because Anthropic enforces the identity line (measured in Phase 1), JP sends
  it and immediately follows it with a block framing it as a transport artifact
  and pointing at JP's own prompt, keeping a foreign identity from acting as an
  instruction; the attestation is not sent.
- **Behavioral constraints**: an output-token clamp (64k) applied to OAuth
  requests even when the model's ceiling is higher.

Phase 1 measures how much of this fingerprint Anthropic enforces by testing
top-down: starting from the complete fingerprint above and removing elements
until only the minimum accepted request remains, rather than starting minimal
and treating the absence of immediate errors as proof.
JP sends the smallest fingerprint that Anthropic reliably accepts, identifying
as JP wherever possible and matching Claude Code only where enforcement requires
it.
Recording this trade-off in the RFD keeps the policy decision explicit rather
than burying it in the `async-anthropic` fork.

Identity metadata and the output clamp follow their own rules:

- If identity metadata is required, JP sends pseudonymous identifiers of its own
  in the required shape (a device identifier derived from a JP-local install ID,
  never Claude Code's), and the resolved credential carries the account UUID as
  a narrowly typed attribution field (see Credential resolution).
  Phase 1 records which identity metadata is transmitted, and profiles without a
  stored account UUID follow the unverified-profile rules in Credential
  resolution.
- The output clamp is behavioral rather than authentication: JP adopts it only
  if Phase 1 shows enforcement, and exposes it as a documented cap on OAuth
  `max_tokens` rather than a hidden side effect of bearer mode.

The in-tree `async-anthropic` fork adds a bearer-auth mode that implements the
fingerprint as named, documented constants.
Bearer mode emits the OAuth headers directly; user-configured `beta_headers`
merge separately without removing or duplicating them, and none of the
fingerprint is sent with API key auth.

### OAuth flow mechanics

The login flow matches Claude Code's OAuth flow, documented by multiple
open-source implementations:

1. Generate a PKCE verifier/challenge and random state.
2. Open `https://claude.ai/oauth/authorize` with Claude Code's client ID,
   requesting `user:inference` and `user:profile` for account identification
   (`user:inference` is granted only via this endpoint; the platform console
   endpoint issues API-key-management tokens only).
3. Capture the callback on a localhost port, or accept a pasted redirect URL.
4. Exchange the code at `https://api.anthropic.com/v1/oauth/token`.
5. Store `{access, refresh, expires, account_id, email}`.

The requested scope set (`user:inference` and `user:profile`) is deliberately
smaller than Claude Code's full grant, which also requests `org:create_api_key`,
`user:sessions:claude_code`, `user:mcp_servers`, and `user:file_upload`.
JP has no use for those extra capabilities, and excluding them narrows both what
a leaked refresh token can do and what the browser consent screen asks the user
to approve.
Within that set, `user:profile` is required because the bootstrap identity call
depends on it (see Phase 1), and dropping it would leave every profile
unverified.
Because JP borrows a client ID it does not own, the authorize endpoint may
refuse the reduced set; whether the reduction holds is measured in Phase 3.
If Anthropic requires the full Claude Code scope set for this client ID, JP will
request it and record a one-line justification per extra capability in this RFD
before Phase 3 ships.

Refresh uses the same token endpoint with `grant_type: refresh_token`.
The exchange and refresh logic is pure (types in, types out); the callback
server, browser launch, and store I/O form the thin imperative shell around it.

## Drawbacks

- **Prompt cache misses on fallback.** Prompt caching is scoped to the account.
  The first request after a credential switch misses every cache breakpoint and
  pays the full input-token price on the entire conversation history.
  Subsequent requests on the new credential re-populate the cache.
- **JP owns secrets on disk.** This design introduces a token file that JP must
  protect and users must know about.
  Until tool sandboxing exists, any same-user process (including JP's own
  approved tools) can read it.
  `0600` permissions limit exposure on Unix, and Windows relies on default
  user-profile ACLs, matching Claude Code.
  On macOS, a token file is a regression against Claude Code, which uses the
  Keychain; the Phase 4 keyring backend closes that gap by gating access behind
  Keychain ACLs.
- **Maintenance of an undocumented flow.** The OAuth endpoints, client ID, and
  headers are Claude Code implementation details rather than a published API.
  Anthropic can change them without notice, and JP owns the breakage.
  The request fingerprint compounds this risk: every impersonated element is
  another surface that can silently change or become an enforcement check.
  If identity metadata is required, JP sends a stable pseudonymous device
  identifier with every OAuth request; this RFD documents that privacy-relevant
  behavior explicitly.

## Alternatives

- **Proxy (such as CLIProxyAPI) plus `base_url` override.** Works with zero JP
  code, but requires a separate daemon, keeps tokens outside JP, and cannot do
  per-request fallback across JP's credential chain.
  Remains available as a workaround.
- **Reuse Claude Code's credential store.** Avoids building a login flow, but
  the format is undocumented and platform-specific (file on Linux, Keychain on
  macOS), and refresh-token rotation means JP and Claude Code would invalidate
  each other's sessions.
  It could become an optional import source later, but is unfit as the primary
  store.
- **Adopt an existing OAuth crate.** Candidate crates have single-digit to low
  double-digit download counts.
  Credential handling is the wrong place for an unproven dependency when the
  PKCE flow is roughly 300 lines of code.

## Non-Goals

- **OAuth for other providers.** Only Anthropic is implemented here.
  The store schema is versioned and namespaced so a later RFD can extend it;
  OpenAI Codex OAuth follows in a separate RFD that `Extends` this one.

  The store API is provider-agnostic (categories, providers, and profiles are
  opaque keys), while credential policy is provider-owned: each provider's chain
  semantics, refresh flow, and error vocabulary live in its own module and move
  with it into a plugin.
  A second provider adds its own policy without touching shared code, using the
  same store API that plugins consume.

- **Multi-account routing UX.** Ordering in the `auth` chain is the only routing
  mechanism.
  Per-conversation account pinning, usage-based routing, and similar features
  are out of scope.

- **Keyring backends beyond macOS.** Phase 4 adds the macOS Keychain backend.
  Linux Secret Service and Windows Credential Manager stores exist in
  `keyring-core` and can be adapted to `CredentialBackend` the same way, but are
  not part of this design.

- **Sharing tokens with Claude Code.** JP's login is independent.
  Users who log the same account into both tools may see one tool's session
  invalidated by the other's refresh, which is inherent to Anthropic's token
  rotation.

## Risks and Open Questions

- **Policy risk.** Using subscription tokens outside Claude Code relies on
  Anthropic's tolerance (reported second-hand as allowed, not published policy).
  Anthropic has previously cut off third-party tools using this flow.
  The feature can break without notice; API key auth remains the supported path.
- **System prompt enforcement.** Inference with OAuth tokens requires the system
  prompt to begin with Claude Code's identity line (measured in Phase 1), so JP
  prepends it and immediately follows it with a block framing it as a transport
  artifact and pointing at JP's prompt.
  In Phase 1 testing, the enforcement check tolerated that insertion and the
  model identified as JP without remarking on the contradiction.
  Two residual risks remain: this is a prompt-level mitigation rather than a
  guarantee (a future model may weigh the identity line differently), and API
  key requests carry neither block, so the same conversation can behave
  differently across a credential switch.
- **Thinking signatures across accounts.** Signatures minted under one account
  may be rejected under another after a fallback switch.
  The existing stale-signature recovery (strip and retry) should absorb this;
  confirm during manual testing.
- **Quota error shape.** The quota headers are taken from Claude Code's client
  source rather than a live rejection observed by JP.
  If the header names or values drift, exhaustion degrades to an ordinary rate
  limit: JP retries in place, records no cooldown, and never switches
  credentials.
  Failing in that direction avoids spending per-token money due to a misread
  header.
  A rejection without reset timing falls back to the fixed 30-minute
  account-scoped cooldown.

## Implementation Plan

### Phase 1: bearer auth, store, and setup-token login

Implements bearer mode in the `async-anthropic` fork; the credential store with
file locking, versioned schema, and tagged credential variants; chain preflight
and resolution in the `jp_credentials` crate; `jp provider auth login
llm.anthropic --setup-token` reading a long-lived token from `claude
setup-token` on stdin and storing it as a static `token` credential (attempting
identity recovery at login and storing the profile as unverified when recovery
fails); and `jp provider auth list` and `jp provider auth logout` so the store
never requires manual editing.

Phase 1 also measures three behaviors that the rest of the design depends on:

- The minimum request fingerprint Anthropic accepts, measured top-down from the
  full inventory in Request authentication (headers, identity metadata, system
  content, and attestation), recording which identity metadata is transmitted.
- Whether the bootstrap endpoint accepts setup tokens for identity recovery.
- The wire shape of a subscription-limit failure (status, body, and headers).

**Measured: the complete fingerprint is accepted.** A setup-token credential on
a single-entry `["subscription"]` chain (with no fallback to mask failures)
completes a turn against `claude-sonnet-5` using the headers, OAuth beta set,
and Claude Code identity line inventoried above.
Trimming starts from that baseline, removing one element per request and keeping
only what Anthropic requires.

**Measured: the system-prompt identity line is enforced.** Dropping the identity
line (tested first because it carries a behavioral cost in addition to a
maintenance cost) causes every request to fail.
JP therefore keeps the line and follows it with a block framing it as a
transport artifact (see Request authentication).
The enforcement check accepts that insertion, and the model identifies as JP
rather than Claude Code.

The fingerprint rejection does not identify itself: Anthropic returns `429
rate_limit_error` with an opaque `"message": "Error"` body rather than a `401`
or a descriptive message.
Subsequent fingerprint trims are evaluated against that signature, since a
rejected fingerprint otherwise looks like ordinary throttling and a subscription
window expiring mid-test would look like a trim failure.
This also constrains Phase 2: HTTP status alone cannot distinguish a fingerprint
rejection, a transient rate limit, and subscription-window exhaustion, and the
fingerprint rejection body is opaque, so the classifier keys on the quota
headers described below.

**Measured: the bootstrap endpoint rejects setup tokens.** It returns HTTP 403
`permission_error` with `scope requirement any_of(user:ccr_inference,
user:profile)`, whereas `claude setup-token` mints tokens scoped only to
`user:inference`.
Identity recovery is therefore unavailable for setup-token profiles: they remain
unverified, which the design treats as a usable state, and duplicate-account
detection does not run for them.
This also sets a scope requirement for Phase 3: `user:profile` is required for
account attribution, so the reduced scope set must retain it.

Setup tokens must be pasted rather than piped directly from `claude
setup-token`: because `claude setup-token` is interactive and shell pipelines
start all stages concurrently, a downstream `jp` process would read stdin before
a token exists while `claude` still needs the terminal for prompts.
Piping from a non-interactive source (such as a clipboard tool or file) works
normally.

**Measured: subscription limits are reported in response headers.** These header
names are taken from Claude Code's client source rather than a live rejection,
so their values should be confirmed against the first live limit JP encounters.

A `429` is a quota rejection only when it carries
`anthropic-ratelimit-unified-representative-claim` or
`anthropic-ratelimit-unified-overage-status`; Claude Code treats a `429` without
them as a non-quota rate limit and surfaces the API error directly.
That predicate separates quota exhaustion from capacity throttling and from the
fingerprint rejection above, all three of which return `429 rate_limit_error`.
JP adds two refinements: a `529` (overloaded) is never treated as a quota
rejection regardless of headers, and an explicit `allowed` or `allowed_warning`
status header overrides a named window (whereas a missing status header does
not, matching Claude Code's client).
The `representative-claim` values map directly to the cooldown scopes recorded
in the store, and `anthropic-ratelimit-unified-reset` provides the reset
timestamp in Unix seconds without requiring a separate usage endpoint.
`seven_day` is the longest window, which sets the seven-day cooldown cap.

The client source also confirms that `overage-status` governs paid spillover on
Anthropic's side: a subscription account with extra usage enabled continues
serving requests past its window and bills them, which is why JP never claims a
`profile` entry is free (see Drawbacks).

### Phase 2: quota fallback

Adds subscription-window exhaustion classification keyed on the Phase 1 quota
headers, retry via credential switch, scoped cooldown persistence, the fallback
notice, and immediate retry without backoff.
Depends on Phase 1.

### Phase 2c: provider-owned credential policy and the core store API

Moves credential policy into the Anthropic provider and reshapes the store into
the core-owned API described in Credential store.
Chain preflight and resolution move from `jp_credentials` into the provider;
in-flight switching moves from the CLI stream-retry path into the provider's
request admission; the switch notice becomes a provider-emitted notice event;
provider construction returns to taking config alone across all providers; and
per-call-site credential resolution in the CLI is removed.
Storage backends sit behind the `CredentialBackend` trait under the
lock-mutate-persist cycle, so adding the Keychain backend requires no changes
above that trait.
User-visible behavior is unchanged except that fallback now applies to every
request the provider sends rather than only the query streaming loop.
This phase is numbered `2c` even though it precedes Phase 2b so existing phase
references keep their numbering.
Depends on Phases 1 and 2; Phases 2b, 3, and 4 build on it.

### Phase 2b: subscription-state awareness

Phase 2 reacts to an exhausted allowance after a request fails.
Anthropic sends the same rate-limit headers on every response, and Claude Code's
client shows which headers support proactive reporting:

- **Warning states.** `anthropic-ratelimit-unified-status` reports
  `allowed_warning` alongside the window that is filling up, while
  `-5h-utilization` and `-7d-utilization` carry the consumed fraction with a
  `-surpassed-threshold` marker.
  Surfacing that warning as chrome tells the user their allowance is nearly
  exhausted before a request fails mid-turn.
- **Explaining a refusal.** `-overage-disabled-reason` reports why paid
  spillover could not serve the request (`out_of_credits`, org-level caps, or
  seat-level caps).
  Including that reason in the terminal error tells the user when an
  organization cap blocked spillover rather than showing a generic limit error.
  This only reports on spillover; JP still does not opt into it (see Quota
  fallback).
- **Recording state from successful responses.** Instead of writing cooldowns
  only after a failed request, reading the rate-limit headers on `200` responses
  lets JP record window utilization and reset timestamps without burning a
  rejected request.

This phase is optional for correctness, since Phase 2 handles fallback on
rejection, but turns subscription state into something JP can report before
exhaustion.
It is numbered `2b` rather than `3` to preserve existing phase references.
Depends on Phases 2 and 2c.

**Measured: the header names from Claude Code's client.** Per-window headers use
abbreviated window names where other headers spell them out:
`anthropic-ratelimit-unified-{5h,7d}-utilization` carries the fraction consumed,
`-{5h,7d}-reset` carries the per-window reset timestamp (distinct from the
top-level `-reset`), and `-{5h,7d,overage}-surpassed-threshold` marks a crossed
warning threshold (its presence is the signal and its value is the threshold).
`-overage-disabled-reason` reports one of thirteen enumerated reasons
(`out_of_credits`, `org_level_disabled`, `seat_tier_zero_credit_limit`, and
others).
JP normalizes the `5h` and `7d` abbreviations to the spelled-out window names in
user-facing output.

Claude Code's client also computes a time-relative early warning when the server
sends no threshold (warning at 90% utilization within 72% of the 5-hour window,
plus three tiers for the weekly window).
JP omits that client-side heuristic and surfaces only the warnings flagged by
the server.

**A spent window is readable from a successful response.** When paid extra usage
covers a request, the `200` response carries `status: rejected`.
That makes server-side overage billing (noted in Drawbacks as unpreventable on
the first request) detectable immediately after that request completes.
When JP sees `status: rejected` on a `200` response, it records the scoped
cooldown so the next request advances to the next credential instead of billing
overage again, and emits a notice naming the exhausted window.
The request that triggered the overage is still billed.

### Transport: reading a successful response's headers

Although quota headers are present on every response, JP's Anthropic client
previously read them only on errors because `reqwest-eventsource` yields a unit
`Open` event and exposes the `Response` only in error variants.
Phase 2b therefore starts with a transport change: the streaming path sends the
request directly with `reqwest` and parses the body with `eventsource-stream`
(the SSE parser wrapped by `reqwest-eventsource`), exposing the `Response` and
its headers on `200` responses.

This change intentionally drops SSE-level reconnection.
Anthropic's messages endpoint sends no SSE event IDs, so a reconnect cannot
resume a stream; instead, it starts a new completion from the beginning.
Because callers already treated every stream error as terminal and abandoned the
stream, `reqwest-eventsource`'s 15-second reconnect timer only fired an orphaned
request that billed tokens nobody read.
Stream recovery remains in the turn loop, which flushes partial content,
rebuilds the thread with that content as an assistant prefill, and starts a new
stream.
The `cerebras` and `llamacpp` providers already use the same approach via
`retry::Never`.

In addition, the buffered `get` and `post` helpers previously retried every
`429` with a 15-second minimum backoff, including quota rejections, stalling a
spent subscription for nearly a minute before switching credentials.
They now retry only capacity throttling.

### Phase 3: PKCE browser login

Implements the browser OAuth flow: PKCE, browser launch, localhost callback,
paste fallback, and token refresh on expiry.
The phase lands in two parts:

1. Token exchange and refresh logic lands first as pure library code in the
   provider's auth module, providing the refresh-on-expiry support that
   per-request resolution needs for `oauth` profiles.
2. The interactive login command ships in the provider's command plugin (`jp
   anthropic login`) once the command-plugin protocol supports running without a
   workspace and prompting for user input.
   Until the plugin ships, `jp provider auth login --setup-token` in the core
   CLI remains the login path.

Phase 3 also tests whether the authorize endpoint grants the reduced
`user:inference` and `user:profile` scope set for Claude Code's client ID; if
not, the full scope set and its per-capability justifications will be recorded
in this RFD before the phase ships (see OAuth flow mechanics).
Depends on Phases 1 and 2c; independent of Phase 2.

**Measured: the OAuth endpoints.** The token endpoint is
`https://platform.claude.com/v1/oauth/token` (rather than the
`api.anthropic.com` URL in earlier documentation), and the authorize endpoint
opened by Anthropic's client is `https://claude.com/cai/oauth/authorize`, which
redirects to `claude.ai`.
JP uses the same endpoints as Anthropic's client.
Both token requests send a JSON body rather than form-encoded parameters, and
the paste fallback redirects to
`https://platform.claude.com/oauth/code/callback`.

**Measured: access tokens refresh five minutes before expiry.** Anthropic's
client treats a token within five minutes of `expires_at` as expired so it
cannot lapse between credential resolution and request admission; JP uses the
same five-minute buffer.

**Still unmeasured: whether the reduced scope set is granted.** Verifying the
reduced scope set requires a live browser authorization round-trip, which lands
with the browser login flow; the full scope set remains the fallback if the
reduced set is rejected.

### Phase 4: macOS keyring store backend

Implements the macOS Keychain as a `CredentialBackend` by adapting the
`keyring-core` Apple store (`apple-native-keyring-store`), replacing the file
store on macOS to match Claude Code's Keychain storage.
The backend stores the same versioned JSON document as a single Keychain item so
cooldowns and re-login state stay with the credentials, while the file-based
`ResourceLocker` continues to serialize mutations because the Keychain provides
no locking.

Existing file stores on macOS migrate once under the file lock: JP reads the
file store, writes the Keychain item, reads it back to verify, updates the
backend marker, and removes the plaintext file on a best-effort basis (warning
if removal fails).
Because the entire store is written as a single Keychain item, partial migration
cannot occur; if any step fails, JP stays on the file backend and logs a
warning.
Linux and Windows continue to use the file backend.
Depends on Phases 1 and 2c; independent of Phases 2 and 3.

## References

- [Claude Code client source (de-obfuscated)][claude-code-source]: quota
  headers, error classification, OAuth handling
- [Anthropic OAuth flow reference implementation (oh-my-pi)][oh-my-pi]
- [oh-my-pi request fingerprint (`buildAnthropicHeaders`)][oh-my-pi-headers]
- [OpenClaw OAuth concepts][openclaw]: token sink, refresh rotation, profile
  routing
- [CLIProxyAPI][cliproxy]: proxy-based alternative
- [Using Claude Code with your Pro or Max plan][claude-plans]

[RFD 048]: 048-four-channel-output-model.md
[RFD 110]: 110-anthropic-subscription-queries-via-acp.md
[\#875]: https://github.com/dcdpr/jp/issues/875
[claude-code-source]: https://github.com/alex000kim/claude-code
[claude-plans]: https://support.claude.com/en/articles/11145838-using-claude-code-with-your-pro-or-max-plan
[cliproxy]: https://github.com/router-for-me/CLIProxyAPI
[oh-my-pi]: https://github.com/can1357/oh-my-pi/blob/75bdb20212871221406e119745136edcb2197653/packages/ai/src/registry/oauth/anthropic.ts
[oh-my-pi-headers]: https://github.com/can1357/oh-my-pi/blob/75bdb20212871221406e119745136edcb2197653/packages/ai/src/providers/anthropic.ts
[openclaw]: https://docs.openclaw.ai/concepts/oauth
