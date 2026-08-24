import ApplicationServices
import Testing

@testable import DriveKit

@Suite("Windows")
struct WindowsTests {
    @Test("reports each window's own facts")
    func reportsWindowFacts() {
        let main = FakeElement(role: "AXWindow", identifier: "workspace-AppWindow-1")
        main.attributes[kAXTitleAttribute] = "JP"
        main.attributes[kAXMainAttribute] = "1"
        main.attributes[kAXMinimizedAttribute] = "0"
        main.attributes["AXFrame"] = "0.0,0.0 1200.0x800.0"

        let other = FakeElement(role: "AXWindow", identifier: "workspace-AppWindow-2")
        other.attributes[kAXMainAttribute] = "0"
        other.attributes[kAXMinimizedAttribute] = "1"

        let app = FakeElement(role: "AXApplication")
        app.related[kAXWindowsAttribute] = [main, other]

        #expect(
            Windows.list(of: app) == [
                WindowSummary(
                    identifier: "workspace-AppWindow-1",
                    title: "JP",
                    main: true,
                    minimized: false,
                    frame: "0.0,0.0 1200.0x800.0"
                ),
                WindowSummary(
                    identifier: "workspace-AppWindow-2",
                    title: nil,
                    main: false,
                    minimized: true,
                    frame: nil
                ),
            ]
        )
    }

    /// A running application with every window closed is a normal state, not an
    /// error.
    @Test("an application with no windows reports an empty list")
    func noWindows() {
        #expect(Windows.list(of: FakeElement(role: "AXApplication")).isEmpty)
    }

    /// Windows are read from the application's own attribute, not found by walking
    /// into the hierarchy. A listing that descended would pick up sheets and popups
    /// as if they were windows.
    @Test("windows are read from the attribute, not from the children")
    func doesNotWalkChildren() {
        let child = FakeElement(role: "AXWindow", identifier: "not.a.window")
        let app = FakeElement(role: "AXApplication", children: [child])

        #expect(Windows.list(of: app).isEmpty)
        #expect(child.reads == 0)
    }
}
