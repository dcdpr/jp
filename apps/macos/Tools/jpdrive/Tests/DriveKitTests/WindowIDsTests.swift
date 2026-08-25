import CoreGraphics
import Foundation
import Testing

@testable import DriveKit

@Suite("WindowIDs")
struct WindowIDsTests {
    /// One entry shaped the way the window server reports it: every number a
    /// `CFNumber`, with the bounds arriving as doubles.
    private func entry(
        id: Int,
        pid: Int,
        layer: Int = 0,
        title: String? = "JP",
        width: Double = 1200,
        height: Double = 800
    ) -> [String: Any] {
        var window: [String: Any] = [
            kCGWindowNumber as String: id,
            kCGWindowOwnerPID as String: pid,
            kCGWindowLayer as String: layer,
            kCGWindowBounds as String: ["X": 0.0, "Y": 0.0, "Width": width, "Height": height],
        ]
        window[kCGWindowName as String] = title
        return window
    }

    @Test("reports the window server's number and the window's size")
    func reportsIdentifiers() {
        let listed = [entry(id: 7412, pid: 4321)]

        #expect(
            WindowIDs.capturable(from: listed, pid: 4321) == [
                CaptureWindow(id: 7412, title: "JP", width: 1200, height: 800)
            ]
        )
    }

    /// Every application on the desktop is in the list, so a capture that took the
    /// first entry would photograph whatever happened to be frontmost.
    @Test("keeps only the windows the pid owns")
    func filtersByOwner() {
        let listed = [
            entry(id: 1, pid: 999, title: "Terminal"),
            entry(id: 2, pid: 4321),
            entry(id: 3, pid: 111, title: "Finder"),
        ]

        #expect(WindowIDs.capturable(from: listed, pid: 4321).map(\.id) == [2])
    }

    /// Tooltips, drag images and menu shadows are the app's too, and capturing one
    /// in place of the window is a wrong answer rather than a failure.
    @Test("keeps only windows on the normal layer")
    func dropsChrome() {
        let listed = [
            entry(id: 1, pid: 4321, layer: 25, title: "tooltip"),
            entry(id: 2, pid: 4321),
        ]

        #expect(WindowIDs.capturable(from: listed, pid: 4321).map(\.id) == [2])
    }

    /// A panel that has never been shown sits in the list at zero size, and
    /// capturing it produces an empty file.
    @Test("drops windows with no area")
    func dropsEmptyWindows() {
        let listed = [
            entry(id: 1, pid: 4321, width: 0, height: 0),
            entry(id: 2, pid: 4321),
        ]

        #expect(WindowIDs.capturable(from: listed, pid: 4321).map(\.id) == [2])
    }

    /// The window server withholds other applications' titles until the Screen
    /// Recording grant is given, which is the state a first run is in.
    @Test("a window with no readable title is still reported")
    func toleratesAMissingTitle() {
        let listed = [entry(id: 7412, pid: 4321, title: nil)]

        #expect(
            WindowIDs.capturable(from: listed, pid: 4321) == [
                CaptureWindow(id: 7412, title: nil, width: 1200, height: 800)
            ]
        )
    }

    /// What actually arrives is a `CFArray` of `CFDictionary`, so every number in it
    /// is an `NSNumber` once bridged, and a reader that only understood Swift's own
    /// numeric types would report an application with no windows at all.
    @Test("reads the numbers as the bridged types the window server hands over")
    func readsBridgedNumbers() {
        let listed: [[String: Any]] = [
            [
                kCGWindowNumber as String: NSNumber(value: 7412),
                kCGWindowOwnerPID as String: NSNumber(value: 4321),
                kCGWindowLayer as String: NSNumber(value: 0),
                kCGWindowName as String: "JP",
                kCGWindowBounds as String: [
                    "X": NSNumber(value: 0.0),
                    "Y": NSNumber(value: 0.0),
                    "Width": NSNumber(value: 1200.0),
                    "Height": NSNumber(value: 800.0),
                ],
            ]
        ]

        #expect(
            WindowIDs.capturable(from: listed, pid: 4321) == [
                CaptureWindow(id: 7412, title: "JP", width: 1200, height: 800)
            ]
        )
    }

    /// Front-to-back is the window server's own order, and it is the only thing
    /// telling a caller which of two windows to capture.
    @Test("preserves the order the window server reported")
    func preservesOrder() {
        let listed = [
            entry(id: 3, pid: 4321),
            entry(id: 1, pid: 4321),
            entry(id: 2, pid: 4321),
        ]

        #expect(WindowIDs.capturable(from: listed, pid: 4321).map(\.id) == [3, 1, 2])
    }
}
