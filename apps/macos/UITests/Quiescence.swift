import Foundation
import ObjectiveC

/// Stops XCUITest waiting for the app under test to go quiet after every event
/// it synthesizes.
///
/// Worth about a second of a sixteen-second run, which is less than it sounds
/// like it should be: the wait is not what makes a synthesized click expensive.
/// A click costs ~410ms with this installed and ~440ms without, against an app
/// that answers in tens of milliseconds; the rest is inside XCTest's pointer
/// path and out of reach from here. Do not expect a second one of these to turn
/// up.
///
/// Safe because nothing in this suite leans on the wait. Every assertion waits
/// on a condition of its own through ``AppUnderTest/wait(for:)``, which is
/// faster and specific about what it is waiting for; an implicit settle after
/// each event only hides where a real one is missing. A test that starts
/// failing after a change here is a test that was relying on it — give it the
/// wait it actually needs rather than putting this one back.
///
/// Private API, reached by replacing two method implementations. It lives in
/// the test bundle and nothing ships it. It is version-fragile: the selector
/// this replaces was one argument in 2016, is two now, and picked up a third in
/// a variant along the way. So ``install()`` checks every assumption it makes
/// and reports rather than guessing, and ``AppUnderTest/launch(against:)``
/// fails the run when it reports. A silent no-op would put the second back and
/// tell nobody.
enum Quiescence {
    /// What went wrong installing this, or `nil` if it took.
    ///
    /// A `let`, so the work happens once however many apps a run launches.
    static let installation: String? = install()

    /// The class that does the waiting.
    private static let className = "XCUIApplicationProcess"

    /// Replace both waits, or say why not.
    ///
    /// Both, not either: XCTest calls the plain one and the one that opens an
    /// activity around the wait, and leaving one in place leaves its share of
    /// the cost in place with it.
    ///
    /// `shouldSkipPreEventQuiescence` and `shouldSkipPostEventQuiescence` look
    /// like the better target — no arguments, `BOOL` return, nothing to get
    /// wrong — and forcing both to `true` measurably changes nothing. XCTest
    /// does not consult them on the path that costs.
    ///
    /// The encodings are checked rather than assumed, because a replacement is
    /// called through a signature the runtime does not police: a method that
    /// gained an argument, or that returns something other than `void`, would
    /// be called with the wrong frame and go wrong somewhere unrelated. `v` is
    /// void, `@0:8` the receiver and selector every method takes, and each `B`
    /// a `_Bool` argument.
    ///
    /// `B` rather than `c` is not an architecture assumption: these parameters
    /// are `_Bool`, not `BOOL`, so they encode as `B` under x86_64 as well.
    /// Verified by reading the encoding out of the x86_64 slice under Rosetta.
    private static func install() -> String? {
        guard let process: AnyClass = NSClassFromString(className) else {
            return "XCTest no longer has a class named \(className)."
        }

        let two: @convention(block) (AnyObject, Bool, Bool) -> Void = { _, _, _ in }
        let three: @convention(block) (AnyObject, Bool, Bool, Bool) -> Void = { _, _, _, _ in }

        let replacements = [
            (
                name: "waitForQuiescenceIncludingAnimationsIdle:isPreEvent:",
                encoding: "v24@0:8B16B20",
                imp: imp_implementationWithBlock(two)
            ),
            (
                name: "waitForQuiescenceIncludingAnimationsIdle:usingActivity:isPreEvent:",
                encoding: "v28@0:8B16B20B24",
                imp: imp_implementationWithBlock(three)
            ),
        ]

        for replacement in replacements {
            let selector = NSSelectorFromString(replacement.name)
            guard let method = class_getInstanceMethod(process, selector) else {
                return "\(className) no longer answers \(replacement.name)."
            }

            let found = method_getTypeEncoding(method).map { String(cString: $0) } ?? "(none)"
            guard found == replacement.encoding else {
                return """
                    \(className).\(replacement.name) is \(found), \
                    expected \(replacement.encoding).
                    """
            }

            method_setImplementation(method, replacement.imp)
        }

        return nil
    }
}
