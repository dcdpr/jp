import CoreGraphics
import Foundation
import ImageIO

/// One stretch of identical pixels along a scanline.
struct PixelRun: Encodable, Equatable {
    /// Where the run begins, in pixels along the scan.
    let start: Int

    /// How many pixels it covers.
    let count: Int

    /// The colour, `#RRGGBB` when opaque and `#RRGGBBAA` when it is not.
    let color: String
}

/// What one scan across an image found.
struct PixelReport: Encodable, Equatable {
    /// The image's width in pixels, which on a retina display is twice its width
    /// in points.
    let width: Int

    /// The image's height in pixels.
    let height: Int

    /// The colour space the values are reported in.
    ///
    /// Always sRGB. Stated anyway, because the numbers mean nothing without it:
    /// the same screenshot read in the display's own profile and in sRGB gives two
    /// different sets of values for the same pixels, and a light grey moves by
    /// several steps between them.
    ///
    /// sRGB because that is the space colours are *written* in — a palette
    /// constant, a value from a colour picker, a hex in a design note — so a
    /// reading can be compared against the thing it was supposed to be.
    let colorSpace: String

    /// Which way the scan ran: `row` or `column`.
    let scan: String

    /// The row or column that was read, in pixels.
    let at: Int

    /// The runs along it, in order, covering the scanned range without gaps.
    let runs: [PixelRun]
}

/// What to scan, and where.
struct PixelOptions {
    /// Which way a scan runs.
    enum Axis: String {
        /// Left to right, across one row.
        case row

        /// Top to bottom, down one column.
        case column
    }

    /// The PNG to read.
    let image: String

    let axis: Axis

    /// The row or column to read, in pixels.
    let at: Int

    /// Where along the scan to start, in pixels. The near edge when absent.
    let from: Int?

    /// Where along the scan to stop, inclusive, in pixels. The far edge when
    /// absent.
    let to: Int?
}

/// Reads the pixels of a screenshot.
///
/// Answers the questions the accessibility tree cannot: what colour something is,
/// and how wide a drawn thing is. A hairline, a selection fill, a divider and a
/// row separator are all invisible to the tree, and all obvious in a scanline.
///
/// Reads a file rather than capturing one. Capture already has a home
/// (`screencapture`, driven by `debug_app_screenshot`), and the only ways to
/// capture from inside this process are deprecated. It also makes this testable
/// against an image built by hand, with no window server and no grants.
enum Pixels {
    /// Scan `options.image` and report the runs along the requested line.
    static func read(_ options: PixelOptions) throws(DriveError) -> PixelReport {
        let bitmap = try Bitmap(path: options.image)
        let extent = options.axis == .row ? bitmap.width : bitmap.height
        let across = options.axis == .row ? bitmap.height : bitmap.width

        guard options.at >= 0, options.at < across else {
            throw DriveError(
                kind: .notFound,
                message:
                    "\(options.axis.rawValue) \(options.at) is outside the image, which is "
                    + "\(bitmap.width)x\(bitmap.height) pixels",
                hint: "a row is indexed down from the top and a column across from the left"
            )
        }

        let from = max(options.from ?? 0, 0)
        let to = min(options.to ?? extent - 1, extent - 1)

        guard from <= to else {
            throw DriveError(
                kind: .badUsage,
                message: "--from \(from) is past --to \(to)",
                hint: "both are pixel offsets along the scan, and --to is inclusive"
            )
        }

        let line = (from...to).map { along in
            options.axis == .row
                ? bitmap.pixel(x: along, y: options.at)
                : bitmap.pixel(x: options.at, y: along)
        }

        return PixelReport(
            width: bitmap.width,
            height: bitmap.height,
            colorSpace: bitmap.colorSpace,
            scan: options.axis.rawValue,
            at: options.at,
            runs: runs(of: line, startingAt: from)
        )
    }

    /// Collapse `line` into runs of one colour, the first starting at `start`.
    ///
    /// The whole point of the output shape: a scan across a window is thousands of
    /// pixels and a handful of colours, and the edges between them are the
    /// measurements a reader is after.
    static func runs(of line: [Pixel], startingAt start: Int) -> [PixelRun] {
        var runs: [PixelRun] = []

        for (offset, pixel) in line.enumerated() {
            if let last = runs.last, last.color == pixel.hex {
                runs[runs.count - 1] = PixelRun(
                    start: last.start, count: last.count + 1, color: last.color)
                continue
            }

            runs.append(PixelRun(start: start + offset, count: 1, color: pixel.hex))
        }

        return runs
    }
}

/// One pixel, as read out of an image.
struct Pixel: Equatable {
    let red: UInt8
    let green: UInt8
    let blue: UInt8
    let alpha: UInt8

    /// `#RRGGBB` when opaque, `#RRGGBBAA` when not.
    ///
    /// Alpha is left off the common case so the values read the way a colour
    /// picker reports them, and included when it is not 255 because a translucent
    /// pixel that printed as opaque would be a lie about what is on screen.
    var hex: String {
        let rgb = String(format: "#%02X%02X%02X", red, green, blue)
        return alpha == 255 ? rgb : rgb + String(format: "%02X", alpha)
    }
}

/// An image's pixels, in the image's own colour space.
private struct Bitmap {
    let width: Int
    let height: Int
    let colorSpace: String

    /// RGBA, row-major, four bytes per pixel and no row padding.
    private let bytes: [UInt8]

    /// Decode the PNG at `path`.
    init(path: String) throws(DriveError) {
        guard
            let source = CGImageSourceCreateWithURL(URL(fileURLWithPath: path) as CFURL, nil),
            let image = CGImageSourceCreateImageAtIndex(source, 0, nil)
        else {
            throw DriveError(
                kind: .notFound,
                message: "could not read an image at \(path)",
                hint: "debug_app_screenshot writes one, and reports where it put it"
            )
        }

        width = image.width
        height = image.height

        // Converted rather than read raw. `screencapture` writes in the display's
        // profile, which is often unnamed and never the space a palette was
        // written in: a `#DBDBDB` divider comes back as `#D6D6D6` read that way,
        // which looks like a bug in the app rather than a difference of space.
        let target = CGColorSpace(name: CGColorSpace.sRGB)

        guard
            let target,
            let context = CGContext(
                data: nil,
                width: width,
                height: height,
                bitsPerComponent: 8,
                bytesPerRow: width * 4,
                space: target,
                bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue
            )
        else {
            throw DriveError(
                kind: .notFound,
                message: "could not open \(path) as an 8-bit RGBA image",
                hint: nil
            )
        }

        colorSpace = "sRGB"
        context.draw(image, in: CGRect(x: 0, y: 0, width: width, height: height))

        guard let data = context.data else {
            throw DriveError(
                kind: .notFound,
                message: "the drawing context for \(path) reported no pixels",
                hint: nil
            )
        }

        bytes = [UInt8](
            UnsafeBufferPointer(
                start: data.assumingMemoryBound(to: UInt8.self),
                count: width * height * 4
            ))
    }

    /// The pixel at `x`, `y`, counted from the top-left corner.
    ///
    /// The buffer runs in the same direction: a bitmap context's first row is the
    /// top of what was drawn into it, so a screenshot's rows and this buffer's rows
    /// are the same rows in the same order.
    func pixel(x: Int, y: Int) -> Pixel {
        let offset = (y * width + x) * 4

        return Pixel(
            red: bytes[offset],
            green: bytes[offset + 1],
            blue: bytes[offset + 2],
            alpha: bytes[offset + 3]
        )
    }
}
