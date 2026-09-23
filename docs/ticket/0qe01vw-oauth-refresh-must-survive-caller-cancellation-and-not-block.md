# OAuth refresh must survive caller cancellation and not block executor threads

- **Status**: Todo
- **Kind**: Bug
- **Authors**: jp
- **Date**: 2026-09-23
- **Implements**: 090
- **Label**: domain=llm
- **Label**: llm-provider=anthropic
- **Label**: package=jp_llm
- **Label**: type=bug

Two problems in `jp_llm::provider::anthropic::resolve` that cannot happen yet,
because no shipped login creates a `CredentialSecret::Oauth` record.
Setup-token login stores `Token`, so no refresh ever runs.
Both **must be fixed before RFD 090 Phase 3 (browser login) ships**.

Raised in review of #1151 (comments `4081033143` and `4081035326`).

## 1. A cancelled refresh loses the rotated token

`refresh_under_lock` holds the store lock across `oauth::refresh` and persists
afterwards.
Its future is owned by whoever called `resolve`.
`TitleGeneratorTask::run` selects cancellation against the title work, which
reaches this through `model_details`.
If the cancellation lands after the token endpoint has rotated the refresh token
but before its response is persisted, the future and the guard are dropped.
The replacement is lost, the next invocation presents the retired token, and the
profile is marked for re-login.

**Fix:** run the lock, recheck, exchange and persist steps as one
`tokio::spawn`ed task and await its `JoinHandle`.
Dropping a handle does not cancel the task, so the transaction completes
whatever happens to the caller.

**Test:** a mock token endpoint that pauses after accepting the refresh.
Cancel the caller while it is paused, release the endpoint, then assert the
rotated token was persisted.

## 2. Outcome writes block executor threads on the refresh lock

`record_outcome` calls the synchronous `CredentialStore::mutate`, which blocks
in `ResourceLocker::lock` (no timeout).
`QuotaWatch::observe` does the same from stream polling.
A refresh holds that lock across a network await.
On a runtime whose worker threads are all parked in these blocking writers
(`--threads 1` is enough), the refresh can never receive its response or run its
timeout, so neither side releases the other.

**Fix:** route both writes through `spawn_blocking`, the way `acquire` already
does for the refresh.
`QuotaWatch::observe` becomes async.

**Test:** on a single-worker runtime, a pending refresh competes with an outcome
write, and both complete.
