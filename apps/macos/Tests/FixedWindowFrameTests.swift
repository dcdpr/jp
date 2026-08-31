import Testing

@testable import JP

/// The window frame a UI test pins the app to.
@Suite("FixedWindowFrame")
struct FixedWindowFrameTests {
    /// A UI test bundle drives the app from another process and cannot import
    /// it, so `AppUnderTest` spells this key out as a literal. Renaming the app's
    /// constant without changing that literal would leave every launch inheriting
    /// the previous run's window size again — silently, because an unset variable
    /// means "behave normally".
    ///
    /// This is the only thing holding the two spellings together.
    @Test("is read from the variable the UI tests set")
    func keyMatchesTheOneUITestsSet() {
        #expect(FixedWindowFrame.environmentKey == "JP_WINDOW_FRAME")
    }
}
