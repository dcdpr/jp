import Foundation

/// Writes the driver's JSON documents.
///
/// Both results and errors go to stdout, so a caller reads one stream and
/// distinguishes the two by the top-level `error` key or by the exit status.
enum Output {
    /// Encode `value` as pretty JSON on stdout, with a trailing newline.
    static func write(_ value: some Encodable) throws(DriveError) {
        let encoder = JSONEncoder()
        encoder.outputFormatting = [.prettyPrinted, .sortedKeys, .withoutEscapingSlashes]

        // Snake case on the wire, matching every other JSON payload JP produces.
        encoder.keyEncodingStrategy = .convertToSnakeCase

        let data: Data
        do {
            data = try encoder.encode(value)
        } catch {
            throw DriveError(
                kind: .encodingFailed,
                message: "could not encode the result as JSON: \(error)",
                hint: nil
            )
        }

        FileHandle.standardOutput.write(data)
        FileHandle.standardOutput.write(Data("\n".utf8))
    }

    /// Write an error document.
    ///
    /// Falls back to hand-built JSON, so a caller still gets something parseable
    /// in the case where even the error will not encode.
    static func writeError(_ error: DriveError) {
        do throws(DriveError) {
            try write(ErrorDocument(error: error))
        } catch {
            let message = error.message.replacingOccurrences(of: "\"", with: "'")
            let json = #"{"error":{"kind":"encoding_failed","message":"\#(message)"}}"# + "\n"
            FileHandle.standardOutput.write(Data(json.utf8))
        }
    }
}
