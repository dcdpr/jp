import ApplicationServices
import Foundation

/// An element of a running application's accessibility tree.
///
/// Every method here is one or more synchronous round-trips to the target's main
/// thread. That cost dominates everything the driver does, so callers batch reads
/// with ``values(_:)`` rather than reading attributes one at a time, and hold onto
/// an element they will read again instead of walking to it twice.
///
/// A reference stays valid while the underlying element lives. Once it is gone,
/// reads answer `invalid_ui_element` rather than crashing.
struct AXElement {
    let element: AXUIElement

    /// The root element of the application owning `pid`.
    ///
    /// Succeeds whether or not the process exists; the first read is what fails.
    static func application(pid: pid_t) -> AXElement {
        return AXElement(element: AXUIElementCreateApplication(pid))
    }

    /// `AXRole`, or `nil` when the element does not report one.
    var role: String? {
        return read(kAXRoleAttribute).flatMap { $0 as? String }
    }

    /// `AXIdentifier`, or `nil` when the element carries none.
    ///
    /// SwiftUI's `.accessibilityIdentifier` surfaces here, but it also composites
    /// with identifiers the framework generates itself, so a value like
    /// `"workspace-AppWindow-1, SidebarNavigationSplitView"` is possible.
    var identifier: String? {
        return read(kAXIdentifierAttribute).flatMap { $0 as? String }
    }

    /// The element's human-readable label.
    ///
    /// Tries `AXAttributedDescription`, then `AXDescription`, then `AXTitle`.
    /// SwiftUI populates the first of those for list rows and leaves the others
    /// empty, while AppKit controls tend to do the reverse.
    var label: String? {
        if let attributed = read(Self.attributedDescription) as? NSAttributedString {
            return attributed.string
        }

        for name in [kAXDescriptionAttribute, kAXTitleAttribute] {
            guard let text = read(name).flatMap({ $0 as? String }), !text.isEmpty else {
                continue
            }
            return text
        }

        return nil
    }

    /// `AXAttributedDescription`, which has no constant in the SDK headers.
    static let attributedDescription = "AXAttributedDescription"

    /// `AXActivationPoint`, which has no constant in the SDK headers.
    ///
    /// Where the element says a click on it belongs, in screen coordinates. Not
    /// always the middle of its frame.
    static let activationPoint = "AXActivationPoint"

    /// The element's children, or an empty array when it has none.
    ///
    /// A round-trip of its own. A walk should take children from ``read(_:)``,
    /// which fetches them alongside everything else it needs.
    var children: [AXElement] {
        guard let value = read(kAXChildrenAttribute), let raw = value as? [AXUIElement] else {
            return []
        }
        return raw.map { AXElement(element: $0) }
    }

    /// Actions the element accepts, such as `AXPress`.
    var actions: [String] {
        var names: CFArray?
        guard AXUIElementCopyActionNames(element, &names) == .success,
            let names = names as? [String]
        else {
            return []
        }
        return names
    }

    /// Every attribute name the element advertises.
    func names() -> [String] {
        var names: CFArray?
        guard AXUIElementCopyAttributeNames(element, &names) == .success,
            let names = names as? [String]
        else {
            return []
        }
        return names
    }

    /// Read one attribute, or `nil` when the read fails or the value is absent.
    ///
    /// Use ``values(_:)`` when reading more than one: this costs a round-trip per
    /// call, which is what makes a naive tree walk take seconds.
    func read(_ name: String) -> CFTypeRef? {
        var value: CFTypeRef?
        guard AXUIElementCopyAttributeValue(element, name as CFString, &value) == .success
        else {
            return nil
        }
        guard let value, CFGetTypeID(value) != CFNullGetTypeID() else { return nil }
        return value
    }

    /// Read several attributes in one round-trip.
    ///
    /// Results are positional and the same count as `names`. An attribute that
    /// could not be read arrives as CoreFoundation's null or as an `AXValue`
    /// boxing the error, both of which ``text(_:)`` reports rather than discards.
    func values(_ names: [String]) -> [CFTypeRef?] {
        guard !names.isEmpty else { return [] }

        var raw: CFArray?
        let status = AXUIElementCopyMultipleAttributeValues(
            element,
            names as CFArray,
            AXCopyMultipleAttributeOptions(),
            &raw
        )

        guard status == .success,
            let values = raw as? [CFTypeRef],
            values.count == names.count
        else {
            return Array(repeating: nil, count: names.count)
        }

        return values
    }

    /// Whether `name` can be written on this element.
    ///
    /// A failed query reports as not settable: the accessibility API answers this
    /// for every attribute it advertises, so a failure means the element is gone
    /// or the attribute is not really there.
    func isSettable(_ name: String) -> Bool {
        var settable = DarwinBoolean(false)
        guard AXUIElementIsAttributeSettable(element, name as CFString, &settable) == .success
        else {
            return false
        }
        return settable.boolValue
    }

    /// Perform `action`, returning the API's own status.
    func perform(_ action: String) -> AXError {
        return AXUIElementPerformAction(element, action as CFString)
    }
}

extension AXElement: Element {
    /// Read the named attributes and the element's children in one round-trip.
    ///
    /// Children come back in the same batch as everything else: asking for them
    /// separately would add a hop per element, and every walk asks for them.
    func read(_ names: [String]) -> Reading<AXElement> {
        let values = self.values(names + [kAXChildrenAttribute])

        let children = (values.last.flatMap { $0 } as? [AXUIElement] ?? [])
            .map { AXElement(element: $0) }

        return Reading(
            text: values.dropLast().map(Self.optionalText),
            children: children
        )
    }

    /// Read a boolean attribute.
    ///
    /// `CFBoolean` bridges to `NSNumber` rather than to `Bool`, so a direct cast
    /// answers `nil` for a perfectly good `0` or `1`.
    func flag(_ name: String) -> Bool? {
        guard let value = read(name) as? NSNumber else { return nil }
        return value.boolValue
    }

    /// Write a boolean attribute, answering the API's own status.
    func setFlag(_ name: String, _ value: Bool) -> AXError {
        return AXUIElementSetAttributeValue(
            element,
            name as CFString,
            value ? kCFBooleanTrue : kCFBooleanFalse
        )
    }

    /// The point held in an attribute, in screen coordinates.
    func point(_ name: String) -> CGPoint? {
        guard let value = read(name), CFGetTypeID(value) == AXValueGetTypeID() else {
            return nil
        }

        let boxed = unsafeDowncast(value, to: AXValue.self)
        guard AXValueGetType(boxed) == .cgPoint else { return nil }

        var point = CGPoint.zero
        guard AXValueGetValue(boxed, .cgPoint, &point) else { return nil }

        return point
    }

    /// The size held in an attribute, in points.
    func size(_ name: String) -> CGSize? {
        guard let value = read(name), CFGetTypeID(value) == AXValueGetTypeID() else {
            return nil
        }

        let boxed = unsafeDowncast(value, to: AXValue.self)
        guard AXValueGetType(boxed) == .cgSize else { return nil }

        var size = CGSize.zero
        guard AXValueGetValue(boxed, .cgSize, &size) else { return nil }

        return size
    }

    /// Write a size attribute, answering the API's own status.
    ///
    /// The value has to be boxed in an `AXValue`: the API takes `CFTypeRef` and a
    /// bare `CGSize` is not one, so passing it any other way fails the write with
    /// no indication of why.
    func setSize(_ name: String, _ value: CGSize) -> AXError {
        var size = value
        guard let boxed = AXValueCreate(.cgSize, &size) else {
            return .failure
        }

        return AXUIElementSetAttributeValue(element, name as CFString, boxed)
    }

    /// Write a string attribute, answering the API's own status.
    func setText(_ name: String, _ value: String) -> AXError {
        return AXUIElementSetAttributeValue(element, name as CFString, value as CFString)
    }

    /// The elements held in an attribute.
    func elements(_ name: String) -> [AXElement] {
        guard let value = read(name) else { return [] }

        if let raw = value as? [AXUIElement] {
            return raw.map { AXElement(element: $0) }
        }

        guard CFGetTypeID(value) == AXUIElementGetTypeID() else { return [] }
        return [AXElement(element: unsafeDowncast(value, to: AXUIElement.self))]
    }
}

extension AXElement {
    /// Render an attribute value as text, or `nil` when there is no value.
    ///
    /// A batched read answers an absent attribute with CoreFoundation's null and an
    /// unreadable one with a boxed error. Both are facts a dump wants to see and a
    /// caller reading one attribute wants as nothing at all.
    static func optionalText(_ value: CFTypeRef?) -> String? {
        guard let value, CFGetTypeID(value) != CFNullGetTypeID() else { return nil }

        if CFGetTypeID(value) == AXValueGetTypeID(),
            AXValueGetType(unsafeDowncast(value, to: AXValue.self)) == .axError
        {
            return nil
        }

        return text(value)
    }

    /// Render an attribute value as text.
    ///
    /// Values arrive as CoreFoundation types, including geometry boxed in
    /// `AXValue` and references to other elements. Everything becomes a string so
    /// that a reader can see which attributes exist and which carry identifiers
    /// without this growing a case per boxed type.
    static func text(_ value: CFTypeRef) -> String {
        // An attribute the element advertises but cannot answer for, such as
        // `AXSubrole` on an element that has none.
        if CFGetTypeID(value) == CFNullGetTypeID() {
            return "<null>"
        }
        if let text = value as? String {
            return text
        }
        // Labels arrive as attributed strings more often than plain ones, and the
        // attributes carry nothing the driver acts on.
        if let attributed = value as? NSAttributedString {
            return attributed.string
        }
        if let number = value as? NSNumber {
            return number.stringValue
        }
        if let elements = value as? [AXUIElement] {
            return "<\(elements.count) AXUIElement>"
        }
        if let array = value as? [Any] {
            return "<array of \(array.count)>"
        }

        let typeID = CFGetTypeID(value)
        if typeID == AXUIElementGetTypeID() {
            return "<AXUIElement>"
        }
        if typeID == AXValueGetTypeID() {
            // The conditional form is rejected here: every CoreFoundation type is
            // bridged as a class, so the compiler sees a cast that cannot fail.
            // The type ID check above is the real test.
            return text(unsafeDowncast(value, to: AXValue.self))
        }
        return "<CFTypeID \(typeID)>"
    }

    /// Render the geometry boxed in an `AXValue`.
    ///
    /// `AXActivationPoint` and `AXFrame` decide where a synthesized click lands,
    /// so these arrive as numbers a reader can check against the screen rather
    /// than as an opaque marker.
    static func text(_ value: AXValue) -> String {
        let type = AXValueGetType(value)

        switch type {
        // A batched read reports a per-attribute failure by boxing the error
        // rather than by failing the whole call.
        case .axError:
            var status = AXError.success
            guard AXValueGetValue(value, .axError, &status) else { break }
            return "<\(status.name)>"

        case .cgPoint:
            var point = CGPoint.zero
            guard AXValueGetValue(value, .cgPoint, &point) else { break }
            return "\(point.x),\(point.y)"

        case .cgSize:
            var size = CGSize.zero
            guard AXValueGetValue(value, .cgSize, &size) else { break }
            return "\(size.width)x\(size.height)"

        case .cgRect:
            var rect = CGRect.zero
            guard AXValueGetValue(value, .cgRect, &rect) else { break }
            return "\(rect.origin.x),\(rect.origin.y) \(rect.size.width)x\(rect.size.height)"

        case .cfRange:
            var range = CFRange()
            guard AXValueGetValue(value, .cfRange, &range) else { break }
            return "\(range.location)+\(range.length)"

        default:
            break
        }

        return "<AXValue \(type.rawValue)>"
    }
}
