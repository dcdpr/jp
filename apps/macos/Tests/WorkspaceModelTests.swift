import Foundation
import Testing

@testable import JP

/// The conversation list's state machine.
///
/// Each load has to land in exactly one state, because the reason this is one
/// value rather than several properties is that several observed mutations in a
/// row make the list reload partway through its own update.
///
/// Nested in `WorkspaceSuite` because each test points `JP_USER_DATA_DIR` at its
/// own directory, and that variable belongs to the whole process.
extension WorkspaceSuite {
    @MainActor
    @Suite("WorkspaceModel")
    struct WorkspaceModelTests {

        /// Before anything is opened, the sidebar explains how to open something.
        @Test("starts by pointing at the Open menu item")
        func startsUnopened() {
            let model = WorkspaceModel()

            guard case .unavailable(let title, _) = model.state else {
                Issue.record("expected an unavailable state, got \(model.state)")
                return
            }
            #expect(title == "No Workspace")
        }

        /// An empty workspace is not a failure, and says so differently from one.
        @Test("reports an empty workspace as having no conversations")
        func reportsAnEmptyWorkspace() async throws {
            let sandbox = try makeSandbox()
            defer { removeSandbox(sandbox) }
            let model = WorkspaceModel()

            await model.open(try makeWorkspace(in: sandbox))

            guard case .unavailable(let title, _) = model.state else {
                Issue.record("expected an unavailable state, got \(model.state)")
                return
            }
            #expect(title == "No Conversations")
        }

        @Test("reports a directory that is not a workspace")
        func reportsANonWorkspace() async throws {
            let sandbox = try makeSandbox()
            defer { removeSandbox(sandbox) }
            let model = WorkspaceModel()

            await model.open(sandbox.appendingPathComponent("nowhere").path)

            guard case .unavailable(let title, let detail) = model.state else {
                Issue.record("expected an unavailable state, got \(model.state)")
                return
            }
            #expect(title == "Could Not Open Workspace")
            #expect(detail.hasPrefix("No workspace found"))
        }

        /// Reading events needs a workspace, and asking before one is open is a
        /// programming mistake worth a message rather than a crash.
        @Test("refuses to read events with no workspace open")
        func refusesEventsWithoutAWorkspace() async {
            let model = WorkspaceModel()

            let result = await model.events(for: "17251488000")

            switch result {
            case .success:
                Issue.record("expected reading events with no workspace open to fail")
            case .failure(let error):
                #expect(error.message == "No workspace is open.")
            }
        }

        /// The path is recorded before the read, so a failed open still leaves the
        /// model pointing at what was attempted.
        @Test("records the path it was asked to open")
        func recordsThePath() async throws {
            let sandbox = try makeSandbox()
            defer { removeSandbox(sandbox) }
            let model = WorkspaceModel()
            let path = try makeWorkspace(in: sandbox)

            await model.open(path)

            #expect(model.path == path)
        }

        /// Opening a second workspace replaces the first rather than merging them.
        @Test("replaces the open workspace")
        func replacesTheOpenWorkspace() async throws {
            let sandbox = try makeSandbox()
            defer { removeSandbox(sandbox) }
            let model = WorkspaceModel()
            let first = try makeWorkspace(in: sandbox, named: "first")
            let second = try makeWorkspace(in: sandbox, named: "second")

            await model.open(first)
            await model.open(second)

            #expect(model.path == second)
        }
    }
}
