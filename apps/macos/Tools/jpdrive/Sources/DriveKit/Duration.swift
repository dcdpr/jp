import Foundation

extension Duration {
    /// The duration in whole milliseconds, for reporting.
    var milliseconds: Int {
        let (seconds, attoseconds) = components
        return Int(seconds) * 1000 + Int(attoseconds / 1_000_000_000_000_000)
    }

    /// The duration in seconds, for the APIs that take a `TimeInterval`.
    var seconds: TimeInterval {
        let (seconds, attoseconds) = components
        return TimeInterval(seconds) + TimeInterval(attoseconds) / 1e18
    }
}
