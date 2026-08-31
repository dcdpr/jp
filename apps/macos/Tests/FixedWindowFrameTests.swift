import CoreGraphics
import Testing

@testable import JP

/// The window frame a UI test pins the app to.
@Suite("FixedWindowFrame")
struct FixedWindowFrameTests {
    /// A UI test bundle drives the app from another process and cannot import
    /// it, so `AppUnderTest` spells this key out as a literal. Renaming the app's
    /// constant without changing that literal would leave every launch inheriting
    /// the previous run's window size again — silently, because an unset variable
    /// means "behave normally".
    ///
    /// This is the only thing holding the two spellings together.
    @Test("is read from the variable the UI tests set")
    func keyMatchesTheOneUITestsSet() {
        #expect(FixedWindowFrame.environmentKey == "JP_WINDOW_FRAME")
    }

    /// A screen with room to spare gets the frame that was asked for. Clamping a
    /// window that already fits would move the edges every test drives.
    @Test("leaves a frame the screen can show alone")
    func aFittingFrameIsUnchanged() {
        let visible = CGRect(x: 0, y: 0, width: 1710, height: 1050)

        let size = FixedWindowFrame.contentSize(
            pinning: CGSize(width: 1000, height: 700),
            within: visible
        )

        #expect(size == CGSize(width: 1000, height: 700))
    }

    /// The failure this exists for: on a display narrower than the pinned width,
    /// an unclamped window keeps that width and puts its right edge past the
    /// screen. A pointer stops at the display bounds, so a test driving that edge
    /// presses whatever is at the boundary and the window never resizes.
    ///
    /// The screen is tall enough for the pinned height on purpose, so the height
    /// assertion says the axis that fits was left alone. A 768-tall screen leaves
    /// 688 after the insets and would clamp both, which is the case below.
    @Test("pulls a frame wider than the screen back onto it")
    func aTooWideFrameIsClamped() {
        let visible = CGRect(x: 0, y: 0, width: 1024, height: 900)

        let size = FixedWindowFrame.contentSize(
            pinning: CGSize(width: 1000, height: 700),
            within: visible
        )

        #expect(size.width == 1024 - FixedWindowFrame.inset * 2)
        #expect(size.height == 700)
    }

    /// Both axes clamp, and the origin puts the window `inset` from the top left,
    /// so the slack has to cover the far edges too.
    @Test("keeps the whole window inside the screen on both axes")
    func aClampedFrameFitsWithItsInsets() {
        let visible = CGRect(x: 0, y: 0, width: 900, height: 600)

        let size = FixedWindowFrame.contentSize(
            pinning: CGSize(width: 1000, height: 700),
            within: visible
        )

        #expect(size.width + FixedWindowFrame.inset * 2 <= visible.width)
        #expect(size.height + FixedWindowFrame.inset * 2 <= visible.height)
    }

    /// A screen's visible frame does not start at the origin — the menu bar and
    /// the Dock move it — and only its size decides what fits.
    @Test("measures the screen's size rather than its position")
    func anOffsetVisibleFrameClampsTheSameWay() {
        let atOrigin = CGRect(x: 0, y: 0, width: 1024, height: 768)
        let offset = CGRect(x: 1512, y: 38, width: 1024, height: 768)
        let pin = CGSize(width: 1000, height: 700)

        #expect(
            FixedWindowFrame.contentSize(pinning: pin, within: atOrigin)
                == FixedWindowFrame.contentSize(pinning: pin, within: offset)
        )
    }
}
