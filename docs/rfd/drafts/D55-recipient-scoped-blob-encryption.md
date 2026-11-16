# RFD D55: Recipient-Scoped Blob Encryption

- **Status**: Draft
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-06-04
- **Extends**: [RFD 066][RFD 066-2]
- **Summary**: Adds optional per-blob, per-recipient `age` encryption to the
  content-addressable blob store for private attachments in shared
  conversations.

## Summary

This RFD adds optional per-blob encryption to the content-addressable blob store
([RFD 066]) using [`age`] so individual attachments in a shared conversation can
be encrypted for specific recipients.
Recipients with a matching private identity read the blob transparently, while
other teammates see a placeholder.

## Motivation

Conversations are shared across a team via Git, and [RFD 066] commits blob
content alongside `events.json` so teammates can read, continue, and fork them.
That sharing is all-or-nothing: every blob in the workspace store is readable by
anyone with access to the repository.

This model fails when a user wants to attach a local file that is relevant to
the conversation but not meant for the whole team, such as a personal note, a
document under NDA, credentials, or a file from another project.
Without per-blob encryption, users must either copy sensitive files into the
workspace and share them with everyone, or keep the data out of JP and lose the
ability to reference it in a conversation.

The required property is **per-blob, per-recipient encryption inside an
otherwise-shared conversation**: an individual resource is readable only by its
author or a chosen subset of teammates, while the rest of the conversation
remains shared and readable.
A single recipient (`{self}`) covers a private attachment, and a larger
recipient list covers a team subset through the same mechanism.

Repository-level encryption tools such as `git-crypt` already encrypt files at
rest for a repository-wide key set, and teams that want to encrypt the entire
`.jp/` directory against repository theft can use them without support from JP.
However, `git-crypt` operates on path patterns with a single key set; it cannot
encrypt an individual blob for specific recipients inside a conversation that
everyone else can read.

## Design

### Attaching and rendering encrypted blobs

A user marks an attachment as encrypted with `--encrypt` and optionally names
the recipients with `--recipient` (defaulting to themselves):

```sh
# Readable only by me.
jp q --attach ~/Downloads/private.pdf --encrypt "Summarize this"

# Readable by me, Alice, and Bob.
jp q --attach ./spec.pdf --encrypt --recipient alice --recipient bob "Review this"
```

Recipients are resolved from configured public keys.
On the author's machine and on any recipient's machine, the resource behaves
like an unencrypted attachment.
On a non-recipient's machine, `jp conversation print` renders a placeholder in
place of the content:

> *[resource `external:…/private.pdf` is encrypted for {jean} and cannot be
> decrypted with the local identity]*

The conversation structure, turn boundaries, and all other content remain
readable; only the encrypted blob is opaque.

### Recipients and identities

Encryption uses two pieces of key material with different trust requirements and
locations in the configuration tree:

- **Recipient public keys**: Stored in workspace or conversation config
  (`encryption.recipients`), keyed by a short alias.
  Because public keys are not secret, they are safe to commit.
- **Local private identity**: Configured as a reference in user-global config
  (`~/.config/jp/`), such as a path to an `age` identity file, an SSH key, or a
  plugin identity.
  Because the private identity is secret, it is never stored inline in any
  committable config.

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

A blob's recipient set defaults to `{self}`.
The implementation treats the recipient list uniformly as `recipients: Vec<_>`,
so single-recipient and multi-recipient encryption follow the same code path.
Configuring the default recipient set at the workspace or conversation scope
rather than only per attachment also supports future conversation-level
encryption (see [Forward compatibility with conversation
encryption](#forward-compatibility-with-conversation-encryption)).

### Envelope encryption via age

JP delegates all cryptography to the [`age`] library and uses its standard
envelope model:

1. Generate a random data-encryption key (DEK) per blob.
2. Encrypt the blob content with the DEK using authenticated encryption.
3. Wrap the DEK once for each recipient's public key.

Any listed recipient unwraps the DEK with their private identity and decrypts
the content; anyone without a matching identity cannot unwrap the DEK.

`age`'s recipient model provides a pluggable backend interface without custom JP
abstractions.
Each recipient string selects its backend: `age1…` for native `age` keys, an
SSH public key directly, or a plugin recipient such as `age1yubikey1…`, which
`age` resolves to an `age-plugin-*` binary on `$PATH` for hardware or cloud keys
(YubiKey, TPM, Secure Enclave, KMS).
JP passes recipient strings directly to the `age` crate (which exposes plugin
support via `RecipientPluginV1` and `IdentityPluginV1`) without inspecting which
backend each recipient selects.

### Blob metadata

Encryption extends [RFD 066]'s `content` object.
The `$blob` reference stores the SHA-256 address of the **ciphertext**, and a
sibling `encryption` descriptor records the scheme and recipients:

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

The `encryption` descriptor sits in `events.json` in cleartext.
Because `recipients` contains only public keys, nothing secret is exposed.
The `scheme` tag provides forward compatibility if a future backend cannot be
expressed through `age`'s recipient model; `age` is the only scheme defined by
this RFD.

The `age` ciphertext header also embeds the recipient stanzas, making the
encrypted blob self-describing as to who can decrypt it.
The `recipients` field in `events.json` mirrors that header for two reasons:

- Rendering the placeholder and listing recipients without reading or decrypting
  the blob, preserving [RFD 066]'s metadata-only fast paths.
- Exposing the intended recipient set to rekey tooling without decrypting every
  blob.

If the two disagree, such as after an interrupted rekey, the ciphertext header
is authoritative for access and the `events.json` field is advisory.

### Decrypt-or-placeholder boundary

[RFD 066] loads blob content lazily through the `resolve` step on
`BlobContent::Ref`.
Encryption hooks into this boundary: when a blob carries an `encryption`
descriptor, `resolve` decrypts the ciphertext with the local identity.
If the local identity does not match any recipient, `resolve` returns a typed
non-recipient outcome and the renderer emits the placeholder.
Metadata-only operations (listing, titles, and garbage collection) never call
`resolve` and remain unaffected.

### Deduplication and token estimation

Because each encryption generates a fresh random DEK, encrypting the same
plaintext twice produces distinct ciphertexts.
Encrypted blobs therefore opt out of three blob-store behaviors:

- **Cross-conversation deduplication ([RFD 066])**: Ciphertext addresses do not
  collide across encryptions, so identical plaintext encrypted twice produces
  two blobs.
- **Token deduplication ([RFD 067])**: Because per-block content identity is
  unavailable across encryptions, encrypted resource blocks are never
  deduplicated.
- **Metadata-only token estimation**: `size` records the ciphertext size, and
  computing the plaintext size requires decryption, so it is unavailable to the
  metadata path.

These exclusions are acceptable because encryption is opt-in and infrequent
compared to the unencrypted `fs_read_file` hot path that deduplication targets.

### Rekeying and key compromise

Two operations update the recipient set of existing encrypted blobs:

- **Add a recipient**: Unwrap the existing DEK and re-wrap it to the new
  recipient's public key without re-encrypting the content.
  This is a fast operation used when a trusted recipient joins.
- **Rekey**: Generate a fresh DEK, **re-encrypt the content**, and wrap the new
  DEK to the updated recipient set.
  This is required for key rotation and compromise recovery: because a
  compromised party already holds the old DEK, only a new DEK over new
  ciphertext protects future writes.

Both operations write new ciphertext at a new address, leaving the old blob
orphaned for the garbage-collection sweep to reclaim.

**Rekeying provides forward protection only.** Once a private key has leaked to
an attacker with access to the repository, any blobs already committed under the
old DEK remain permanently decryptable by that attacker.
Re-encrypting the working tree writes new blobs, but the old ciphertext remains
in Git history, as with any encryption-at-rest scheme layered on a distributed
VCS.

Recovering from a key compromise therefore requires three steps:

1. Rekey in JP so future writes no longer use the compromised key or old DEK.
2. Rotate any secret already pushed to a repository the attacker can read (such
   as API keys, credentials, or document access) at its source.
3. Purge the old ciphertext from Git history using external VCS tools (`git
   filter-repo` or BFG).

### Forward compatibility with conversation encryption

Encrypting the conversation itself (`events.json`) is a separate design (see
[Non-Goals](#non-goals)), but it shares the same underlying mechanism: `age`
envelope encryption, recipient sets, identities, rekeying, and
decrypt-or-placeholder fallback.
Two design choices in this RFD keep that path open:

1. The `age` primitive and the `{ scheme, recipients }` descriptor live in a
   standalone encryption module (a small `jp_crypto`-style concern) rather than
   inside the blob reference types, so both blob encryption and future file
   encryption can call the same primitive.
2. The default recipient set is configured at workspace or conversation scope
   with per-attachment overrides, matching the scoping conversation-level
   encryption would reuse.

## Drawbacks

- **Cryptographic dependency on `age`.** Adding a cryptographic library carries
  a security and maintenance contract, mitigated by choosing a well-scoped,
  widely used library and delegating all cryptography to it.
- **Loss of deduplication and token estimation.** Re-attaching the same
  encrypted file stores a new blob each time, and the token cost of an encrypted
  resource cannot be estimated without decrypting it.
- **Forward-only compromise recovery.** Rekeying cannot protect data already
  pushed under a leaked key, so CLI output and user documentation must state
  this limitation clearly rather than imply full recovery.
- **Reduced context for non-recipients.** A teammate without a matching private
  identity sees a placeholder instead of the attachment content, which degrades
  collaborative workflows if applied to content the whole team needs.

## Alternatives

### Use git-crypt or sops for the whole store

Encrypting `.jp/blobs/` at the VCS layer with `git-crypt` or `sops` uses a
repository-wide key set matched by path pattern.
This is **rejected as the primary design** because it cannot express per-blob,
per-recipient encryption inside a shared conversation.
It remains the right tool for coarse repository-wide encryption at rest.

### Define a custom external encryption-tool interface

JP could define its own trait or command protocol and delegate encryption and
decryption to an external tool.
This is **rejected** because `age`'s recipient and plugin protocol already
provides a specified, audited interface with existing hardware and KMS plugins.
A custom JP interface would have no existing implementations and would pipe full
plaintext content across a subprocess boundary, whereas `age` performs content
AEAD in-process and only passes the small DEK wrap across the plugin boundary.

### Address encrypted blobs by plaintext hash

Addressing encrypted blobs by their plaintext SHA-256 while storing ciphertext
on disk would preserve deduplication.
This is **rejected** because the plaintext hash would sit in cleartext in
`events.json`, creating a confirmation oracle: any teammate who guesses a file
can hash their copy to confirm it was attached.
Addressing by plaintext hash also breaks storage integrity verification (which
re-hashes the stored bytes) and causes collisions when two users encrypt the
same plaintext for different recipient sets.

### Convergent encryption

Deriving the DEK deterministically from the plaintext would make identical
plaintexts produce identical ciphertexts, preserving deduplication.
This is **rejected** because it reintroduces the file-confirmation oracle and
adds cryptographic complexity for a deduplication gain that rarely applies on
the encrypted path.

### Random per-attachment URIs / UUIDs

Assigning a random identifier to each encrypted attachment is **rejected** for
the same reason [D03] rejects random identifiers for external attachments: it
breaks repeated-attachment identity and provides nothing that random-DEK
encryption does not already provide.

## Non-Goals

- **Conversation-level encryption.** Encrypting `events.json` itself is deferred
  to a separate RFD.
  It breaks [RFD 066]'s premise that `events.json` is a readable skeleton for
  non-recipients (listing, titles, and garbage collection all read it without
  blob content), so its degradation model differs fundamentally from per-blob
  encryption (see [Forward compatibility with conversation
  encryption](#forward-compatibility-with-conversation-encryption)).
- **Rekey authorization policy.** Any teammate holding a matching private
  identity can unwrap the DEK and run a rekey; team policy governing who is
  permitted to rekey is deferred.
- **Team removal semantics.** Removing a recipient provides forward-only
  protection for the same reason as key compromise, since the removed party
  already holds the repository.
  Semantics and tooling for managing a changing team recipient set are deferred;
  a static recipient set declared in config needs none of it.
- **Coarse encryption at rest.** Encrypting the entire workspace against
  repository theft is left to `git-crypt` or `sops`.

## Risks and Open Questions

- **`size` semantics.** Because `size` records ciphertext length on encrypted
  blobs, any consumer using it for plaintext token estimation must treat
  encrypted blobs as unknown-size.
  Implementation must confirm that no existing consumer assumes `size` is always
  plaintext length.
- **Placeholder wording.** The placeholder appears both in transcripts for
  non-recipients and in the LLM context if a non-recipient continues the
  conversation.
  The exact text must be validated so it is clear without leaking content.
- **Metadata vs ciphertext drift.** The `recipients` mirror in `events.json` can
  drift from the `age` header after an interrupted rekey.
  The `resolve` and rekey paths must treat the ciphertext header as
  authoritative and the mirror as advisory, and tooling should be able to repair
  the mirror.
- **Identity loss.** If the sole recipient loses their private identity, the
  blob is permanently unrecoverable.
  This is inherent to client-side encryption and should be documented in
  user-facing docs.
- **Non-UTF-8 and binary content.** Encryption operates on raw bytes regardless
  of content type, so binary blobs encrypt the same way as text, but integration
  with [RFD 066]'s binary path must be verified.

## Implementation Plan

### Phase 1: Shared encryption module

Add a standalone encryption module wrapping `age` with a `{ scheme, recipients
}` descriptor type, recipient and identity parsing (native `age`, SSH, and
plugin keys), and `encrypt(bytes, recipients)` and `decrypt(bytes, identity)`
functions over the envelope model.
This phase includes no blob-store wiring, depends only on adding the `age`
dependency, and is mergeable independently.

### Phase 2: Config surface

Add `encryption.recipients` (workspace and conversation config, public keys by
alias) and the user-global `encryption.identity` reference.
Validate that the private identity is never read from committable config.
This phase depends on Phase 1.

### Phase 3: Blob store integration

Extend [RFD 066]'s `content` object with the `encryption` descriptor, address
encrypted blobs by ciphertext hash, and hook decrypt-or-placeholder into the
`resolve` path.
Exclude encrypted blobs from cross-conversation deduplication and token
estimation.
This phase depends on Phase 1, Phase 2, and [RFD 066].

### Phase 4: Attachment surface

Add `--encrypt` and `--recipient` to `--attach`, defaulting the recipient set to
`{self}`, and render the placeholder in `jp conversation print` for
non-recipients.
This phase depends on Phase 3.

### Phase 5: Rekey tooling

Implement add-recipient (DEK re-wrap) and rekey (fresh DEK plus re-encryption)
over a conversation's encrypted blobs, documenting the forward-only limitation
in the command output and user docs.
This phase depends on Phase 3.

## References

- [RFD 066: Content-Addressable Blob Store][RFD 066] defines the blob store, the
  `content` object, and the lazy `resolve` boundary that this RFD extends.
- [RFD 065: Typed Resource Model for Attachments][RFD 065] defines the
  `Resource` type whose content is stored as blobs.
- [RFD 067: Resource Deduplication for Token Efficiency][RFD 067] defines the
  per-block deduplication behavior that encrypted blobs opt out of.
- [D03: External Attachment URI Scheme][D03] defines the external-attachment
  workflow that motivates the private-attachment use case.
- [`age`] provides the encryption library and recipient/plugin model that JP
  delegates to.

[D03]: D03-external-attachment-uri-scheme.md
[RFD 065]: ../065-typed-resource-model-for-attachments.md
[RFD 066]: ../066-content-addressable-blob-store.md
[RFD 066-2]: ../066-content-addressable-blob-store.md
[RFD 067]: ../067-resource-deduplication-for-token-efficiency.md
[`age`]: https://github.com/FiloSottile/age
