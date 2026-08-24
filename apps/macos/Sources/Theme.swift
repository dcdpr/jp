import AppKit
import SwiftUI

/// One colour of the palette, in both appearances.
///
/// Held as sRGB numbers rather than as `Color`s, so a colour is defined in one
/// place for both appearances and the palette can be read and compared without
/// a running app.
struct ThemeColor: Equatable, Sendable {
    /// The value used in light appearance, as `0xRRGGBB`.
    let light: UInt32

    /// The value used in dark appearance, as `0xRRGGBB`.
    let dark: UInt32

    /// The SwiftUI colour to draw with.
    ///
    /// Resolves per appearance as it draws rather than being fixed when it is
    /// built: a window moved between appearances redraws from the same `Color`
    /// value and has to pick up the other half.
    var color: Color {
        Color(nsColor: nsColor)
    }

    /// The AppKit colour behind ``color``.
    var nsColor: NSColor {
        let (light, dark) = (self.light, self.dark)

        return NSColor(name: nil) { appearance in
            Self.srgb(appearance.isDark ? dark : light)
        }
    }

    /// The value this shows under `appearance`.
    func value(under appearance: NSAppearance) -> UInt32 {
        appearance.isDark ? dark : light
    }

    /// An opaque sRGB colour from `0xRRGGBB`.
    static func srgb(_ hex: UInt32) -> NSColor {
        NSColor(
            srgbRed: Double((hex >> 16) & 0xFF) / 255,
            green: Double((hex >> 8) & 0xFF) / 255,
            blue: Double(hex & 0xFF) / 255,
            alpha: 1
        )
    }
}

extension NSAppearance {
    /// Whether this is one of the dark appearances.
    ///
    /// Matched rather than compared by name, because the accessibility variants
    /// (`accessibilityHighContrastDarkAqua` and friends) are dark too and have
    /// names of their own.
    var isDark: Bool {
        bestMatch(from: [.aqua, .darkAqua]) == .darkAqua
    }
}

/// The app's colours, in one place.
///
/// Every surface and every piece of text picks its colour from here rather than
/// from a system semantic colour, because the app's appearance is a design
/// decision that has to hold across both appearances and both windows.
enum Theme {
    /// Behind the conversation list.
    static let sidebarBackground = ThemeColor(light: 0xFF_FFFF, dark: 0x1D_1E20)

    /// Behind the selected row of the conversation list.
    static let selectedRowBackground = ThemeColor(light: 0xF4_F5F7, dark: 0x2E_2E30)

    /// The line between the sidebar and the transcript, and the search field's
    /// border.
    static let paneDivider = ThemeColor(light: 0xD9_D9D9, dark: 0x2C_2D2E)

    /// The line between two rows of the conversation list.
    ///
    /// Lighter than ``paneDivider``, because there is one of those and dozens of
    /// these: at the pane divider's weight the list reads as a grid.
    static let rowSeparator = ThemeColor(light: 0xE4_E5E6, dark: 0x2C_2D2E)

    /// Behind the search field.
    static let searchFieldBackground = ThemeColor(light: 0xFF_FFFF, dark: 0x1D_1E20)

    /// Behind the transcript.
    static let editorBackground = ThemeColor(light: 0xFF_FFFF, dark: 0x1D_1E20)

    /// Prose, and anything else a person is meant to read.
    static let bodyText = ThemeColor(light: 0x44_4444, dark: 0xCC_DBE5)

    /// Dates, counts, speaker names — text that labels rather than says.
    static let secondaryText = ThemeColor(light: 0x88_8888, dark: 0xA2_A3A4)

    /// The one colour that draws the eye: the pin glyph, and controls that tint.
    ///
    /// Red in light appearance and blue in dark, which is not a mistake — it is
    /// what the design calls for.
    static let accent = ThemeColor(light: 0xDD_4D4F, dark: 0x45_A2E5)

    /// Behind an inline code span.
    static let inlineCodeBackground = ThemeColor(light: 0xF4_F5F7, dark: 0x2E_2E30)

    /// An inline code span's text.
    static let inlineCodeText = ThemeColor(light: 0x44_4444, dark: 0xDF_E0E0)

    /// Behind a tag pill.
    static let tagBackground = ThemeColor(light: 0xE4_E5E6, dark: 0x46_4647)

    /// A tag pill's text.
    static let tagText = ThemeColor(light: 0x44_4444, dark: 0xDF_E0E0)
}
