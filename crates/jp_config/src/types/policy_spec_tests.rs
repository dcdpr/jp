use serde::{Deserialize, Serialize};

use super::*;
use crate::conversation::compaction::{ReasoningMode, ToolCallsMode};

/// A stand-in for a policy that carries its own tag and extra fields, matching
/// the shape of `jp_conversation`'s `ToolCallPolicy`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "policy", rename_all = "snake_case")]
enum TaggedPolicy {
    Strip { request: bool, response: bool },
    Omit,
}

// ---------------------------------------------------------------------------
// Serialization shape
// ---------------------------------------------------------------------------

#[test]
fn bare_policy_serializes_without_a_wrapper() {
    // A spec with no options must keep the shape a plain policy already had,
    // so streams and configs that set no threshold are untouched.
    let spec = PolicySpec::new(ToolCallsMode::StripResponses);

    let json = serde_json::to_value(spec).unwrap();

    assert_eq!(json, serde_json::json!("strip-responses"));
}

#[test]
fn string_policy_with_option_is_promoted_to_a_table() {
    let spec = PolicySpec::over(ReasoningMode::Strip, ByteSize::from_bytes(16 * 1024));

    let json = serde_json::to_value(spec).unwrap();

    assert_eq!(
        json,
        serde_json::json!({ "policy": "strip", "over": "16KB" })
    );
}

#[test]
fn tagged_policy_with_option_gains_a_sibling_key() {
    // A policy that already serializes as a map keeps its own fields at the top
    // level rather than nesting under a wrapper.
    let spec = PolicySpec::over(
        TaggedPolicy::Strip {
            request: false,
            response: true,
        },
        ByteSize::from_bytes(1024 * 1024),
    );

    let json = serde_json::to_value(spec).unwrap();

    assert_eq!(
        json,
        serde_json::json!({
            "policy": "strip",
            "request": false,
            "response": true,
            "over": "1MB",
        })
    );
}

// ---------------------------------------------------------------------------
// Deserialization
// ---------------------------------------------------------------------------

#[test]
fn reads_a_bare_string_policy() {
    let spec: PolicySpec<ToolCallsMode> =
        serde_json::from_value(serde_json::json!("omit")).unwrap();

    assert_eq!(spec, PolicySpec::new(ToolCallsMode::Omit));
}

#[test]
fn reads_a_promoted_string_policy() {
    let spec: PolicySpec<ReasoningMode> =
        serde_json::from_value(serde_json::json!({ "policy": "strip", "over": "16KB" })).unwrap();

    assert_eq!(
        spec,
        PolicySpec::over(ReasoningMode::Strip, ByteSize::from_bytes(16 * 1024))
    );
}

#[test]
fn reads_a_tagged_unit_variant_as_the_policy_itself() {
    // `{"policy": "omit"}` is both a valid tagged policy and shaped exactly
    // like a promoted string. The tagged reading is the correct one, so it must
    // win.
    let spec: PolicySpec<TaggedPolicy> =
        serde_json::from_value(serde_json::json!({ "policy": "omit" })).unwrap();

    assert_eq!(spec, PolicySpec::new(TaggedPolicy::Omit));
}

#[test]
fn reads_a_tagged_unit_variant_carrying_an_option() {
    let spec: PolicySpec<TaggedPolicy> =
        serde_json::from_value(serde_json::json!({ "policy": "omit", "over": "2MB" })).unwrap();

    assert_eq!(
        spec,
        PolicySpec::over(TaggedPolicy::Omit, ByteSize::from_bytes(2 * 1024 * 1024))
    );
}

#[test]
fn round_trips_with_and_without_an_option() {
    let cases = [
        PolicySpec::new(TaggedPolicy::Strip {
            request: true,
            response: true,
        }),
        PolicySpec::over(
            TaggedPolicy::Strip {
                request: true,
                response: false,
            },
            ByteSize::from_bytes(4096),
        ),
        PolicySpec::new(TaggedPolicy::Omit),
        PolicySpec::over(TaggedPolicy::Omit, ByteSize::from_bytes(512)),
    ];

    for spec in cases {
        let json = serde_json::to_value(spec).unwrap();
        let parsed: PolicySpec<TaggedPolicy> = serde_json::from_value(json).unwrap();
        assert_eq!(parsed, spec);
    }
}

#[test]
fn reports_the_policy_error_for_an_unreadable_value() {
    let err = serde_json::from_value::<PolicySpec<ToolCallsMode>>(serde_json::json!("nonsense"))
        .unwrap_err()
        .to_string();

    assert!(
        err.contains("unknown tool_calls mode"),
        "unexpected error: {err}"
    );
}

// ---------------------------------------------------------------------------
// String form (`--cfg` assignment and the inline DSL)
// ---------------------------------------------------------------------------

#[test]
fn parses_a_bare_policy_string() {
    let spec: PolicySpec<ToolCallsMode> = "strip-responses".parse().unwrap();

    assert_eq!(spec, PolicySpec::new(ToolCallsMode::StripResponses));
}

#[test]
fn parses_a_policy_string_with_an_over_option() {
    let spec: PolicySpec<ToolCallsMode> = "sres,over=1MB".parse().unwrap();

    assert_eq!(
        spec,
        PolicySpec::over(
            ToolCallsMode::StripResponses,
            ByteSize::from_bytes(1024 * 1024)
        )
    );
}

#[test]
fn rejects_an_unknown_option() {
    let err = "strip,under=1MB"
        .parse::<PolicySpec<ToolCallsMode>>()
        .unwrap_err()
        .to_string();

    assert_eq!(err, "unknown policy option `under`");
}

#[test]
fn rejects_an_option_without_a_value() {
    let err = "strip,over"
        .parse::<PolicySpec<ToolCallsMode>>()
        .unwrap_err()
        .to_string();

    assert_eq!(err, "invalid policy option `over`: expected `key=value`");
}

#[test]
fn displays_the_form_it_parses() {
    for input in ["strip", "strip-responses,over=1MB", "omit,over=512"] {
        let spec: PolicySpec<ToolCallsMode> = input.parse().unwrap();
        assert_eq!(spec.to_string(), input);
    }
}

// ---------------------------------------------------------------------------
// Threshold predicate
// ---------------------------------------------------------------------------

#[test]
fn covers_everything_without_a_threshold() {
    let spec = PolicySpec::new(ToolCallsMode::Strip);

    assert!(spec.covers(0));
    assert!(spec.covers(u64::MAX));
}

#[test]
fn threshold_is_strictly_exclusive() {
    let spec = PolicySpec::over(ToolCallsMode::Strip, ByteSize::from_bytes(1024));

    assert!(!spec.covers(1023));
    assert!(
        !spec.covers(1024),
        "a payload of exactly the threshold is left alone"
    );
    assert!(spec.covers(1025));
}
