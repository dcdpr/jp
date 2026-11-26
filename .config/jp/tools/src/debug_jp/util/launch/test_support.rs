//! A [`Launcher`] for tests, returning a canned [`LaunchResult`].

use crate::{
    Error,
    debug_jp::util::launch::{LaunchResult, LaunchSpec, Launcher, Timeouts},
};

/// A [`Launcher`] that returns a canned [`LaunchResult`] without touching any
/// real process.
pub(crate) struct MockLauncher {
    result: LaunchResult,
}

impl MockLauncher {
    pub(crate) fn returning(result: LaunchResult) -> Self {
        Self { result }
    }
}

impl Launcher for MockLauncher {
    fn run(
        &self,
        _spec: &LaunchSpec,
        _timeouts: Timeouts,
        on_spawn: &mut dyn FnMut(u32),
    ) -> Result<LaunchResult, Error> {
        on_spawn(0);
        Ok(self.result.clone())
    }
}
