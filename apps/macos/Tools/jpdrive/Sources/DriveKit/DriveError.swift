import Foundation

/// A failure reported as JSON on stdout, alongside a non-zero exit status.
///
/// Every exit path produces either a result document or one of these, so a
/// caller never has to scrape prose off stderr to find out what happened.
struct DriveError: Error, Encodable {
    /// Machine-readable discriminator. Callers switch on this; the message is
    /// for humans and may be reworded freely.
    enum Kind: String, Encodable {
        /// The command line named an unknown subcommand or was missing a value.
        case badUsage = "bad_usage"

        /// No process is running under the given pid.
        case appNotRunning = "app_not_running"

        /// The accessibility API refused the request for want of a TCC grant.
        case notPermitted = "not_permitted"

        /// No element carries the identifier the step addressed.
        case identifierNotFound = "identifier_not_found"

        /// The addressed element and its nearest ancestors do not accept a write
        /// to `AXSelected`.
        case notSelectable = "not_selectable"

        /// An attribute write was refused by the accessibility API.
        case writeFailed = "write_failed"

        /// The addressed element does not accept a write to its value.
        case notEditable = "not_editable"

        /// The addressed element reports nowhere on screen to click.
        case notClickable = "not_clickable"

        /// The addressed element does not accept the action the step performs.
        case actionUnsupported = "action_unsupported"

        /// The addressed element is present but refuses to act while disabled.
        case disabled = "disabled"

        /// An action was refused by the accessibility API.
        case actionFailed = "action_failed"

        /// An element waited for did not appear in time.
        case timeout = "timeout"

        /// The application reports no element of the requested kind.
        case notFound = "not_found"

        /// The accessibility API refused to read part of the tree, so what it
        /// holds is unknown rather than known to be absent.
        case readFailed = "read_failed"

        /// The result could not be encoded as JSON.
        case encodingFailed = "encoding_failed"
    }

    let kind: Kind

    /// One sentence saying what went wrong.
    let message: String

    /// What the operator can do about it, when there is something to do.
    var hint: String?
}

extension DriveError {
    /// Names the System Settings pane that grants Accessibility.
    ///
    /// macOS attributes the grant to the responsible process, which for a
    /// command-line tool is normally the terminal rather than the tool, so this
    /// points at the terminal and not at `jpdrive`.
    static let accessibilityHint = """
        grant Accessibility to the terminal application running this command, \
        under System Settings > Privacy & Security > Accessibility, then start \
        a new terminal session
        """
}

/// Envelope that makes an error document distinguishable from a result document
/// by its top-level key alone.
struct ErrorDocument: Encodable {
    let error: DriveError
}
