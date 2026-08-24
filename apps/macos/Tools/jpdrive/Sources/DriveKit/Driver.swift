import Foundation

/// The driver's entry point.
///
/// Everything below this is internal to the library, so the tests reach it with
/// `@testable import` and the executable target stays a single line.
public enum Driver {
    /// Parse the process arguments, run the command, and exit.
    ///
    /// Writes one JSON document to stdout either way: a result, or an error with a
    /// non-zero exit status.
    public static func run() -> Never {
        do throws(DriveError) {
            try dispatch(Array(CommandLine.arguments.dropFirst()))
        } catch {
            Output.writeError(error)
            exit(1)
        }

        exit(0)
    }

    /// Run one command and write its result.
    static func dispatch(_ arguments: [String]) throws(DriveError) {
        switch try Arguments.parse(arguments) {
        case .doctor(let pid):
            try Output.write(try Doctor.run(targetPid: pid))

        case .dump(let options):
            try Output.write(try Dump.walk(options))

        case .tree(let options):
            guard let tree = try Tree.read(options) else {
                throw DriveError(
                    kind: .identifierNotFound,
                    message:
                        "no element's identifier begins with \(options.identifierPrefix ?? "")",
                    hint: "drop --identifier to see what the application reports"
                )
            }
            try Output.write(tree)

        case .windows(let pid):
            try Output.write(try Windows.read(pid: pid))

        case .windowid(let pid):
            try Output.write(try WindowIDs.read(pid: pid))

        case .menu(let pid, let options):
            try Output.write(try Menu.read(pid: pid, options: options))

        case .act(let step, let pid):
            try Output.write(try Act.run(step, pid: pid))

        case .pixels(let options):
            try Output.write(try Pixels.read(options))

        case .frontmost(let set):
            let report =
                if let set { Ambient.activate(bundleID: set) } else { Ambient.frontmost() }
            try Output.write(report)

        case .pointer(let set):
            let report = if let set { Ambient.movePointer(to: set) } else { Ambient.pointer() }
            try Output.write(report)
        }
    }
}
