import Testing

@testable import DriveKit

@Suite("Arguments")
struct ArgumentsTests {
    @Test("doctor takes an optional pid")
    func doctorPid() throws {
        guard case .doctor(let pid) = try Arguments.parse(["doctor", "--pid", "42"]) else {
            Issue.record("expected a doctor command")
            return
        }
        #expect(pid == 42)

        guard case .doctor(let none) = try Arguments.parse(["doctor"]) else {
            Issue.record("expected a doctor command")
            return
        }
        #expect(none == nil)
    }

    @Test("tree defaults its bounds")
    func treeDefaults() throws {
        guard case .tree(let options) = try Arguments.parse(["tree", "--pid", "42"]) else {
            Issue.record("expected a tree command")
            return
        }

        #expect(options.pid == 42)
        #expect(options.identifierPrefix == nil)
        #expect(options.maxMatches == Arguments.defaultMatches)
        #expect(options.maxDepth == Arguments.defaultDepth)
        #expect(options.maxSiblings == Arguments.defaultSiblings)
        #expect(!options.frames)
    }

    @Test("tree takes every bound")
    func treeFlags() throws {
        let parsed = try Arguments.parse([
            "tree", "--pid", "42", "--identifier", "sidebar.", "--max-matches", "1",
            "--depth", "3", "--max-siblings", "0", "--frames",
        ])

        guard case .tree(let options) = parsed else {
            Issue.record("expected a tree command")
            return
        }

        #expect(options.identifierPrefix == "sidebar.")
        #expect(options.maxMatches == 1)
        #expect(options.maxDepth == 3)
        #expect(options.maxSiblings == 0)
        #expect(options.frames)
    }

    /// Zero means "every sibling", which is a different thing from the cap being
    /// unset, so it has to survive parsing rather than be rejected as non-positive.
    @Test("max-siblings accepts zero for no cap")
    func zeroSiblingsIsAllowed() throws {
        guard
            case .dump(let options) = try Arguments.parse([
                "dump", "--pid", "1", "--max-siblings", "0",
            ])
        else {
            Issue.record("expected a dump command")
            return
        }

        #expect(options.maxSiblings == 0)
    }

    @Test("windows takes only a pid")
    func windowsPid() throws {
        guard case .windows(let pid) = try Arguments.parse(["windows", "--pid", "42"]) else {
            Issue.record("expected a windows command")
            return
        }
        #expect(pid == 42)
    }

    @Test("windowid takes only a pid")
    func windowidPid() throws {
        guard case .windowid(let pid) = try Arguments.parse(["windowid", "--pid", "42"]) else {
            Issue.record("expected a windowid command")
            return
        }
        #expect(pid == 42)
    }

    /// A menu bar is small and every item in it is a thing to press, so the sibling
    /// cap that keeps a thousand-row list readable would only hide verbs here.
    @Test("menu walks every sibling by default")
    func menuHasNoSiblingCap() throws {
        guard case .menu(let pid, let options) = try Arguments.parse(["menu", "--pid", "42"])
        else {
            Issue.record("expected a menu command")
            return
        }

        #expect(pid == 42)
        #expect(options.maxSiblings == 0)
    }

    @Test("act decodes a step")
    func actStep() throws {
        let parsed = try Arguments.parse([
            "act", "--pid", "42", "--json", #"{"select":{"identifier":"sidebar.row.7"}}"#,
        ])

        guard case .act(let step, let pid) = parsed else {
            Issue.record("expected an act command")
            return
        }

        #expect(pid == 42)
        guard case .select(let target) = step else {
            Issue.record("expected a select step")
            return
        }
        #expect(target.identifier == "sidebar.row.7")
    }

    /// Every field of a step is spelled the way the tool definition documents it,
    /// and a mismatch is silent: an unrecognised key decodes as absent, so a wait
    /// given a short timeout would wait the default instead and the run would look
    /// merely slow.
    @Test("act decodes every field of a wait")
    func actWaitFields() throws {
        let parsed = try Arguments.parse([
            "act", "--pid", "42", "--json",
            #"{"wait_for":{"identifier":"transcript.scroll","under":"sidebar.list","timeout_ms":1500,"interval_ms":25}}"#,
        ])

        guard case .act(let step, _) = parsed, case .waitFor(let target) = step else {
            Issue.record("expected a wait_for step")
            return
        }

        #expect(target.identifier == "transcript.scroll")
        #expect(target.under == "sidebar.list")
        #expect(target.timeoutMs == 1500)
        #expect(target.intervalMs == 25)
    }

    @Test(
        "a step naming no known verb is rejected",
        arguments: [
            #"{"nope":{"identifier":"x"}}"#,
            #"{"wait":{"identifier":"x"}}"#,
            #"{}"#,
            #"not json"#,
            #"{"select":{}}"#,
        ]
    )
    func rejectsAMalformedStep(json: String) throws {
        let error = try #require(throws: DriveError.self) {
            try Arguments.parse(["act", "--pid", "1", "--json", json])
        }

        #expect(error.kind == .badUsage)
    }

    @Test(
        "a command missing its pid is rejected",
        arguments: [
            ["tree"], ["dump"], ["windows"], ["windowid"], ["menu"], ["act", "--json", "{}"],
        ]
    )
    func requiresAPid(arguments: [String]) throws {
        let error = try #require(throws: DriveError.self) {
            try Arguments.parse(arguments)
        }

        #expect(error.kind == .badUsage)
    }

    /// An empty command substitution is the shape this most often takes:
    /// `--pid $(pgrep -f JP.app)` expands to nothing when the app is not running,
    /// leaving the flag with no value.
    @Test("a pid flag with no value is rejected with a usable hint")
    func rejectsAMissingPidValue() throws {
        let error = try #require(throws: DriveError.self) {
            try Arguments.parse(["doctor", "--pid"])
        }

        #expect(error.kind == .badUsage)
        #expect(error.hint?.contains("pgrep") == true)
    }

    @Test(
        "unknown input is rejected",
        arguments: [["fly", "--pid", "1"], ["tree", "--pid", "1", "--nope"], []]
    )
    func rejectsUnknownInput(arguments: [String]) throws {
        let error = try #require(throws: DriveError.self) {
            try Arguments.parse(arguments)
        }

        #expect(error.kind == .badUsage)
    }
}
