import AppKit
import Testing

@testable import JP

/// Tests for the palette and the mechanism that resolves it.
///
/// Not a pin of every hex value: those are declared once in `Theme.swift` and
/// re-typing them here would only assert that copy-paste works. What is worth
/// holding is the wiring — that a colour resolves to its light half under a light
/// appearance and its dark half under a dark one, and that a value survives the
/// trip through `NSColor` unchanged.
@Suite("Theme")
struct ThemeTests {
    /// Every colour the palette declares, so a test can hold all of them to the
    /// same rule at once.
    private static let palette: [(name: String, color: ThemeColor)] = [
        ("sidebarBackground", Theme.sidebarBackground),
        ("selectedRowBackground", Theme.selectedRowBackground),
        ("paneDivider", Theme.paneDivider),
        ("rowSeparator", Theme.rowSeparator),
        ("searchFieldBackground", Theme.searchFieldBackground),
        ("editorBackground", Theme.editorBackground),
        ("bodyText", Theme.bodyText),
        ("secondaryText", Theme.secondaryText),
        ("accent", Theme.accent),
        ("inlineCodeBackground", Theme.inlineCodeBackground),
        ("inlineCodeText", Theme.inlineCodeText),
        ("tagBackground", Theme.tagBackground),
        ("tagText", Theme.tagText),
    ]

    /// The hex `color` resolves to under `appearance`, read back off the drawn
    /// colour rather than off the declaration.
    ///
    /// A dynamic `NSColor` reports nothing about its components until it is
    /// resolved against an appearance, which is what `usingColorSpace` after
    /// `performAsCurrentDrawingAppearance` does here.
    private func drawn(_ color: ThemeColor, under appearance: NSAppearance) -> UInt32? {
        var resolved: NSColor?
        appearance.performAsCurrentDrawingAppearance {
            resolved = color.nsColor.usingColorSpace(.sRGB)
        }

        guard let resolved else { return nil }

        let component = { (value: CGFloat) in UInt32((value * 255).rounded()) }
        return component(resolved.redComponent) << 16
            | component(resolved.greenComponent) << 8
            | component(resolved.blueComponent)
    }

    @Test("draws its light half under a light appearance")
    func resolvesLight() throws {
        let aqua = try #require(NSAppearance(named: .aqua))

        for entry in Self.palette {
            #expect(
                drawn(entry.color, under: aqua) == entry.color.light,
                "\(entry.name) drew the wrong colour in light appearance"
            )
        }
    }

    @Test("draws its dark half under a dark appearance")
    func resolvesDark() throws {
        let darkAqua = try #require(NSAppearance(named: .darkAqua))

        for entry in Self.palette {
            #expect(
                drawn(entry.color, under: darkAqua) == entry.color.dark,
                "\(entry.name) drew the wrong colour in dark appearance"
            )
        }
    }

    /// One line between the panes, dozens between the rows: at the same weight
    /// the list reads as a grid, so the two are deliberately different.
    @Test("separates rows more lightly than it separates panes")
    func rowsAreSeparatedMoreLightly() {
        #expect(Theme.rowSeparator.light > Theme.paneDivider.light)
    }

    /// The two halves of every colour differ. A pair that matched would be a
    /// half-finished copy-paste, and it looks like a working app right up until
    /// somebody switches appearance and finds white text on white.
    @Test("gives every colour two distinct halves")
    func halvesDiffer() {
        for entry in Self.palette {
            #expect(
                entry.color.light != entry.color.dark,
                "\(entry.name) is the same colour in both appearances"
            )
        }
    }

    /// The accessibility appearances are variants of the two base ones and have
    /// names of their own, so matching by name alone would send a high-contrast
    /// dark window down the light path.
    @Test("treats the high-contrast dark appearance as dark")
    func highContrastDarkIsDark() throws {
        let variant = try #require(NSAppearance(named: .accessibilityHighContrastDarkAqua))

        #expect(variant.isDark)
    }

    @Test("treats the high-contrast light appearance as light")
    func highContrastLightIsLight() throws {
        let variant = try #require(NSAppearance(named: .accessibilityHighContrastAqua))

        #expect(variant.isDark == false)
    }

    /// The channels are unpacked in the right order, which a grey would hide.
    @Test("unpacks a hex value into its channels")
    func unpacksChannels() throws {
        let color = try #require(ThemeColor.srgb(0x11_22_33).usingColorSpace(.sRGB))

        #expect((color.redComponent * 255).rounded() == 0x11)
        #expect((color.greenComponent * 255).rounded() == 0x22)
        #expect((color.blueComponent * 255).rounded() == 0x33)
        #expect(color.alphaComponent == 1)
    }
}
