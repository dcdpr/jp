import Testing

@testable import DriveKit

@Suite("Act.waitFor")
struct WaitForTests {
    /// A container that produces the awaited element on its third read, and not
    /// before.
    ///
    /// The delay is what makes a wait test mean anything: against a tree that
    /// already holds the element, polling and not polling look identical.
    private func appearsOnThirdRead() -> FakeElement {
        let container = FakeElement(role: "AXScrollArea", identifier: "transcript.scroll")
        container.onRead = { element in
            guard element.reads == 3 else { return }
            element.children = [
                FakeElement(role: "AXGroup", identifier: "transcript.event.1")
            ]
        }
        return container
    }

    private func step(
        _ identifier: String,
        under: String? = nil,
        timeoutMs: Int? = nil,
        intervalMs: Int? = 1
    ) -> Step {
        return .waitFor(
            .init(
                identifier: identifier, under: under, timeoutMs: timeoutMs,
                intervalMs: intervalMs)
        )
    }

    @Test("an element already present is returned on the first attempt")
    func returnsImmediately() throws {
        let root = FakeElement.sidebar(rowCount: 2)

        // A zero timeout permits exactly one attempt, so a pass here cannot have
        // come from a retry.
        let result = try Act.run(step("sidebar.row.1", timeoutMs: 0), in: root)

        #expect(result.step == "wait_for")
        #expect(result.role == "AXUnknown")
        #expect(result.confirmed == true)
    }

    /// Half of a pair. This one proves the fixture genuinely withholds the element,
    /// so that the passing case below is evidence of retrying rather than of the
    /// element having been there all along.
    @Test("one attempt is not enough for an element that appears later")
    func oneAttemptIsNotEnough() throws {
        let container = appearsOnThirdRead()
        let root = FakeElement(role: "AXApplication", children: [container])

        let error = try #require(throws: DriveError.self) {
            try Act.run(step("transcript.event.1", timeoutMs: 0), in: root)
        }

        #expect(error.kind == .timeout)
    }

    @Test("polling finds an element that appears later")
    func findsAnElementThatAppearsLater() throws {
        let container = appearsOnThirdRead()
        let root = FakeElement(role: "AXApplication", children: [container])

        let result = try Act.run(step("transcript.event.1", timeoutMs: 2000), in: root)

        #expect(result.confirmed == true)
        #expect(result.role == "AXGroup")
        #expect(
            container.reads >= 3, "the element cannot have been found before its third read")
    }

    @Test("an element that never appears times out")
    func timesOut() throws {
        let root = FakeElement.sidebar(rowCount: 2)

        let error = try #require(throws: DriveError.self) {
            try Act.run(step("never.appears", timeoutMs: 20), in: root)
        }

        #expect(error.kind == .timeout)
        #expect(error.message.contains("never.appears"))
    }

    /// A single attempt eating the whole timeout is the failure mode that makes an
    /// unscoped wait useless, so the error says what to do about it.
    @Test("a timeout after one attempt suggests scoping")
    func suggestsScopingAfterOneAttempt() throws {
        let root = FakeElement.sidebar(rowCount: 2)

        let error = try #require(throws: DriveError.self) {
            try Act.run(step("never.appears", timeoutMs: 0), in: root)
        }

        #expect(error.hint?.contains("under") == true)
    }

    /// Waiting inside something that does not exist is a mistake in the script, not
    /// a condition that might come true.
    @Test("a missing container fails at once rather than being waited for")
    func missingContainerFailsFast() throws {
        let root = FakeElement.sidebar(rowCount: 2)

        let error = try #require(throws: DriveError.self) {
            try Act.run(step("anything", under: "no.such.container", timeoutMs: 5000), in: root)
        }

        #expect(error.kind == .identifierNotFound)
    }

    /// The reason `under` exists. Without it every attempt re-reads the whole
    /// application, and against this app's sidebar one attempt outlasts a typical
    /// timeout.
    @Test("scoping keeps polling off the rest of the tree")
    func scopingBoundsThePolling() throws {
        let sidebar = FakeElement.sidebar(rowCount: 20)
        let outline = try #require(sidebar.children.first)
        let container = FakeElement(role: "AXScrollArea", identifier: "transcript.scroll")
        let root = FakeElement(role: "AXApplication", children: [outline, container])

        let error = try #require(throws: DriveError.self) {
            try Act.run(
                step("transcript.event.1", under: "transcript.scroll", timeoutMs: 30),
                in: root
            )
        }
        #expect(error.kind == .timeout)

        // Read once while resolving the container, and never again. Repeated reads
        // here would mean each poll was walking the sidebar.
        #expect(outline.children.allSatisfy { $0.reads == 1 })
    }
}
