// Tests elsewhere in the crate build query-message fixtures from the real
// marker, so a fixture cannot drift from the parser.
pub(crate) use super::parser::CUT_MARKER;

#[path = "backend_tests.rs"]
mod backend_tests;

#[path = "draft_tests.rs"]
mod draft_tests;

#[path = "history_tests.rs"]
mod history_tests;
