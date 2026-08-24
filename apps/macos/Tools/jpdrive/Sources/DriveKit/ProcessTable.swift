import Darwin
import Foundation

/// One process in the chain from the driver up towards `launchd`.
struct ProcessLink: Encodable {
    let pid: pid_t

    /// Short command name from the kernel process table. The kernel truncates it
    /// to 16 bytes, so `Terminal` and `iTerm2` arrive whole but a long binary
    /// name does not.
    let command: String
}

/// Process identity read from the kernel through `sysctl(KERN_PROC_PID)`.
///
/// The spike needs the ancestor chain because TCC attributes a grant to the
/// responsible process, and the chain is the list of candidates for that role.
enum ProcessTable {
    /// Depth limit for the ancestor walk. A shell-to-`launchd` chain is a
    /// handful of processes; the limit only guards against a process table that
    /// changes underneath the walk.
    private static let maxDepth = 32

    /// `pid` and its ancestors, nearest first, stopping below `launchd`.
    static func ancestry(from pid: pid_t) -> [ProcessLink] {
        var links: [ProcessLink] = []
        var current = pid

        while current > 1, links.count < maxDepth {
            guard let record = record(for: current) else { break }
            links.append(ProcessLink(pid: current, command: name(of: record)))
            current = record.kp_eproc.e_ppid
        }

        return links
    }

    /// The kernel's record for `pid`, or `nil` when no such process is running.
    static func record(for pid: pid_t) -> kinfo_proc? {
        var selector: [Int32] = [CTL_KERN, KERN_PROC, KERN_PROC_PID, pid]
        var record = kinfo_proc()
        var size = MemoryLayout<kinfo_proc>.stride

        let result = sysctl(&selector, u_int(selector.count), &record, &size, nil, 0)

        // Querying a pid that no longer exists succeeds and writes nothing, so
        // the written size is what separates a dead pid from a live one.
        guard result == 0, size > 0 else { return nil }
        return record
    }

    /// The short command name held in a kernel record.
    static func name(of record: kinfo_proc) -> String {
        return withUnsafeBytes(of: record.kp_proc.p_comm) { bytes in
            return String(decoding: bytes.prefix { $0 != 0 }, as: UTF8.self)
        }
    }
}
