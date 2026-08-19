//! The action vocabulary, as data.
//!
//! A step is a single-key JSON object naming what to do:
//!
//! ```json
//! {"select": {"identifier": "sidebar.row.jp-c12345"}}
//! ```
//!
//! Steps and the harness that runs them are independent axes.
//! A list is read here and walked by [`drive`] live; a harness that runs the
//! same list under a profiler needs no second copy of the vocabulary.
//! A list is also replayable, diffable, and transcribable into an `XCUITest`
//! case once the flow it describes is understood.
//!
//! Payload validation belongs to `jpdrive`, which owns the schema and reports
//! against it.
//! What is checked here is the shape a list has to have — an array of
//! single-key objects naming a verb something can run — and it is checked for
//! the whole list before the first step runs, because a list abandoned halfway
//! leaves the app in a state nobody asked for.
//!
//! [`drive`]: super::drive

use serde_json::Value;

use crate::Error;

/// Verbs `jpdrive act` runs.
///
/// Alphabetical, because the list is quoted back in errors.
const DRIVER_VERBS: [&str; 9] = [
    "click", "drag", "menu", "perform", "press", "resize", "select", "type", "wait_for",
];

/// The verb the harness answers itself, by reading rather than acting.
pub(crate) const SNAPSHOT: &str = "snapshot";

/// Verbs that synthesize input, and so borrow what the person at the keyboard
/// is using.
///
/// Mouse events go to whatever is on top at a coordinate, and the ordering
/// between applications follows activation — so any of these has to bring the
/// app forward, and moves the pointer to do it.
/// The others reach their target through the accessibility tree and disturb
/// nothing.
const POINTER_VERBS: [&str; 3] = ["click", "drag", "menu"];

/// Verbs a caller reaches for that the vocabulary deliberately lacks.
///
/// Waiting a fixed duration and assuming the work finished is a guess.
/// If a wait cannot be written as a predicate on the tree, the app is missing
/// an identifier, and adding one is the fix.
const SLEEP_VERBS: [&str; 4] = ["sleep", "delay", "pause", "wait"];

/// One thing to do, and what to do it to.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Step {
    verb: String,
    payload: Value,
}

impl Step {
    /// Whether the harness answers this step itself, rather than the driver.
    pub(crate) fn is_snapshot(&self) -> bool {
        self.verb == SNAPSHOT
    }

    /// Whether running this step takes focus and moves the pointer.
    ///
    /// Read from the verb rather than reported by the driver, because the
    /// driver runs one step per process and the decision is about the whole
    /// run.
    pub(crate) fn perturbs_ambient_state(&self) -> bool {
        POINTER_VERBS.contains(&self.verb.as_str())
    }

    /// The step as `jpdrive act --json` reads it.
    pub(crate) fn json(&self) -> String {
        Value::Object(
            [(self.verb.clone(), self.payload.clone())]
                .into_iter()
                .collect(),
        )
        .to_string()
    }

    /// One line naming the step, for a report.
    ///
    /// The verb reads first, so a numbered list of steps scans as a list of
    /// verbs rather than a wall of JSON.
    pub(crate) fn label(&self) -> String {
        match &self.payload {
            Value::Null => self.verb.clone(),
            Value::Object(map) if map.is_empty() => self.verb.clone(),
            payload => format!("{} {payload}", self.verb),
        }
    }
}

/// Read a step list.
///
/// Errors name the step by its position in the list, counting from one, so a
/// caller can find it in what they wrote.
pub(crate) fn parse(value: &Value) -> Result<Vec<Step>, Error> {
    // An array argument sometimes arrives as a JSON string holding the array.
    if let Value::String(raw) = value
        && let Ok(inner) = serde_json::from_str::<Value>(raw)
    {
        return parse(&inner);
    }

    let Some(items) = value.as_array() else {
        return Err(format!(
            "`steps` is a JSON array of steps, but a {} was given. {VOCABULARY}",
            kind(value)
        )
        .into());
    };

    if items.is_empty() {
        return Err(format!("`steps` is empty, so there is nothing to do. {VOCABULARY}").into());
    }

    items
        .iter()
        .enumerate()
        .map(|(index, item)| step(index + 1, item))
        .collect()
}

/// The vocabulary, quoted in every error so a caller can correct in place.
const VOCABULARY: &str = "A step is a single-key object naming one of: click, drag, menu, \
                          perform, press, resize, select, snapshot, type, wait_for. For example \
                          `{\"select\": {\"identifier\": \"sidebar.row.jp-c12345\"}}`.";

/// Read one step, `position` being its place in the list counting from one.
fn step(position: usize, value: &Value) -> Result<Step, Error> {
    let Some(map) = value.as_object() else {
        return Err(format!(
            "Step {position} is a {}, not an object. {VOCABULARY}",
            kind(value)
        )
        .into());
    };

    let mut entries = map.iter();
    let (Some((verb, payload)), None) = (entries.next(), entries.next()) else {
        return Err(format!(
            "Step {position} names {} verbs ({}). Each step does one thing, so split it into that \
             many steps. {VOCABULARY}",
            map.len(),
            map.keys().cloned().collect::<Vec<_>>().join(", ")
        )
        .into());
    };

    if verb != SNAPSHOT && !DRIVER_VERBS.contains(&verb.as_str()) {
        return Err(unknown_verb(position, verb).into());
    }

    Ok(Step {
        verb: verb.clone(),
        payload: payload.clone(),
    })
}

/// Why a verb is not one of the ones that exist.
fn unknown_verb(position: usize, verb: &str) -> String {
    if SLEEP_VERBS.contains(&verb) {
        return format!(
            "Step {position} names `{verb}`, and there is no step that waits a fixed duration: \
             waiting and assuming the work finished is a guess. Use `wait_for` against an \
             identifier the app publishes once the work is done. If there is no such identifier, \
             the app is missing one, and adding it is the fix."
        );
    }

    format!("Step {position} names an unknown verb `{verb}`. {VOCABULARY}")
}

/// What a value is, for an error that says what was given instead.
fn kind(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

#[cfg(test)]
#[path = "steps_tests.rs"]
mod tests;
