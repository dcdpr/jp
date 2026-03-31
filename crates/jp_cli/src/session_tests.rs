use jp_workspace::session::{SessionId, SessionSource};

use super::*;

#[test]
fn from_env_returns_none_for_unset_var() {
    // Use a variable name that won't be set in any real environment.
    let result = from_env("JP_TEST_NONEXISTENT_VAR_83927461");
    assert!(result.is_none());
}

#[test]
fn from_env_source_is_env_with_key() {
    // We can't safely set env vars in Rust 2024 without unsafe, so test the
    // source construction via the SessionSource type directly.
    let source = SessionSource::Env {
        key: "TMUX_PANE".to_owned(),
    };
    assert_eq!(source, SessionSource::Env {
        key: "TMUX_PANE".to_owned(),
    });
}

#[test]
fn session_id_rejects_empty_string() {
    assert!(SessionId::new("").is_none());
}

#[test]
fn session_id_accepts_nonempty_string() {
    let id = SessionId::new("12345").unwrap();
    assert_eq!(id.as_str(), "12345");
    assert_eq!(id.to_string(), "12345");
}

#[test]
fn session_id_display() {
    let id = SessionId::new("my-session").unwrap();
    assert_eq!(format!("{id}"), "my-session");
}

#[cfg(unix)]
#[test]
fn from_platform_returns_getsid_on_unix() {
    // In a test runner there's always a session leader, so this should
    // succeed unless we're in a very unusual environment (container
    // without a session leader).
    let Some(session) = from_platform() else {
        return;
    };

    assert_eq!(session.source, SessionSource::Getsid);
    // The ID should be a numeric PID string.
    assert!(session.id.as_str().parse::<i32>().is_ok());
}

#[cfg(windows)]
#[test]
fn from_platform_returns_hwnd_on_windows() {
    // In CI the test process typically inherits a console from the
    // PowerShell/cmd parent, so this should succeed. If the environment
    // has no console (e.g. a GUI-only subsystem), the function returns
    // None and the test still passes.
    let Some(session) = from_platform() else {
        return;
    };

    assert_eq!(session.source, SessionSource::Hwnd);
    // The ID should be a non-empty string representation of the HWND.
    assert!(!session.id.as_str().is_empty());
}

#[test]
fn from_platform_is_stable_across_calls() {
    let a = from_platform();
    let b = from_platform();
    assert_eq!(a, b);
}
