import Foundation

/// A parsed command line.
enum Command {
    /// Report whether this process may read another app's accessibility tree.
    case doctor(pid: pid_t?)

    /// Print the elements and attributes under an application.
    case dump(DumpOptions)

    /// Report the elements under an application, identified and described.
    case tree(TreeOptions)

    /// List the application's windows.
    case windows(pid: pid_t)

    /// Report the window-server identifiers of the application's windows.
    case windowid(pid: pid_t)

    /// Report the application's menu bar.
    case menu(pid: pid_t, options: TreeOptions)

    /// Do one thing to one element.
    case act(step: Step, pid: pid_t)

    /// Report the colours along one row or column of a screenshot.
    case pixels(PixelOptions)

    /// Report which application is in front, or put one there.
    case frontmost(set: String?)

    /// Report where the pointer is, or put it somewhere.
    case pointer(set: CGPoint?)
}

/// What to walk, and how much of it.
struct DumpOptions {
    let pid: pid_t

    /// How deep to recurse before reporting a node's children as elided.
    let maxDepth: Int

    /// How many children to walk at each level, or `0` for all of them.
    let maxSiblings: Int

    /// Whether to ask, per attribute, if it can be written.
    let settable: Bool
}

/// The command line the driver accepts.
///
/// Hand-rolled rather than pulled from `swift-argument-parser`: the package has
/// no other dependency, and keeping it that way means the build needs no network
/// and no resolved manifest.
enum Arguments {
    /// Usage text, embedded in every bad-usage error.
    static let usage = """
        usage: jpdrive doctor   [--pid <pid>]
               jpdrive tree     --pid <pid> [--identifier <prefix>] [--max-matches <n>]
                                [--frames] [--depth <n>] [--max-siblings <n>]
               jpdrive windows  --pid <pid>
               jpdrive windowid --pid <pid>
               jpdrive menu     --pid <pid> [--depth <n>] [--max-siblings <n>]
               jpdrive dump     --pid <pid> [--depth <n>] [--max-siblings <n>] [--settable]
               jpdrive act      --pid <pid> --json '<step>'
                                a step is a single-key object, e.g.
                                {"resize":{"identifier":"w","width":1400,"height":900}}
               jpdrive frontmost [--set <bundle-id>]
               jpdrive pointer  [--set <x>,<y>]
               jpdrive pixels   --image <path> --scan row|column --at <n>
                                [--from <n>] [--to <n>]
        """

    /// Depth cap for `dump` when `--depth` is not given.
    ///
    /// A SwiftUI window nests deeply: the wrapper groups between a `List` and its
    /// rows are several levels on their own, so a cap low enough to be tidy hides
    /// the elements worth seeing.
    static let defaultDepth = 20

    /// Sibling cap for `dump` when `--max-siblings` is not given.
    ///
    /// A thousand sidebar rows are a thousand copies of one shape, and walking
    /// them all costs a round-trip per attribute per element. Five is enough to
    /// see the shape and to tell a homogeneous list from a mixed one.
    static let defaultSiblings = 5

    /// Match budget for a filtered `tree` when `--max-matches` is not given.
    ///
    /// Identifiers sit on leaves, so a prefix search cannot prune on the way down
    /// and an unbounded one reads every element in the application. Five answers
    /// what a list looks like; looking up one known identifier wants `1`.
    static let defaultMatches = 5

    /// Parse `arguments`, which excludes the executable path.
    static func parse(_ arguments: [String]) throws(DriveError) -> Command {
        guard let subcommand = arguments.first else {
            throw DriveError(kind: .badUsage, message: usage, hint: nil)
        }

        let options = try options(arguments.dropFirst())

        switch subcommand {
        case "doctor":
            return .doctor(pid: options.pid)

        case "dump":
            guard let pid = options.pid else {
                throw DriveError(
                    kind: .badUsage,
                    message: "dump needs --pid <pid>",
                    hint: usage
                )
            }
            return .dump(
                DumpOptions(
                    pid: pid,
                    maxDepth: options.depth ?? defaultDepth,
                    maxSiblings: options.siblings ?? defaultSiblings,
                    settable: options.settable
                )
            )

        case "tree":
            guard let pid = options.pid else {
                throw DriveError(
                    kind: .badUsage, message: "tree needs --pid <pid>", hint: usage)
            }
            return .tree(
                TreeOptions(
                    pid: pid,
                    identifierPrefix: options.identifier,
                    maxMatches: options.matches ?? defaultMatches,
                    maxDepth: options.depth ?? defaultDepth,
                    maxSiblings: options.siblings ?? defaultSiblings,
                    frames: options.frames
                )
            )

        case "frontmost":
            return .frontmost(set: options.set)

        case "pointer":
            guard let raw = options.set else {
                return .pointer(set: nil)
            }

            let parts = raw.split(separator: ",")
            guard
                parts.count == 2,
                let x = Double(parts[0].trimmingCharacters(in: .whitespaces)),
                let y = Double(parts[1].trimmingCharacters(in: .whitespaces))
            else {
                throw DriveError(
                    kind: .badUsage,
                    message: "pointer --set takes <x>,<y>",
                    hint: usage
                )
            }
            return .pointer(set: CGPoint(x: x, y: y))

        case "windows":
            guard let pid = options.pid else {
                throw DriveError(
                    kind: .badUsage, message: "windows needs --pid <pid>", hint: usage)
            }
            return .windows(pid: pid)

        case "windowid":
            guard let pid = options.pid else {
                throw DriveError(
                    kind: .badUsage, message: "windowid needs --pid <pid>", hint: usage)
            }
            return .windowid(pid: pid)

        case "menu":
            guard let pid = options.pid else {
                throw DriveError(
                    kind: .badUsage, message: "menu needs --pid <pid>", hint: usage)
            }
            return .menu(
                pid: pid,
                options: TreeOptions(
                    pid: pid,
                    identifierPrefix: options.identifier,
                    maxMatches: options.matches ?? defaultMatches,
                    maxDepth: options.depth ?? defaultDepth,
                    // A menu bar is a couple of hundred elements and every one of
                    // them is a thing you might press, so the default that keeps a
                    // thousand-row list readable would only hide half the verbs.
                    maxSiblings: options.siblings ?? 0,
                    frames: options.frames
                )
            )

        case "act":
            guard let pid = options.pid else {
                throw DriveError(kind: .badUsage, message: "act needs --pid <pid>", hint: usage)
            }
            guard let json = options.json else {
                throw DriveError(
                    kind: .badUsage, message: "act needs --json '<step>'", hint: usage)
            }

            let step: Step
            do {
                step = try JSONDecoder().decode(Step.self, from: Data(json.utf8))
            } catch {
                throw DriveError(
                    kind: .badUsage,
                    message: "could not read the step: \(error)",
                    hint: #"a step is a single-key object, e.g. {"select":{"identifier":"…"}}"#
                )
            }

            return .act(step: step, pid: pid)

        case "pixels":
            guard let image = options.image else {
                throw DriveError(
                    kind: .badUsage, message: "pixels needs --image <path>", hint: usage)
            }
            guard let scan = options.scan else {
                throw DriveError(
                    kind: .badUsage,
                    message: "pixels needs --scan row or --scan column",
                    hint: usage
                )
            }
            guard let at = options.at else {
                throw DriveError(
                    kind: .badUsage, message: "pixels needs --at <n>", hint: usage)
            }

            return .pixels(
                PixelOptions(
                    image: image,
                    axis: scan,
                    at: at,
                    from: options.from,
                    to: options.to
                )
            )

        default:
            throw DriveError(
                kind: .badUsage,
                message: "unknown subcommand '\(subcommand)'",
                hint: usage
            )
        }
    }

    /// Flags accepted by any subcommand, whether or not that subcommand reads
    /// them. Keeping one parser means `--pid` behaves identically everywhere.
    private struct Options {
        var pid: pid_t?
        var depth: Int?
        var siblings: Int?
        var matches: Int?
        var settable = false
        var identifier: String?
        var json: String?
        var frames = false
        var image: String?
        var scan: PixelOptions.Axis?
        var at: Int?
        var from: Int?
        var to: Int?
        var set: String?
    }

    private static func options(_ arguments: ArraySlice<String>) throws(DriveError) -> Options {
        var options = Options()
        var rest = arguments.makeIterator()

        while let argument = rest.next() {
            switch argument {
            case "--pid":
                guard let value = rest.next(),
                    let raw = Int(value),
                    let pid = pid_t(exactly: raw)
                else {
                    throw DriveError(
                        kind: .badUsage,
                        message: "--pid takes an integer process id",
                        hint: """
                            \(usage). `--pid $(pgrep -f JP.app)` expands to nothing \
                            when the app is not running, which lands here rather \
                            than reporting app_not_running
                            """
                    )
                }
                options.pid = pid

            case "--depth":
                guard let value = rest.next(), let depth = Int(value), depth > 0 else {
                    throw DriveError(
                        kind: .badUsage,
                        message: "--depth takes a positive integer",
                        hint: usage
                    )
                }
                options.depth = depth

            case "--max-siblings":
                guard let value = rest.next(), let siblings = Int(value), siblings >= 0 else {
                    throw DriveError(
                        kind: .badUsage,
                        message:
                            "--max-siblings takes a non-negative integer, where 0 means all",
                        hint: usage
                    )
                }
                options.siblings = siblings

            case "--settable":
                options.settable = true

            case "--max-matches":
                guard let value = rest.next(), let matches = Int(value), matches > 0 else {
                    throw DriveError(
                        kind: .badUsage,
                        message: "--max-matches takes a positive integer",
                        hint: usage
                    )
                }
                options.matches = matches

            case "--frames":
                options.frames = true

            case "--identifier":
                guard let value = rest.next() else {
                    throw DriveError(
                        kind: .badUsage,
                        message: "--identifier takes a value",
                        hint: usage
                    )
                }
                options.identifier = value

            case "--set":
                guard let value = rest.next() else {
                    throw DriveError(
                        kind: .badUsage,
                        message: "--set takes a value",
                        hint: usage
                    )
                }
                options.set = value

            case "--json":
                guard let value = rest.next() else {
                    throw DriveError(
                        kind: .badUsage,
                        message: "--json takes a value",
                        hint: usage
                    )
                }
                options.json = value

            case "--image":
                guard let value = rest.next() else {
                    throw DriveError(
                        kind: .badUsage,
                        message: "--image takes a path",
                        hint: usage
                    )
                }
                options.image = value

            case "--scan":
                guard let value = rest.next(), let axis = PixelOptions.Axis(rawValue: value)
                else {
                    throw DriveError(
                        kind: .badUsage,
                        message: "--scan takes `row` or `column`",
                        hint: usage
                    )
                }
                options.scan = axis

            case "--at":
                guard let value = rest.next(), let at = Int(value), at >= 0 else {
                    throw DriveError(
                        kind: .badUsage,
                        message: "--at takes a non-negative integer",
                        hint: usage
                    )
                }
                options.at = at

            case "--from":
                guard let value = rest.next(), let from = Int(value), from >= 0 else {
                    throw DriveError(
                        kind: .badUsage,
                        message: "--from takes a non-negative integer",
                        hint: usage
                    )
                }
                options.from = from

            case "--to":
                guard let value = rest.next(), let to = Int(value), to >= 0 else {
                    throw DriveError(
                        kind: .badUsage,
                        message: "--to takes a non-negative integer",
                        hint: usage
                    )
                }
                options.to = to

            default:
                throw DriveError(
                    kind: .badUsage,
                    message: "unknown argument '\(argument)'",
                    hint: usage
                )
            }
        }

        return options
    }
}
