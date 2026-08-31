//! Test-only scoped mutation of process environment variables.
//!
//! Environment variables are process-global, so a test that sets one is writing
//! shared state that outlives it.
//! [`EnvVarGuard`] restores the previous value on drop, including when the test
//! panics part-way through.

/// Sets an environment variable for as long as the guard is held.
///
/// On drop the variable returns to the value it had when the guard was created,
/// or is removed if it had none.
/// Both the panicking and non-panicking exits restore it, which a bare
/// `set_var`/`remove_var` pair around the body of a test does not.
///
/// Every test holding one must be marked `#[serial(env_vars)]`: the guard makes
/// a test's own writes tidy, not concurrent tests' writes ordered.
pub(crate) struct EnvVarGuard {
    /// The variable being held.
    name: String,

    /// Its value before the guard was created, restored on drop.
    original_value: Option<String>,
}

impl EnvVarGuard {
    /// Set `name` to `value` until the guard drops.
    pub(crate) fn set(name: &str, value: &str) -> Self {
        let name = name.to_owned();
        let original_value = std::env::var(&name).ok();

        // SAFETY: writing the environment races with any thread reading it.
        // Callers are serialized on the `env_vars` group, and the test binary
        // spawns no thread that reads this variable.
        unsafe { std::env::set_var(&name, value) };

        Self {
            name,
            original_value,
        }
    }

    /// Unset `name` until the guard drops.
    ///
    /// For a test that has to run without a variable the developer's shell may
    /// have exported, rather than assuming it is absent.
    pub(crate) fn remove(name: &str) -> Self {
        let name = name.to_owned();
        let original_value = std::env::var(&name).ok();

        // SAFETY: as in `set`.
        unsafe { std::env::remove_var(&name) };

        Self {
            name,
            original_value,
        }
    }
}

impl Drop for EnvVarGuard {
    fn drop(&mut self) {
        // SAFETY: as in `set`.
        match self.original_value.as_ref() {
            Some(original) => unsafe { std::env::set_var(&self.name, original) },
            None => unsafe { std::env::remove_var(&self.name) },
        }
    }
}
