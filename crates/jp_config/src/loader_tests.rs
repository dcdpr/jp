use serde_json::json;
use test_log::test;

use super::*;
use crate::PartialAppConfig;

#[test]
fn test_loader_reset_deserialize() {
    let partial: PartialLoaderConfig = serde_json::from_value(json!({ "reset": "none" })).unwrap();
    assert_eq!(partial.reset, Some(LoaderReset::None));

    let partial: PartialLoaderConfig = serde_json::from_value(json!({})).unwrap();
    assert_eq!(partial.reset, None);
}

#[test]
fn test_loader_reset_rejects_unknown_target() {
    // Only `"none"` is defined by [RFD 038]; unknown targets fail loudly
    // instead of being misread, keeping the field open for future variants.
    let result = serde_json::from_value::<PartialLoaderConfig>(json!({ "reset": "workspace" }));
    assert!(result.is_err());
}

#[test]
fn test_fill_from_prefers_own_value() {
    let own = PartialLoaderConfig {
        reset: Some(LoaderReset::None),
    };
    let filled = own.fill_from(PartialLoaderConfig { reset: None });
    assert_eq!(filled.reset, Some(LoaderReset::None));

    let filled = PartialLoaderConfig { reset: None }.fill_from(PartialLoaderConfig {
        reset: Some(LoaderReset::None),
    });
    assert_eq!(filled.reset, Some(LoaderReset::None));
}

#[test]
fn test_app_config_delta_strips_loader() {
    // Loader metadata is interpreted at load time and never travels through
    // partial deltas ([RFD 038]): only its *effect* outlives loading, never
    // the field itself.
    let prev = PartialAppConfig::empty();
    let mut next = PartialAppConfig::empty();
    next.loader.reset = Some(LoaderReset::None);

    let delta = prev.delta(next);
    assert_eq!(delta.loader.reset, None);
}
