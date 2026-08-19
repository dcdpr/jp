import ApplicationServices

@testable import DriveKit

/// Records clicks and drags instead of posting them.
///
/// A real click goes to the window server and lands on whatever occupies the
/// coordinate, so the only part a test can hold still is where the driver aimed.
final class FakePoster: EventPoster {
    /// Every point clicked, in order.
    private(set) var clicks: [CGPoint] = []

    /// Every drag's path, in order.
    private(set) var drags: [[CGPoint]] = []

    /// The pause each drag was asked to wait between moves.
    private(set) var pauses: [Duration] = []

    /// What `click` and `drag` should answer, for exercising a post that could
    /// not be built.
    var succeeds = true

    func click(at point: CGPoint) -> Bool {
        guard succeeds else { return false }
        clicks.append(point)
        return true
    }

    func drag(through path: [CGPoint], pausing pause: Duration) -> Bool {
        guard succeeds else { return false }
        drags.append(path)
        pauses.append(pause)
        return true
    }
}
