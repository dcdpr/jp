//! Reporting of the input items OpenAI withholds from the model.
//!
//! A response's `input_transformations` lists replayed items the API removed
//! before sampling, with the reason.
//! [`input_transformation_notices`] turns them into log lines and user-facing
//! [`Event::Notice`]s.

use openai_responses::types::response::InputTransformation;
use tracing::debug;

use crate::event::Event;

/// Notices for the replayed reasoning items the API withheld from the model.
///
/// Every withheld input item is logged.
/// Reasoning removed because the conversation before it changed, or because the
/// conversation moved to another model, produces one notice per reason: both
/// follow from a change the user or JP made.
/// Other reasons are logged only.
pub(super) fn input_transformation_notices(transformations: &[InputTransformation]) -> Vec<Event> {
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
