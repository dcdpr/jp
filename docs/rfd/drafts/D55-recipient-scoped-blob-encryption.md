# RFD D48: Recipient-Scoped Blob Encryption

- **Status**: Draft
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-06-04
- **Extends**: [RFD 066][RFD 066-2]

## Summary

This RFD adds optional, per-blob encryption to the content-addressable blob
store ([RFD 066]).
A blob can be encrypted to a set of recipients, so its content is readable only
by holders of a matching private identity.
The primary use case is attaching a private file to an otherwise-shared
conversation: the author and any chosen recipients can read it, every other
teammate sees a placeholder.
Encryption is delegated to [`age`]; JP owns the policy (what is encrypted, for
whom), not the cryptography.

## Motivation

Conversations are shared across a team via Git, and [RFD 066] commits blob
content alongside `events.json` so teammates can read, continue, and fork them.
That sharing is all-or-nothing: every blob in the workspace store is readable by
anyone with the repository.

A common case breaks under that model.
A user attaches a local file that is relevant to the conversation but not meant
for the whole team: a personal note, a document under NDA, credentials, a file
from another project.
Today the only options are "don't attach it" or "share it with everyone."

The valuable property is **per-blob, per-recipient encryption inside an
otherwise-shared conversation**: one resource is readable only by its author (or
a chosen subset), while the rest of the conversation stays shared and readable.
A single recipient (`{self}`) covers the private-attachment case; a larger
recipient set covers a fixed team subset.
These are the same mechanism with a different recipient list.

### What doing nothing costs

Users who need this either pollute the workspace by copying files in and
accepting that they are shared, or keep the data out of JP entirely and lose the
ability to reference it in a conversation.

### Why this is not git-crypt's job

Tools like `git-crypt` already encrypt files at rest for a repository-wide key
set, and a team that wants coarse "encrypt the whole `.jp/` directory against
repository theft" can use them today with no work from JP.
They cannot express the property above: their granularity is a path pattern with
one key set, not one blob to one recipient inside a conversation everyone else
can read.
That granularity is what justifies building this in JP.

## Design

### What the user sees

A user marks an attachment as encrypted and names the recipients (defaulting to
themselves):

```sh
# Readable only by me.
jp q --attach ~/Downloads/private.pdf --encrypt "Summarize this"

# Readable by me, Alice, and Bob.
jp q --attach ./spec.pdf --encrypt --recipient alice --recipient bob "Review this"
```

Recipients are resolved from configured public keys (see below).
On the author's machine and on any recipient's machine, the resource behaves
exactly as an unencrypted attachment: same content in, same output.
On a non-recipient's machine, `jp conversation print` renders a placeholder in
place of the content:

> *[resource `external:…/private.pdf` is encrypted for {jean} and cannot be
> decrypted with the local identity]*

The conversation structure, turn boundaries, and all other content remain
readable.
Only the encrypted blob is opaque.

### Recipients and identities

Two pieces of key material, with different trust requirements and different
homes in the config tree:

- **Recipient public keys** are public.
  They live in workspace or conversation config (`encryption.recipients`), keyed
  by a short alias, and are safe to commit.
- **The local private identity** is secret.
  It is a *reference* in user-global config (`~/.config/jp/`), never an inline
  value in any committable config: a path to an age identity file, an ssh key,
  or a plugin identity.

<!-- end list -->

```toml
# Workspace config (committed): public keys, safe to share.
[encryption.recipients]
jean  = "age1qy8e...
alice = "ssh-ed25519 AAAAC3Nz..."
bob   = "age1yubikey1qg8..."

# User-global config (never committed): a reference to the private identity.
[encryption]
identity = "~/.config/jp/age/identity.txt"
```

The recipient set for a blob defaults to `{self}`.
The recipient count is a configuration value, never a structural boundary in the
code: `recipients: Vec<_>` handles one recipient and ten through the same path.
Locating the default recipient set at conversation/workspace scope (rather than
only per-attachment) is deliberate; see [Forward
compatibility](#forward-compatibility-with-conversation-encryption).

### Envelope encryption via age

JP does not implement cryptography.
It depends on the [`age`] library and uses its standard envelope model:

1. Generate a random data-encryption key (DEK) per blob.
2. Encrypt the content with the DEK (authenticated encryption).
3. Wrap the DEK once per recipient public key.

Any recipient unwraps the DEK with their private identity, then decrypts the
content.
A non-recipient cannot unwrap the DEK and cannot read the content.

age's recipient model **is** the pluggable backend the project would otherwise
be tempted to design.
A recipient string selects its backend: `age1…` for native age keys, an ssh
public key directly, or a plugin recipient such as `age1yubikey1…`, which age
resolves to an `age-plugin-*` binary on `$PATH` (YubiKey, TPM, Secure Enclave,
KMS plugins already exist).
JP passes recipient strings to age and does not care which backend each one
selects.
The `age` Rust crate exposes this via `RecipientPluginV1` / `IdentityPluginV1`.

### Blob metadata

Encryption extends [RFD 066]'s `content` object.
The `$blob` reference becomes the address of the **ciphertext**, and a sibling
`encryption` descriptor records the scheme and recipients:

```json
"content": {
  "$blob": "<sha256 of ciphertext>",
  "size": 4096,
  "encryption": {
    "scheme": "age",
    "recipients": ["age1qy8e...", "age1yubikey1qg8..."]
  }
}
```

This descriptor sits in `events.json` in cleartext.
Nothing secret leaks: recipients are public keys.

The `scheme` tag is the forward escape hatch.
The only scheme defined here is `age`.
A second scheme is added only if a real user needs a backend that age's
recipient model cannot express; until then, the tag exists but has one value.

The age ciphertext header also embeds the recipient stanzas, so the encrypted
blob is self-describing about *who can actually decrypt it*.
The `recipients` field in `events.json` is therefore a **mirror** of that
header, kept for two reasons: rendering the placeholder and listing recipients
without reading or decrypting the blob (preserving [RFD 066]'s metadata-only
fast paths), and giving rekey tooling the intended recipient set without
decrypting every blob.
When the two disagree (for example after a partial rekey), the ciphertext header
is authoritative for access; the `events.json` field is advisory.

### Decrypt-or-placeholder boundary

[RFD 066] already loads blob content lazily through a `resolve` step on
`BlobContent::Ref`.
Encryption hooks in there: if the blob carries an `encryption` descriptor,
`resolve` decrypts with the local identity.
If no local identity matches a recipient, `resolve` returns a typed "not a
recipient" outcome and the renderer emits the placeholder.
Metadata-only operations (listing, titles, GC) never call `resolve` and are
unaffected.

### Relationship to dedup and token estimation

Encrypted blobs are a distinct class that opts out of three blob-store
behaviors, because a fresh random DEK makes each encryption produce unique
ciphertext:

- **Cross-conversation dedup ([RFD 066])**: ciphertext addresses do not collide,
  so identical plaintext encrypted twice produces two blobs.
- **Token dedup ([RFD 067])**: the same per-block identity is unavailable, so
  encrypted resource blocks are never deduplicated.
- **Metadata-only token estimation**: `size` is the ciphertext size, and the
  plaintext size is not exposed to the metadata path (it would require
  decryption).

These exclusions are acceptable: encryption is opt-in and rare, not the
`fs_read_file` hot path that dedup targets.

### Rekey and key compromise

A key can change, and the design must answer "my key was compromised" rather
than declare the data lost.
Two operations cover this, both expressed over the same recipient set:

- **Add a recipient.** Re-wrap the existing DEK to the new recipient's public
  key.
  Cheap, no re-encryption, the content key is unchanged.
  Used when a trusted recipient joins.
- **Rekey.** Generate a fresh DEK, **re-encrypt the content**, and wrap to the
  new recipient set.
  Required for rotation and compromise: whoever was compromised already holds
  the old DEK, so only a new DEK over new ciphertext protects future reads.

Both produce new ciphertext at a new address; the old blob is orphaned and the
GC sweep reclaims it.

**Rekey is forward protection only, and the RFD states this plainly.** Once a
private key has leaked and the attacker has the repository, the blobs already
committed under the old DEK are decryptable by them, permanently.
Re-encrypting the working tree produces new blobs, but the old ciphertext
remains in Git history, which the attacker already cloned.
No JP operation can un-leak that.
This is true of every encryption-at-rest scheme layered on a distributed VCS.

The responsible answer to a compromise report is therefore:

1. JP rekeys, so everything written from now on is safe from the compromised
   key.
2. Any secret already pushed to a repository the attacker can read is burned and
   must be rotated at its source (the API key, credential, or document access).
   JP cannot do this part.
3. Purging the old ciphertext from history is the user's VCS operation (`git
   filter-repo` / BFG), not JP's.

### Forward compatibility with conversation encryption

A likely future direction is encrypting the conversation itself (`events.json`),
not just its blobs.
The *mechanism* is shared (age, recipient sets, identities, rekey,
decrypt-or-placeholder); the *feature* is a separate, heavier design and is a
Non-Goal here.
Two cheap decisions in this RFD keep that door open:

1. The age primitive and the `{ scheme, recipients }` descriptor live in a
   standalone encryption concern (a small `jp_crypto`-style module), not welded
   into the blob reference and not named after blobs.
   Both "encrypt a blob" and a future "encrypt a file" call the same primitive.
2. The default recipient set is configured at conversation/workspace scope, with
   per-attachment as an override.
   That scoping is exactly what conversation-level encryption would reuse.

## Drawbacks

**A new dependency on `age`.** This is a cryptographic dependency, which carries
a security and maintenance contract.
It is mitigated by `age` being a well-scoped, widely-used library and by JP
delegating all cryptography to it rather than implementing any.

**Encrypted blobs lose dedup and token estimation.** Re-attaching the same
encrypted file produces a new blob each time, and the token cost of an encrypted
resource cannot be estimated without decrypting it.

**Compromise recovery is forward-only.** The design cannot protect data already
pushed under a leaked key, and the RFD has to communicate that limitation
honestly rather than imply full recovery.

**Non-recipients lose information.** A teammate without the key sees a
placeholder, not the content.
For genuinely shared work this is a downgrade; the feature is for the cases
where that is the intent.

## Alternatives

### Use git-crypt or sops for the whole store

Encrypt `.jp/blobs/` at the VCS layer with a repository-wide key set.
**Rejected as the primary design** because it cannot express per-blob,
per-recipient encryption inside a shared conversation: its granularity is a path
pattern with one key set.
It remains the right tool for coarse "encrypt the repo at rest," and the RFD
points users there for that case.

### Have JP define its own external encryption-tool interface

Define a JP trait or command protocol and let an external tool implement
encrypt/decrypt.
**Rejected** because age's recipient and plugin protocol already is that
interface, and is specified and audited.
A JP-defined interface would have zero implementations behind it (the midlayer
mistake) and would pipe full plaintext content across a process boundary JP
designed, a strictly larger attack surface than age, which keeps content AEAD
in-process and only moves the small DEK wrap across the plugin boundary.

### Address encrypted blobs by plaintext hash

Keep dedup by storing the plaintext SHA-256 as the blob address (storing
ciphertext).
**Rejected** because the plaintext hash would sit in cleartext `events.json`,
which a teammate has via Git, creating a confirmation oracle: anyone who guesses
the file can hash their copy and confirm it was attached.
It also breaks integrity verification (cannot re-hash the stored bytes) and
collides when two recipients encrypt the same plaintext to different keys.

### Convergent encryption

Derive the DEK deterministically from the plaintext so identical plaintext
encrypts identically and dedup survives.
**Rejected** because it reintroduces the confirmation-of-file weakness and adds
real cryptographic complexity for a dedup gain that does not matter on the rare
encrypted path.

### Random per-attachment URIs / UUIDs

Use a random identifier per encryption act.
**Rejected** for the same reason [D03] rejects it for external attachments: it
breaks repeated-attachment identity and gives nothing encryption does not
already provide.

## Non-Goals

- **Conversation-level encryption.** Encrypting `events.json` itself is a
  separate RFD.
  It breaks [RFD 066]'s premise that `events.json` is a readable skeleton for
  non-recipients (listing, titles, GC all read it without content), so its
  degradation model differs fundamentally from per-blob encryption.
  This RFD only makes that future cheaper to build (see [Forward
  compatibility](#forward-compatibility-with-conversation-encryption)).

- **Rekey authorization policy.** Who on a team is permitted to run a rekey
  (they need a current private identity to unwrap the DEK first) is policy, not
  cryptography, and is deferred.

- **Team removal semantics.** Removing a recipient is forward-only by the same
  logic as compromise: the removed party already holds the repository.
  The semantics and tooling for managing a *changing* team recipient set are
  deferred; a static recipient set declared in config needs none of it.

- **Coarse encryption at rest.** Encrypting the whole workspace against
  repository theft is left to git-crypt / sops.

## Risks and Open Questions

- **`size` semantics.** With `size` recording ciphertext length, any consumer
  using it for plaintext token estimation must treat encrypted blobs as
  unknown-size.
  Confirm no current consumer assumes `size` is plaintext length.

- **Placeholder wording.** The placeholder is what a non-recipient sees in
  transcripts and what the LLM sees if a non-recipient continues the
  conversation.
  The exact text should be validated so it is clear without leaking content.

- **Metadata vs ciphertext drift.** The `events.json` `recipients` mirror can
  drift from the age header after a partial rekey.
  The resolve and rekey paths must treat the header as authoritative and the
  mirror as advisory, and tooling should be able to repair the mirror.

- **Identity loss.** If the sole recipient loses their private identity, the
  blob is unrecoverable.
  This is inherent and should be documented in user-facing docs, not solved
  here.

- **Non-UTF-8 and binary content.** Encryption operates on raw bytes and is
  agnostic to content type, so binary blobs encrypt the same as text; no special
  handling is expected, but it should be verified against [RFD 066]'s binary
  path.

## Implementation Plan

### Phase 1: Shared encryption module

Add a small encryption concern wrapping `age`: a `{ scheme, recipients }`
descriptor type, recipient/identity parsing (native age, ssh, plugin), and
`encrypt(bytes, recipients)` / `decrypt(bytes, identity)` over the envelope
model.
No blob-store wiring yet.
Depends on choosing the `age` dependency.
Mergeable independently.

### Phase 2: Config surface

Add `encryption.recipients` (workspace/conversation, public keys by alias) and
the user-global `encryption.identity` reference.
Validate that the private identity is never read from committable config.
Depends on Phase 1.

### Phase 3: Blob store integration

Extend [RFD 066]'s `content` object with the `encryption` descriptor, address
encrypted blobs by ciphertext hash, and hook decrypt-or-placeholder into the
`resolve` path.
Exclude encrypted blobs from cross-conversation dedup and from token estimation.
Depends on Phase 1, Phase 2, and [RFD 066].

### Phase 4: Attachment surface

Add `--encrypt` / `--recipient` to `--attach`, defaulting the recipient set to
`{self}`.
Render the placeholder in `jp conversation print` for non-recipients.
Depends on Phase 3.

### Phase 5: Rekey tooling

Implement add-recipient (re-wrap) and rekey (fresh DEK plus re-encrypt) over a
conversation's encrypted blobs, with the forward-only limitation documented in
the command output and user docs.
Depends on Phase 3.

## References

- [RFD 066: Content-Addressable Blob Store][RFD 066] — defines the blob store,
  the `content` object, and the lazy `resolve` boundary this RFD extends.
- [RFD 065: Typed Resource Model for Attachments][RFD 065] — defines the
  `Resource` type whose content is stored as blobs.
- [RFD 067: Resource Deduplication for Token Efficiency][RFD 067] — the dedup
  behavior encrypted blobs opt out of.
- [D03: External Attachment URI Scheme][D03] — the external-attachment workflow
  that motivates the private-attachment use case.
- [`age`] — the encryption library and recipient/plugin model JP delegates to.

[D03]: D03-external-attachment-uri-scheme.md
[RFD 065]: ../065-typed-resource-model-for-attachments.md
[RFD 066]: ../066-content-addressable-blob-store.md
[RFD 066-2]: ../066-content-addressable-blob-store.md
[RFD 067]: ../067-resource-deduplication-for-token-efficiency.md
[`age`]: https://github.com/FiloSottile/age
