use camino::Utf8Path;

use crate::util::runner::{ProcessOutput, ProcessRunner};

const MAX_RETRIES: u32 = 5;
const BASE_DELAY_MS: u64 = 50;

/// Apply a patch to the git index with retry logic for lock contention.
///
/// Git's `index.lock` prevents concurrent index modifications. When multiple
/// tool calls run in parallel, they race for this lock. This function retries
/// with exponential backoff when it detects lock contention.
pub fn apply_patch_to_index<R: ProcessRunner>(
    patch: &str,
    root: &Utf8Path,
    runner: &R,
) -> Result<(), String> {
    let mut last_err = String::new();

    for attempt in 0..=MAX_RETRIES {
        if attempt > 0 {
            let delay = BASE_DELAY_MS * 2u64.pow(attempt - 1);
            std::thread::sleep(std::time::Duration::from_millis(delay));
        }

        let ProcessOutput { stderr, status, .. } = runner
            .run_with_env_and_stdin(
                "git",
                &["apply", "--cached", "--unidiff-zero", "-"],
                root,
                &[],
                Some(patch),
            )
            .map_err(|e| format!("Failed to run git apply: {e}"))?;

        if status.is_success() {
            return Ok(());
        }

        if !is_lock_contention(&stderr) {
            return Err(format!("Failed to apply patch: {stderr}"));
        }

        last_err = stderr;
    }

    Err(format!(
        "Failed to apply patch after {MAX_RETRIES} retries (index.lock contention): {last_err}"
    ))
}

fn is_lock_contention(stderr: &str) -> bool {
    stderr.contains("index.lock") && stderr.contains("File exists")
}

/// Build a full git patch string from a path and hunk content.
pub fn build_patch(path: &str, hunks: &str) -> String {
    format!("diff --git a/{path} b/{path}\n--- a/{path}\n+++ b/{path}\n{hunks}")
}

#[cfg(test)]
#[path = "apply_tests.rs"]
mod tests;
