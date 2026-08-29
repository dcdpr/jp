import ApplicationServices
import Foundation

/// What `jpdrive doctor` observed.
struct DoctorReport: Encodable {
    /// `AXIsProcessTrusted()` for this process.
    let trusted: Bool

    /// This process and its ancestors, nearest first. One of these holds the
    /// Accessibility grant when `trusted` is true.
    let processes: [ProcessLink]

    /// A real read against a target app, present when `--pid` was given.
    let probe: WindowProbe?
}

/// The outcome of reading a target application's window list.
struct WindowProbe: Encodable {
    let pid: pid_t

    /// The target's short command name.
    let command: String

    /// How many windows were read, when the read succeeded.
    let windowCount: Int?

    /// The accessibility error, when it did not.
    let axError: String?
}

/// Answers whether this process may read another application's accessibility
/// tree, and records the evidence for why.
///
/// `AXIsProcessTrusted()` alone is not enough: it reports what TCC believes
/// about the responsible process, which is not always the process making the
/// call. So the report pairs the flag with a real `AXUIElementCopyAttributeValue`
/// against a live app, and with the ancestor chain the grant might be attributed
/// to. Apple documents neither the attribution algorithm nor its stability, so
/// this records observations rather than asserting a rule.
enum Doctor {
    /// Run every probe and collect the results.
    ///
    /// Throws only when `pid` names a process that is not running. A refused
    /// accessibility read is an observation the report carries, not a failure of
    /// the diagnostic.
    static func run(targetPid pid: pid_t?) throws(DriveError) -> DoctorReport {
        let probe: WindowProbe?
        if let pid {
            guard let record = ProcessTable.record(for: pid) else {
                throw DriveError(
                    kind: .appNotRunning,
                    message: "no process is running under pid \(pid)",
                    hint: "start the app, then pass its pid: --pid $(pgrep -f JP.app)"
                )
            }
            probe = windowProbe(pid: pid, command: ProcessTable.name(of: record))
        } else {
            probe = nil
        }

        return DoctorReport(
            trusted: AXIsProcessTrusted(),
            processes: ProcessTable.ancestry(from: getpid()),
            probe: probe
        )
    }

    /// Read the target's window list, reporting the accessibility error instead
    /// of the count when the read is refused.
    ///
    /// Uses the non-prompting trust path throughout: a spike that raises the
    /// system's "grant access" dialog changes the state it is measuring.
    private static func windowProbe(pid: pid_t, command: String) -> WindowProbe {
        let app = AXUIElementCreateApplication(pid)
        var value: CFTypeRef?
        let status = AXUIElementCopyAttributeValue(app, kAXWindowsAttribute as CFString, &value)

        guard status == .success else {
            return WindowProbe(
                pid: pid,
                command: command,
                windowCount: nil,
                axError: status.name
            )
        }

        let windows = value as? [AXUIElement]
        return WindowProbe(
            pid: pid,
            command: command,
            windowCount: windows?.count ?? 0,
            axError: nil
        )
    }
}
