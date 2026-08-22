import Testing

/// The suite every UI test belongs to.
///
/// Serialized, and serialized *together*: `XCUIApplication` addresses the app
/// under test by bundle identifier, so two tests running side by side would
/// drive one process between them. Nesting is what puts sibling suites under
/// the same ordering — `.serialized` orders a suite's own tests and its nested
/// suites, while suites declared alongside each other still run in parallel.
///
/// The `extension UISuite` declarations in the sibling files are that nesting.
@Suite("UI", .serialized)
struct UISuite {}
