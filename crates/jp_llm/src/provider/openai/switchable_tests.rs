//! Which failures are worth trying another credential for.

use super::is_switchable;
use crate::error::{StreamError, StreamErrorKind};

#[test]
fn test_a_refused_or_spent_credential_is_switchable() {
    for error in [
        StreamError::auth_rejected("token revoked"),
        StreamError::new(StreamErrorKind::SubscriptionExhausted, "limit reached"),
        StreamError::new(StreamErrorKind::InsufficientQuota, "out of credits"),
    ] {
        assert!(is_switchable(&error), "kind: {:?}", error.kind);
    }
}

#[test]
fn test_a_deterministic_rejection_is_not_switchable() {
    // Each of these is answered the same way by every credential in the
    // chain, so advancing would burn each profile in turn on a request that
    // cannot succeed under any of them.
    for error in [
        StreamError::other("System messages are not allowed (HTTP 400)"),
        StreamError::context_window_exceeded("prompt too long"),
        StreamError::new(StreamErrorKind::OutputLimit, "runaway output"),
    ] {
        assert!(!is_switchable(&error), "kind: {:?}", error.kind);
    }
}

#[test]
fn test_a_transient_failure_is_not_switchable() {
    // The retry layer above handles these in place; switching credentials
    // would lose the backoff and pointlessly re-route a request that is
    // expected to work on the same one.
    for error in [
        StreamError::transient("upstream overloaded"),
        StreamError::rate_limit(None),
        StreamError::new(StreamErrorKind::Timeout, "timed out"),
        StreamError::new(StreamErrorKind::Connect, "connection refused"),
    ] {
        assert!(!is_switchable(&error), "kind: {:?}", error.kind);
    }
}
