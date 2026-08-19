import CoreGraphics
import Foundation
import ImageIO
import Testing
import UniformTypeIdentifiers

@testable import DriveKit

/// Tests for reading a screenshot's pixels.
///
/// Everything here works against a PNG written by the test, so none of it needs a
/// window server, a running app or a Screen Recording grant.
@Suite("Pixels")
struct PixelsTests {
    /// A four-by-two image, written to a temporary file and removed afterwards.
    ///
    /// Rows top to bottom, each row left to right, as `#RRGGBB` strings. Written
    /// through `CGImageDestination` so the file is a real PNG decoded by the same
    /// path a screenshot takes.
    private func withImage(
        rows: [[String]],
        _ body: (String) throws -> Void
    ) throws {
        let height = rows.count
        let width = try #require(rows.first?.count)

        var bytes: [UInt8] = []
        for row in rows {
            for hex in row {
                let value = try #require(UInt32(hex.dropFirst(), radix: 16))
                bytes.append(UInt8((value >> 16) & 0xFF))
                bytes.append(UInt8((value >> 8) & 0xFF))
                bytes.append(UInt8(value & 0xFF))
                bytes.append(255)
            }
        }

        let space = try #require(CGColorSpace(name: CGColorSpace.sRGB))
        let provider = try #require(CGDataProvider(data: Data(bytes) as CFData))
        let image = try #require(
            CGImage(
                width: width,
                height: height,
                bitsPerComponent: 8,
                bitsPerPixel: 32,
                bytesPerRow: width * 4,
                space: space,
                bitmapInfo: CGBitmapInfo(rawValue: CGImageAlphaInfo.premultipliedLast.rawValue),
                provider: provider,
                decode: nil,
                shouldInterpolate: false,
                intent: .defaultIntent
            ))

        let path = NSTemporaryDirectory() + "/jpdrive-pixels-\(UUID().uuidString).png"
        let url = URL(fileURLWithPath: path) as CFURL
        let destination = try #require(
            CGImageDestinationCreateWithURL(url, UTType.png.identifier as CFString, 1, nil))
        CGImageDestinationAddImage(destination, image, nil)
        #expect(CGImageDestinationFinalize(destination))

        defer { try? FileManager.default.removeItem(atPath: path) }
        try body(path)
    }

    /// Two colours across a row, which is the shape every real question takes: a
    /// wide background, a narrow line, and the offset where one becomes the other.
    @Test("collapses a row into runs of one colour")
    func scansARow() throws {
        try withImage(rows: [
            ["#FFFFFF", "#FFFFFF", "#DBDBDB", "#FFFFFF"],
            ["#000000", "#000000", "#000000", "#000000"],
        ]) { path in
            let report = try Pixels.read(
                PixelOptions(image: path, axis: .row, at: 0, from: nil, to: nil))

            #expect(report.width == 4)
            #expect(report.height == 2)
            #expect(report.colorSpace == "sRGB")
            #expect(
                report.runs == [
                    PixelRun(start: 0, count: 2, color: "#FFFFFF"),
                    PixelRun(start: 2, count: 1, color: "#DBDBDB"),
                    PixelRun(start: 3, count: 1, color: "#FFFFFF"),
                ]
            )
        }
    }

    /// Rows are indexed down from the top, the way a screenshot is read, not up
    /// from the bottom the way CoreGraphics draws.
    @Test("counts rows down from the top")
    func rowsCountFromTheTop() throws {
        try withImage(rows: [
            ["#FF0000", "#FF0000"],
            ["#00FF00", "#00FF00"],
            ["#0000FF", "#0000FF"],
        ]) { path in
            let top = try Pixels.read(
                PixelOptions(image: path, axis: .row, at: 0, from: nil, to: nil))
            let bottom = try Pixels.read(
                PixelOptions(image: path, axis: .row, at: 2, from: nil, to: nil))

            #expect(top.runs == [PixelRun(start: 0, count: 2, color: "#FF0000")])
            #expect(bottom.runs == [PixelRun(start: 0, count: 2, color: "#0000FF")])
        }
    }

    @Test("collapses a column into runs of one colour")
    func scansAColumn() throws {
        try withImage(rows: [
            ["#FFFFFF", "#111111"],
            ["#FFFFFF", "#111111"],
            ["#222222", "#111111"],
        ]) { path in
            let report = try Pixels.read(
                PixelOptions(image: path, axis: .column, at: 0, from: nil, to: nil))

            #expect(report.scan == "column")
            #expect(
                report.runs == [
                    PixelRun(start: 0, count: 2, color: "#FFFFFF"),
                    PixelRun(start: 2, count: 1, color: "#222222"),
                ]
            )
        }
    }

    /// A window is nine hundred points wide and the interesting part is a few of
    /// them, so a scan can be bounded. The offsets stay absolute, because they are
    /// what gets compared against a frame from the accessibility tree.
    @Test("bounds a scan and keeps the offsets absolute")
    func boundsAScan() throws {
        try withImage(rows: [["#FFFFFF", "#AAAAAA", "#BBBBBB", "#FFFFFF"]]) { path in
            let report = try Pixels.read(
                PixelOptions(image: path, axis: .row, at: 0, from: 1, to: 2))

            #expect(
                report.runs == [
                    PixelRun(start: 1, count: 1, color: "#AAAAAA"),
                    PixelRun(start: 2, count: 1, color: "#BBBBBB"),
                ]
            )
        }
    }

    /// Reading past the edge is a mistake worth reporting rather than clamping: a
    /// silently moved scan answers a question nobody asked.
    @Test("refuses a line outside the image")
    func refusesALineOutside() throws {
        try withImage(rows: [["#FFFFFF"]]) { path in
            #expect(throws: DriveError.self) {
                try Pixels.read(
                    PixelOptions(image: path, axis: .row, at: 7, from: nil, to: nil))
            }
        }
    }

    @Test("reports an image it cannot read")
    func reportsAMissingImage() {
        #expect(throws: DriveError.self) {
            try Pixels.read(
                PixelOptions(
                    image: "/no/such/screenshot.png", axis: .row, at: 0, from: nil, to: nil))
        }
    }

    /// A translucent pixel carries its alpha, so it cannot be mistaken for an
    /// opaque one of the same colour.
    @Test("spells an opaque colour without alpha and a translucent one with it")
    func spellsAlphaOnlyWhenItMatters() {
        #expect(Pixel(red: 0xDB, green: 0xDB, blue: 0xDB, alpha: 255).hex == "#DBDBDB")
        #expect(Pixel(red: 0xDB, green: 0xDB, blue: 0xDB, alpha: 128).hex == "#DBDBDB80")
    }

    @Test("collapses an empty line into no runs")
    func emptyLine() {
        #expect(Pixels.runs(of: [], startingAt: 0).isEmpty)
    }

    /// A screenshot is written in the display's profile, which is not the space a
    /// palette constant was written in. Read raw, a `#DBDBDB` divider comes back
    /// as something several steps off and looks like a bug in the app.
    ///
    /// The image here is tagged Display P3 and holds the P3 encoding of sRGB
    /// `#DBDBDB`, so a reader that converts reports the value the palette names
    /// and a reader that does not reports `#DBDBDB` itself — which is the wrong
    /// answer, arrived at by leaving the numbers alone.
    @Test("reports colours in sRGB whatever space the image is tagged with")
    func convertsToSRGB() throws {
        let p3 = try #require(CGColorSpace(name: CGColorSpace.displayP3))
        let sRGB = try #require(CGColorSpace(name: CGColorSpace.sRGB))
        let level = CGFloat(0xDB) / 255
        let grey = try #require(
            CGColor(colorSpace: sRGB, components: [level, level, level, 1]))
        let converted = try #require(
            grey.converted(to: p3, intent: CGColorRenderingIntent.defaultIntent, options: nil))
        let parts = try #require(converted.components)

        let path = NSTemporaryDirectory() + "/jpdrive-p3-\(UUID().uuidString).png"
        defer { try? FileManager.default.removeItem(atPath: path) }

        let bytes = parts.prefix(3).map { UInt8(($0 * 255).rounded()) } + [255]
        let provider = try #require(CGDataProvider(data: Data(bytes) as CFData))
        let image = try #require(
            CGImage(
                width: 1,
                height: 1,
                bitsPerComponent: 8,
                bitsPerPixel: 32,
                bytesPerRow: 4,
                space: p3,
                bitmapInfo: CGBitmapInfo(rawValue: CGImageAlphaInfo.premultipliedLast.rawValue),
                provider: provider,
                decode: nil,
                shouldInterpolate: false,
                intent: .defaultIntent
            ))

        let url = URL(fileURLWithPath: path) as CFURL
        let destination = try #require(
            CGImageDestinationCreateWithURL(url, UTType.png.identifier as CFString, 1, nil))
        CGImageDestinationAddImage(destination, image, nil)
        #expect(CGImageDestinationFinalize(destination))

        let report = try Pixels.read(
            PixelOptions(image: path, axis: .row, at: 0, from: nil, to: nil))

        #expect(report.runs.first?.color == "#DBDBDB")
    }
}
