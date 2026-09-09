//! The user-global credential store.
//!
//! Credentials live in a single versioned JSON document, keyed by category
//! (`llm`), provider (`anthropic`), and profile name.
//! [`CredentialBackend`] abstracts where the serialized document is kept: the
//! file backend is the baseline, and the macOS Keychain backend (RFD 090, Phase
//! 4) swaps in behind the same interface.
//!
//! [`CredentialStore`] owns the semantics every backend shares: the document
//! encoding and schema-version check, and the mutation cycle — acquire the
//! cross-process lock, re-read, apply, persist.
//! The lock is a [`ResourceLocker`] and stays file-based even for non-file
//! backends, since e.g. the Keychain provides no locking of its own.

use std::{
    collections::BTreeMap,
    fmt::Debug,
    io::{self, Write as _},
    sync::{Arc, Mutex},
};

use camino::{Utf8Path, Utf8PathBuf};
use camino_tempfile::NamedUtf8TempFile;
use chrono::{DateTime, Utc};
use jp_storage::resource_lock::{FsResourceLocker, LockError, ResourceLocker};
use serde::{Deserialize, Serialize};

/// The store filename inside JP's user data directory.
pub const STORE_FILENAME: &str = "credentials.json";

/// The resource name the store's mutation lock is held under.
const LOCK_RESOURCE: &str = "credentials";

/// How long a quota cooldown lasts when the provider reported no reset timing.
///
/// The costs are asymmetric, so this biases short: a too-short cooldown wastes
/// one admission-rejected request per expiry and bills no tokens, while a
/// too-long one spends per-token money while a recovered allowance sits idle.
pub const DEFAULT_COOLDOWN: chrono::TimeDelta = chrono::TimeDelta::minutes(30);

/// The longest cooldown that can be recorded, matching the longest usage window
/// a subscription is known to use.
///
/// Caps observed reset timing so a misparsed timestamp cannot make a profile
/// unusable indefinitely.
pub const MAX_COOLDOWN: chrono::TimeDelta = chrono::TimeDelta::days(7);

/// The cooldown scope covering every model on an account.
pub const SCOPE_ACCOUNT: &str = "account";

/// When a cooldown recorded now should expire.
///
/// Provider-reported timing wins when it is in the future, capped at
/// [`MAX_COOLDOWN`]; otherwise [`DEFAULT_COOLDOWN`] applies.
#[must_use]
pub fn cooldown_until(reported: Option<DateTime<Utc>>, now: DateTime<Utc>) -> DateTime<Utc> {
    match reported {
        Some(reset) if reset > now => reset.min(now + MAX_COOLDOWN),
        _ => now + DEFAULT_COOLDOWN,
    }
}

/// The newest store schema this build can read and write.
const STORE_VERSION: u32 = 1;

/// Errors from loading or mutating the credential store.
#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("cannot locate the user data directory (no home directory?)")]
    MissingDataDir,

    #[error("failed to access credential store at {location}: {source}")]
    Io {
        location: String,
        #[source]
        source: io::Error,
    },

    #[error("malformed credential store at {location}: {source}")]
    Malformed {
        location: String,
        #[source]
        source: serde_json::Error,
    },

    #[error(
        "credential store at {location} was written by a newer version of JP (schema version \
         {found}, this build supports up to {STORE_VERSION})"
    )]
    NewerVersion { location: String, found: u32 },

    #[error(transparent)]
    Lock(#[from] LockError),

    #[error("{0}")]
    Rejected(String),
}

/// Where the serialized credential store document is kept.
///
/// A backend reads and writes the document as one opaque unit; the encoding,
/// schema versioning, and locking are [`CredentialStore`] concerns, shared by
/// every backend.
///
/// Implementations must uphold two contracts:
///
/// - [`persist`] is all-or-nothing: a crash mid-persist leaves either the
///   previous or the new document, never a mix.
/// - The document contains secrets and is protected at rest per platform idiom
///   (owner-only file permissions, Keychain ACLs, ...).
///
/// [`persist`]: Self::persist
pub trait CredentialBackend: Send + Sync + Debug {
    /// Human-readable location for error messages.
    fn describe(&self) -> String;

    /// Read the serialized store document.
    ///
    /// `None` when no store exists yet.
    fn load(&self) -> Result<Option<String>, StoreError>;

    /// Persist the serialized document atomically.
    fn persist(&self, document: &str) -> Result<(), StoreError>;
}

/// File-backed credential storage.
///
/// The document is written to a temp file (created with `0600` on Unix) and
/// atomically renamed over the store; Windows relies on the default
/// user-profile ACLs.
#[derive(Debug, Clone)]
pub struct FsCredentialBackend {
    path: Utf8PathBuf,
}

impl FsCredentialBackend {
    /// A backend storing the document at `path`.
    pub fn new(path: impl Into<Utf8PathBuf>) -> Self {
        Self { path: path.into() }
    }

    fn io_err(&self, source: io::Error) -> StoreError {
        StoreError::Io {
            location: self.describe(),
            source,
        }
    }
}

impl CredentialBackend for FsCredentialBackend {
    fn describe(&self) -> String {
        self.path.to_string()
    }

    fn load(&self) -> Result<Option<String>, StoreError> {
        match std::fs::read_to_string(&self.path) {
            Ok(content) => Ok(Some(content)),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(source) => Err(self.io_err(source)),
        }
    }

    fn persist(&self, document: &str) -> Result<(), StoreError> {
        let dir = self.path.parent().unwrap_or(Utf8Path::new("."));
        std::fs::create_dir_all(dir).map_err(|e| self.io_err(e))?;

        let mut temp = NamedUtf8TempFile::new_in(dir).map_err(|e| self.io_err(e))?;
        restrict_permissions(temp.path()).map_err(|e| self.io_err(e))?;

        temp.write_all(document.as_bytes())
            .map_err(|e| self.io_err(e))?;
        temp.as_file().sync_all().map_err(|e| self.io_err(e))?;
        temp.persist(&self.path)
            .map_err(|error| self.io_err(error.error))?;

        Ok(())
    }
}

/// In-memory credential storage, for tests.
#[derive(Debug, Clone, Default)]
pub struct InMemoryCredentialBackend {
    document: Arc<Mutex<Option<String>>>,
}

impl InMemoryCredentialBackend {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }
}

impl CredentialBackend for InMemoryCredentialBackend {
    fn describe(&self) -> String {
        "<memory>".to_owned()
    }

    fn load(&self) -> Result<Option<String>, StoreError> {
        Ok(self.document.lock().expect("poisoned").clone())
    }

    fn persist(&self, document: &str) -> Result<(), StoreError> {
        *self.document.lock().expect("poisoned") = Some(document.to_owned());
        Ok(())
    }
}

/// Restrict `path` to owner read/write (`0600`).
///
/// No-op on non-Unix platforms, which fall back to the platform's default
/// user-profile ACLs.
fn restrict_permissions(path: &Utf8Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    }
    #[cfg(not(unix))]
    let _ = path;

    Ok(())
}

/// Handle to the credential store: a backend plus the shared semantics.
#[derive(Debug, Clone)]
pub struct CredentialStore {
    backend: Arc<dyn CredentialBackend>,
    locker: Arc<dyn ResourceLocker>,
}

impl CredentialStore {
    /// The file-backed store at its default, user-global location.
    pub fn file_default() -> Result<Self, StoreError> {
        let dir = jp_config::fs::user_data_dir().ok_or(StoreError::MissingDataDir)?;

        Ok(Self::new(
            Arc::new(FsCredentialBackend::new(dir.join(STORE_FILENAME))),
            // The mutation lock is file-based regardless of backend; it
            // stays alongside the store in the user data directory.
            Arc::new(FsResourceLocker::new(dir)),
        ))
    }

    /// A store over an explicit backend and locker.
    pub fn new(backend: Arc<dyn CredentialBackend>, locker: Arc<dyn ResourceLocker>) -> Self {
        Self { backend, locker }
    }

    /// Read the store into a snapshot.
    ///
    /// A missing document is an empty store, not an error.
    pub fn load(&self) -> Result<StoreDocument, StoreError> {
        let Some(content) = self.backend.load()? else {
            return Ok(StoreDocument::empty());
        };

        decode(&content, &self.backend.describe())
    }

    /// Mutate the store under the cross-process lock.
    ///
    /// The document is re-read after the lock is acquired, so `f` always
    /// operates on the latest persisted state; a process holding a stale
    /// snapshot must recheck its preconditions inside `f`.
    /// An error from `f` leaves the store untouched.
    pub fn mutate<T>(
        &self,
        f: impl FnOnce(&mut StoreDocument) -> Result<T, StoreError>,
    ) -> Result<T, StoreError> {
        let _guard = self.locker.lock(LOCK_RESOURCE, None)?;

        let mut document = self.load()?;
        let value = f(&mut document)?;

        let content =
            serde_json::to_string_pretty(&document).map_err(|source| StoreError::Malformed {
                location: self.backend.describe(),
                source,
            })?;
        self.backend.persist(&content)?;

        Ok(value)
    }
}

impl CredentialStore {
    /// Record a quota cooldown against a stored profile.
    ///
    /// `scope` is the whole account ([`SCOPE_ACCOUNT`]) or a model family;
    /// resolution skips the profile only for models the scope matches.
    /// A later expiry never shortens an existing cooldown for the same scope,
    /// so a concurrent process that saw a longer window wins.
    ///
    /// Returns whether the profile was found; a chain entry with no stored
    /// profile (`api_key`) has nothing to record against.
    ///
    /// # Errors
    ///
    /// Returns an error when the store cannot be read or written.
    pub fn record_cooldown(
        &self,
        category: &str,
        provider: &str,
        profile: &str,
        scope: &str,
        until: DateTime<Utc>,
    ) -> Result<bool, StoreError> {
        self.update_profile(category, provider, profile, |credential| {
            let entry = credential
                .cooldowns
                .entry(scope.to_owned())
                .or_insert(until);
            *entry = (*entry).max(until);
        })
    }

    /// Mark a stored profile as needing a fresh login.
    ///
    /// Returns whether the profile was found.
    ///
    /// # Errors
    ///
    /// Returns an error when the store cannot be read or written.
    pub fn mark_needs_relogin(
        &self,
        category: &str,
        provider: &str,
        profile: &str,
    ) -> Result<bool, StoreError> {
        self.update_profile(category, provider, profile, |credential| {
            credential.needs_relogin = true;
        })
    }

    /// Apply `f` to a stored profile under the store lock.
    ///
    /// Returns whether the profile was found.
    fn update_profile(
        &self,
        category: &str,
        provider: &str,
        profile: &str,
        f: impl FnOnce(&mut StoredCredential),
    ) -> Result<bool, StoreError> {
        self.mutate(|document| {
            let Some(credential) = document.profile_mut(category, provider, profile) else {
                return Ok(false);
            };

            f(credential);
            Ok(true)
        })
    }
}

/// Parse and version-check a serialized store document.
fn decode(content: &str, location: &str) -> Result<StoreDocument, StoreError> {
    let document: StoreDocument =
        serde_json::from_str(content).map_err(|source| StoreError::Malformed {
            location: location.to_owned(),
            source,
        })?;

    if document.version > STORE_VERSION {
        return Err(StoreError::NewerVersion {
            location: location.to_owned(),
            found: document.version,
        });
    }

    Ok(document)
}

/// The store document: a schema version plus credentials nested by category,
/// provider, and profile name.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StoreDocument {
    version: u32,
    #[serde(default)]
    credentials: BTreeMap<String, BTreeMap<String, BTreeMap<String, StoredCredential>>>,
}

impl Default for StoreDocument {
    fn default() -> Self {
        Self::empty()
    }
}

impl StoreDocument {
    fn empty() -> Self {
        Self {
            version: STORE_VERSION,
            credentials: BTreeMap::new(),
        }
    }

    /// The profiles stored for a category/provider pair.
    #[must_use]
    pub fn profiles(
        &self,
        category: &str,
        provider: &str,
    ) -> Option<&BTreeMap<String, StoredCredential>> {
        self.credentials.get(category)?.get(provider)
    }

    /// Mutable access to a stored profile.
    pub fn profile_mut(
        &mut self,
        category: &str,
        provider: &str,
        profile: &str,
    ) -> Option<&mut StoredCredential> {
        self.credentials
            .get_mut(category)?
            .get_mut(provider)?
            .get_mut(profile)
    }

    /// Insert or replace a profile.
    pub fn insert_profile(
        &mut self,
        category: &str,
        provider: &str,
        profile: &str,
        credential: StoredCredential,
    ) {
        self.credentials
            .entry(category.to_owned())
            .or_default()
            .entry(provider.to_owned())
            .or_default()
            .insert(profile.to_owned(), credential);
    }

    /// Remove a profile, pruning empty parent maps.
    ///
    /// Returns the removed credential, or `None` when no such profile was
    /// stored.
    pub fn remove_profile(
        &mut self,
        category: &str,
        provider: &str,
        profile: &str,
    ) -> Option<StoredCredential> {
        let providers = self.credentials.get_mut(category)?;
        let profiles = providers.get_mut(provider)?;
        let removed = profiles.remove(profile);

        if profiles.is_empty() {
            providers.remove(provider);
        }
        if providers.is_empty() {
            self.credentials.remove(category);
        }

        removed
    }

    /// Iterate all stored profiles as `(category, provider, profile,
    /// credential)`.
    pub fn iter(&self) -> impl Iterator<Item = (&str, &str, &str, &StoredCredential)> {
        self.credentials.iter().flat_map(|(category, providers)| {
            providers.iter().flat_map(move |(provider, profiles)| {
                profiles.iter().map(move |(profile, credential)| {
                    (
                        category.as_str(),
                        provider.as_str(),
                        profile.as_str(),
                        credential,
                    )
                })
            })
        })
    }
}

/// A stored credential: the secret material plus account identity, quota
/// cooldowns, and re-login state.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StoredCredential {
    /// The secret material, tagged by mechanism.
    #[serde(flatten)]
    pub secret: CredentialSecret,

    /// The account UUID this credential belongs to.
    ///
    /// `None` marks the profile as unverified: identity recovery failed at
    /// login and duplicate-account detection is skipped for it.
    pub account_id: Option<String>,

    /// The account email, when identity recovery reported one.
    pub email: Option<String>,

    /// Active quota cooldowns, keyed by scope: the whole account (`"account"`)
    /// or a model family (`"opus"`, `"sonnet"`).
    /// Values are the instant the cooldown expires.
    #[serde(default)]
    pub cooldowns: BTreeMap<String, DateTime<Utc>>,

    /// Whether the credential was rejected by the provider and needs a fresh
    /// `jp provider auth login`.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub needs_relogin: bool,
}

impl StoredCredential {
    /// A cooldown scope covering `model` that has not yet expired.
    #[must_use]
    pub fn active_cooldown(
        &self,
        model: &str,
        now: DateTime<Utc>,
    ) -> Option<(&str, DateTime<Utc>)> {
        self.cooldowns
            .iter()
            .filter(|(_, expires)| **expires > now)
            .find(|(scope, _)| scope_covers(scope, model))
            .map(|(scope, expires)| (scope.as_str(), *expires))
    }
}

/// Whether a recorded cooldown scope covers `model`.
///
/// [`SCOPE_ACCOUNT`] and the account-wide usage windows (`five_hour`,
/// `seven_day`) cover every model.
/// A window naming a model family covers only that family, whether it is
/// recorded as the provider reports it (`seven_day_opus`) or as the bare family
/// name (`opus`).
///
/// An unrecognized scope covers only models whose name contains it, so a window
/// this build does not know about cannot silently take a whole account out of
/// use.
fn scope_covers(scope: &str, model: &str) -> bool {
    match scope {
        SCOPE_ACCOUNT | "five_hour" | "seven_day" => true,
        scope => {
            let family = scope
                .strip_prefix("seven_day_")
                .or_else(|| scope.strip_prefix("five_hour_"))
                .unwrap_or(scope);

            model.contains(family)
        }
    }
}

/// The secret material of a credential, tagged by mechanism.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum CredentialSecret {
    /// A refreshable OAuth token pair.
    Oauth {
        access_token: String,
        refresh_token: String,
        expires_at: DateTime<Utc>,
    },

    /// A static bearer token (`claude setup-token` output) with no refresh flow
    /// and no expiry JP can inspect.
    Token { token: String },
}

impl CredentialSecret {
    /// A short label for `jp provider auth list`.
    #[must_use]
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Oauth { .. } => "oauth",
            Self::Token { .. } => "token",
        }
    }
}

#[cfg(test)]
#[path = "store_tests.rs"]
mod tests;
