import ApplicationServices
import Testing

@testable import DriveKit

/// The menu bar's own walk is [`Tree`](Tree)'s, already covered by `TreeTests`.
/// What is specific here is the root: the menu bar hangs off an attribute of the
/// application rather than sitting in its children, and every level of it should be
/// reported rather than capped.
@Suite("Menu")
struct MenuTests {
    /// The whole menu bar, with no cap, because every item in it is something a
    /// script might press.
    private func options() -> TreeOptions {
        return TreeOptions(
            pid: 0,
            identifierPrefix: nil,
            maxMatches: 100,
            maxDepth: 20,
            maxSiblings: 0,
            frames: false
        )
    }

    /// An application whose menu bar hangs off the attribute the real one uses.
    private func app() -> FakeElement {
        let app = FakeElement(role: "AXApplication")
        app.related[kAXMenuBarAttribute] = [menuBar()]
        return app
    }

    private func menuBar() -> FakeElement {
        return FakeElement(
            role: "AXMenuBar",
            children: [
                FakeElement(
                    role: "AXMenuBarItem",
                    label: "File",
                    children: [
                        FakeElement(
                            role: "AXMenu",
                            children: [
                                FakeElement(
                                    role: "AXMenuItem",
                                    identifier: "performClose:",
                                    label: "Close",
                                    actions: ["AXCancel", "AXPress", "AXPick"]
                                ),
                                FakeElement(
                                    role: "AXMenuItem",
                                    identifier: "closeAll:",
                                    label: "Close All",
                                    actions: ["AXCancel", "AXPress", "AXPick"]
                                ),
                            ]
                        )
                    ]
                )
            ]
        )
    }

    @Test("every menu item is reported, with the action that activates it")
    func reportsEveryItem() throws {
        let tree = try #require(Tree.walk(from: menuBar(), options: options()))

        #expect(tree.role == "AXMenuBar")
        let items = try #require(tree.children.first?.children.first?.children)
        #expect(items.count == 2)
        #expect(items.map(\.identifier) == ["performClose:", "closeAll:"])
        #expect(items[0].actions.contains("AXPress"))
        #expect(tree.children.first?.children.first?.elidedChildren == nil)
    }

    /// Menu items advertise `AXPress` where a list row advertises nothing, so they
    /// are the case `press` was built for.
    @Test("a menu item can be pressed by identifier")
    func pressesAMenuItem() throws {
        let bar = menuBar()

        let result = try Act.run(.press(.init(identifier: "closeAll:")), in: bar)

        #expect(result.role == "AXMenuItem")
        let items = try #require(bar.children.first?.children.first?.children)
        #expect(items[1].performed == ["AXPress"])
        #expect(items[0].performed.isEmpty, "only the addressed item may be pressed")
    }

    /// The path names the two titled levels a user sees and skips the `AXMenu`
    /// between them, because that container has no title to name.
    @Test("a titled path resolves through the intervening menu")
    func resolvesATitledPath() throws {
        let root = app()

        let result = try Act.run(.menu(.init(path: ["File", "Close All"])), in: root)

        #expect(result.step == "menu")
        #expect(result.identifier == "File > Close All")
        #expect(result.role == "AXMenuItem")

        let bar = try #require(root.elements(kAXMenuBarAttribute).first)
        let items = try #require(bar.children.first?.children.first?.children)
        #expect(items[1].performed == ["AXPress"])
        #expect(items[0].performed.isEmpty)
    }

    /// The point of addressing by title: an item that moved to another menu keeps
    /// its identifier, so only a path notices. The failure has to say what the
    /// level does hold, or the test that catches the move cannot say what changed.
    @Test("a path that does not resolve names what the level holds")
    func reportsWhatTheLevelHolds() throws {
        let error = try #require(throws: DriveError.self) {
            try Act.run(.menu(.init(path: ["File", "Quit"])), in: app())
        }

        #expect(error.kind == .notFound)
        #expect(error.message.contains("'File' holds no item titled 'Quit'"))
        #expect(error.hint == "it holds: Close, Close All")
    }

    /// A context menu's items cannot be addressed any other way: `SwiftUI` gives
    /// every one of them the same selector name, so a title is all there is.
    @Test("a path can start at the menu an element is showing")
    func resolvesUnderAShownMenu() throws {
        let item = FakeElement(role: "AXMenuItem", label: "Copy Link", actions: ["AXPress"])
        let owner = FakeElement(
            role: "AXOutline",
            identifier: "sidebar.list",
            children: [
                FakeElement(role: "AXRow", identifier: "sidebar.row.0"),
                FakeElement(role: "AXMenu", children: [item]),
            ]
        )
        let root = FakeElement(role: "AXApplication", children: [owner])

        let result = try Act.run(
            .menu(.init(path: ["Copy Link"], under: "sidebar.list")), in: root)

        #expect(result.step == "menu")
        #expect(result.identifier == "Copy Link")
        #expect(item.performed == ["AXPress"])
    }

    /// The menu closes as soon as the application deactivates, so "press an item
    /// in it" fails far more often than "open it" does. The error has to name the
    /// step that was missed.
    @Test("a path under an element that shows no menu says how to open one")
    func reportsAnUnshownMenu() throws {
        let owner = FakeElement(
            role: "AXOutline",
            identifier: "sidebar.list",
            children: [FakeElement(role: "AXRow", identifier: "sidebar.row.0")]
        )
        let root = FakeElement(role: "AXApplication", children: [owner])

        let error = try #require(throws: DriveError.self) {
            try Act.run(.menu(.init(path: ["Copy Link"], under: "sidebar.list")), in: root)
        }

        #expect(error.kind == .notFound)
        #expect(error.message == "sidebar.list is not showing a menu")
        #expect(error.hint?.contains("AXShowMenu") == true)
    }

    @Test("a missing top-level menu is reported against the bar")
    func reportsAMissingTopLevelMenu() throws {
        let error = try #require(throws: DriveError.self) {
            try Act.run(.menu(.init(path: ["Edit", "Copy"])), in: app())
        }

        #expect(error.kind == .notFound)
        #expect(error.message.contains("the menu bar holds no item titled 'Edit'"))
        #expect(error.hint == "it holds: File")
    }

    /// Stopping at a bar item addresses the menu, not an item in it, and pressing a
    /// menu is not what the script meant.
    @Test("a path stopping at a submenu is rejected")
    func rejectsAPathToASubmenu() throws {
        let error = try #require(throws: DriveError.self) {
            try Act.run(.menu(.init(path: ["File"])), in: app())
        }

        #expect(error.kind == .actionUnsupported)
        #expect(error.hint?.contains("name the item inside it") == true)
    }

    @Test("an empty path is rejected")
    func rejectsAnEmptyPath() throws {
        let error = try #require(throws: DriveError.self) {
            try Act.run(.menu(.init(path: [])), in: app())
        }

        #expect(error.kind == .badUsage)
    }

    /// The reason every menu test would otherwise pass while the real thing did
    /// nothing: AppKit disables every item acting on the front window or the
    /// responder chain while the application is in the background, which a driven
    /// app always is.
    @Test("a menu step brings the application forward first")
    func bringsTheApplicationForward() throws {
        let root = app()

        _ = try Act.run(.menu(.init(path: ["File", "Close All"])), in: root)

        #expect(root.flag(kAXFrontmostAttribute) == true)
    }

    /// An application already in front must not be written to: the write is what
    /// steals focus, and a run of several menu steps would take it repeatedly.
    @Test("an application already in front is left alone")
    func leavesAFrontApplicationAlone() throws {
        let root = app()
        root.attributes[kAXFrontmostAttribute] = "1"
        // Any write from here on fails, so a needless one fails the step.
        root.writeStatus = .failure

        let result = try Act.run(.menu(.init(path: ["File", "Close All"])), in: root)

        #expect(result.identifier == "File > Close All")
    }

    /// A disabled item swallows `AXPress` and answers success, so a step that
    /// pressed it anyway would report having done something it did not do.
    @Test("a disabled item is refused rather than pressed")
    func refusesADisabledItem() throws {
        let root = app()
        let bar = try #require(root.elements(kAXMenuBarAttribute).first)
        let items = try #require(bar.children.first?.children.first?.children)
        items[1].attributes[kAXEnabledAttribute] = "0"

        let error = try #require(throws: DriveError.self) {
            try Act.run(
                .menu(.init(path: ["File", "Close All"])),
                in: root,
                activation: .milliseconds(1)
            )
        }

        #expect(error.kind == .disabled)
        #expect(error.message == "'File > Close All' is disabled")
        #expect(items[1].performed.isEmpty, "a disabled item must not be pressed")
    }

    /// Most elements report no `AXEnabled` at all, and reading its absence as a
    /// refusal would reject every one of them.
    @Test("an item reporting no enabled state is pressed")
    func pressesAnItemWithNoEnabledState() throws {
        let root = app()

        let result = try Act.run(
            .menu(.init(path: ["File", "Close All"])),
            in: root,
            activation: .milliseconds(1)
        )

        #expect(result.identifier == "File > Close All")
    }

    /// The menu bar comes from the application's attribute. An app without one is a
    /// real case, and it must not be reported as a missing menu item.
    @Test("an application with no menu bar says so")
    func reportsNoMenuBar() throws {
        let error = try #require(throws: DriveError.self) {
            try Act.run(.menu(.init(path: ["File"])), in: FakeElement(role: "AXApplication"))
        }

        #expect(error.kind == .notFound)
        #expect(error.message.contains("no menu bar"))
    }
}
