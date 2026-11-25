//! Reporting of input items that OpenAI removes from a request.
//!
//! A response's `input_transformations` field lists replayed items that the API
//! removed before sampling, along with the removal reason.
//! [`input_transformation_notices`] logs each transformation and converts
//! user-actionable removals into [`Event::Notice`] events.
//!
//! The API lists a removed item again on every request that replays it, so a
//! removal is reported once per provider.

use openai_responses::types::response::InputTransformation;
use tracing::debug;

use crate::{event::Event, provider::notices::SeenNotices};

/// Emit notices for replayed reasoning items that the API removed.
///
/// Every withheld input item is logged at `debug`.
/// Reasoning items removed for `prefix_mismatch` (the conversation prefix
/// before them changed) or `model_mismatch` (the conversation switched to a
/// different or less capable model) emit one [`Event::Notice`] per reason;
/// other removal reasons are only logged.
///
/// A notice counts the removals `seen` does not hold yet, keyed on the item's
/// id, and records them.
/// A reason with no such removal emits no notice.
pub(super) fn input_transformation_notices(
    transformations: &[InputTransformation],
    seen: &SeenNotices,
) -> Vec<Event> {
    for transformation in transformations {
        debug!(
            id = transformation.id,
            action = transformation.action,
            reason = transformation.reason,
            kind = transformation.r#type,
            "OpenAI withheld an input item from the model."
        );
    }

    [
        ("prefix_mismatch", "the conversation before them changed"),
        (
            "model_mismatch",
            "the conversation moved to a different or less capable model",
        ),
    ]
    .into_iter()
    .filter_map(|(reason, cause)| {
        let count = transformations
            .iter()
            .filter(|t| t.action == "removed" && t.r#type == "reasoning" && t.reason == reason)
            .filter(|t| seen.first(&format!("removed:{}", t.id)))
            .count();
        let items = if count == 1 { "item" } else { "items" };

        (count > 0).then(|| {
            Event::Notice(format!(
                "OpenAI dropped {count} reasoning {items} from earlier turns because {cause} \
                 ({reason}); the model may redo that reasoning."
            ))
        })
    })
    .collect()
}
