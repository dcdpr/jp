import Testing
import XCTest

/// Runs a suite's tests against one launched app instead of one each.
///
/// Launching costs seconds and the work under test costs milliseconds, so a
/// suite that launches per test spends almost all of its time starting and
/// stopping the app. This launches once, hands the same instance to every test
/// in the suite, and terminates it when the suite finishes.
///
/// The trade is that tests share what the app remembers. A suite using this has
/// to leave the app as it found it, or order its tests so that what one leaves
/// behind is what the next one expects. A test that cannot work that way asks
/// for its own instance with ``AppUnderTest/launch(against:)`` and terminates
/// it itself.
///
/// Safe despite the shared mutable state because ``UISuite`` is serialized:
/// only one test runs at a time, and all of this is main-actor isolated.
struct SharedApp: SuiteTrait, TestScoping {
    /// The fixture the app is launched against.
    let fixture: @Sendable () throws -> WorkspaceFixture

    func provideScope(
        for test: Test,
        testCase: Test.Case?,
        performing function: () async throws -> Void
    ) async throws {
        let fixture = try fixture()
        let driven = await AppUnderTest.launch(against: fixture)
        await SharedAppBox.shared.set(driven, fixture: fixture)

        // Torn down on both paths rather than in a `defer`, so terminating is
        // awaited: a `defer` would have to spawn a task to reach the main actor,
        // and a fire-and-forget task can lose the race with the process exiting
        // — leaving the app on the developer's screen.
        do {
            try await function()
        } catch {
            await Self.teardown(driven, fixture)
            throw error
        }

        await Self.teardown(driven, fixture)
    }

    @MainActor
    private static func teardown(_ driven: AppUnderTest, _ fixture: WorkspaceFixture) {
        SharedAppBox.shared.clear()
        driven.terminate()
        fixture.remove()
    }
}

extension Trait where Self == SharedApp {
    /// One app for the whole suite, launched against `fixture`.
    static func sharedApp(
        _ fixture: @escaping @Sendable () throws -> WorkspaceFixture
    ) -> Self {
        SharedApp(fixture: fixture)
    }
}

/// Where the suite's app is kept between the trait that launches it and the
/// tests that use it.
///
/// A global rather than a property on the suite, because swift-testing builds a
/// fresh suite value for every test: anything stored on the suite is gone by
/// the time the next test runs.
@MainActor
final class SharedAppBox {
    static let shared = SharedAppBox()

    private var driven: AppUnderTest?
    private var fixture: WorkspaceFixture?

    private init() {}

    func set(_ driven: AppUnderTest, fixture: WorkspaceFixture) {
        self.driven = driven
        self.fixture = fixture
    }

    func clear() {
        driven = nil
        fixture = nil
    }

    /// The app the suite is running against.
    ///
    /// Traps rather than returning an optional every caller has to unwrap: a
    /// test reaching for this without ``SharedApp`` on its suite is a mistake in
    /// the test, and every assertion after it would be meaningless anyway.
    var app: AppUnderTest {
        guard let driven else {
            fatalError("no shared app: put `.sharedApp(...)` on the suite")
        }
        return driven
    }

    /// The workspace the suite's app was launched against.
    var workspace: WorkspaceFixture {
        guard let fixture else {
            fatalError("no shared app: put `.sharedApp(...)` on the suite")
        }
        return fixture
    }
}
