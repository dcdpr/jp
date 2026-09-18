//! The core-owned credential store.
//!
//! Credentials for LLM providers live in a user-global store, managed with `jp
//! provider auth login|list|logout` and consumed by each provider's own
//! credential-chain resolution (`providers.llm.anthropic.auth`).
//!
//! This crate owns storage integrity — the document encoding, the schema
//! version check, and the lock-mutate-persist cycle — and exposes it as an
//! API: providers in core call it directly, and plugins reach the same
//! operations through host protocol messages, so the on-disk format is never a
//! cross-binary contract.
//!
//! Credential *policy* — which chain entry to use, when to skip, refresh, or
//! switch — is provider-owned and lives with each provider implementation, so
//! it migrates into a plugin together with the provider.

pub mod store;

pub use store::{
    CredentialBackend, CredentialSecret, CredentialStore, DEFAULT_COOLDOWN, FsCredentialBackend,
    InMemoryCredentialBackend, MAX_COOLDOWN, SCOPE_ACCOUNT, StoreDocument, StoreError,
    StoredCredential, cooldown_until,
};

/// The store category for LLM provider credentials.
pub const CATEGORY_LLM: &str = "llm";

/// The sole provider implemented under [`CATEGORY_LLM`].
pub const PROVIDER_ANTHROPIC: &str = "anthropic";
