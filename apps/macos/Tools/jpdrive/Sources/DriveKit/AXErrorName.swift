import ApplicationServices

extension AXError {
    /// A stable snake_case name for this error.
    ///
    /// Only the codes a read or an action can realistically produce are named.
    /// Anything else keeps its numeric code rather than being flattened into
    /// "unknown", so an unexpected failure stays traceable to a header.
    var name: String {
        switch self {
        case .success: return "success"
        case .apiDisabled: return "api_disabled"
        case .cannotComplete: return "cannot_complete"
        case .invalidUIElement: return "invalid_ui_element"
        case .notImplemented: return "not_implemented"
        case .attributeUnsupported: return "attribute_unsupported"
        case .actionUnsupported: return "action_unsupported"
        case .noValue: return "no_value"
        case .illegalArgument: return "illegal_argument"
        case .failure: return "failure"
        default: return "ax_error_\(rawValue)"
        }
    }
}
