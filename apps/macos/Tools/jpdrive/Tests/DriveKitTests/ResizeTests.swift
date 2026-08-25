import ApplicationServices
import Foundation
import Testing

@testable import DriveKit

/// Tests for the `resize` step.
///
/// A window is the only thing that accepts a write to `AXSize`, and resizing is
/// the one interaction a driver cannot reach any other way: a drag of a window's
/// edge has to be synthesized, and a synthesized drag needs the window frontmost.
@Suite("Resize")
struct ResizeTests {
    /// A window that accepts a size, starting at `size`.
    private func window(_ size: CGSize, settable: Bool = true) -> FakeElement {
        let window = FakeElement(
            role: "AXWindow",
            identifier: "the-window",
            settable: settable ? [kAXSizeAttribute] : []
        )
        window.sizes[kAXSizeAttribute] = size
        return window
    }

    private func step(width: Double, height: Double) -> Step {
        .resize(Step.SizeTarget(identifier: "the-window", width: width, height: height))
    }

    @Test("writes the size it was asked for")
    func writesTheSize() throws {
        let window = self.window(CGSize(width: 900, height: 450))

        let result = try Act.run(step(width: 1400, height: 900), in: window)

        #expect(window.sizes[kAXSizeAttribute] == CGSize(width: 1400, height: 900))
        #expect(result.step == "resize")
        #expect(result.role == "AXWindow")
        #expect(result.confirmed == true)
        #expect(result.size == "1400x900")
    }

    /// A window clamps to its own minimum and maximum, so the write succeeds and
    /// the window lands somewhere else. Reporting what it reached is the whole
    /// reason the step reads the size back instead of echoing the request.
    @Test("reports the size it reached when the window clamps the request")
    func reportsAClampedSize() throws {
        let window = self.window(CGSize(width: 900, height: 450))
        window.ignoresWrites = true

        let result = try Act.run(step(width: 200, height: 100), in: window)

        #expect(result.confirmed == false)
        #expect(result.size == "900x450")
    }

    /// Most elements inside a window do not accept a size, and a step that asked
    /// anyway would report success having changed nothing.
    @Test("refuses an element that does not accept a size")
    func refusesAnUnsizableElement() {
        let element = window(CGSize(width: 900, height: 450), settable: false)

        #expect(throws: DriveError.self) {
            try Act.run(step(width: 1400, height: 900), in: element)
        }
    }

    @Test("reports an identifier that is not in the tree")
    func reportsAMissingIdentifier() {
        let other = FakeElement(role: "AXWindow", identifier: "something-else")

        #expect(throws: DriveError.self) {
            try Act.run(step(width: 1400, height: 900), in: other)
        }
    }

    @Test("surfaces a write the accessibility API refused")
    func surfacesARefusedWrite() {
        let window = self.window(CGSize(width: 900, height: 450))
        window.writeStatus = .cannotComplete

        #expect(throws: DriveError.self) {
            try Act.run(step(width: 1400, height: 900), in: window)
        }
    }

    /// The step arrives as JSON from the driver's caller, so the spelling of its
    /// keys is part of the contract.
    @Test("decodes the step a caller writes")
    func decodesTheStep() throws {
        let json = #"{"resize":{"identifier":"w","width":1400,"height":900}}"#

        let decoded = try JSONDecoder().decode(Step.self, from: Data(json.utf8))

        guard case .resize(let target) = decoded else {
            Issue.record("expected a resize step, got \(decoded)")
            return
        }

        #expect(target.identifier == "w")
        #expect(target.width == 1400)
        #expect(target.height == 900)
    }
}
