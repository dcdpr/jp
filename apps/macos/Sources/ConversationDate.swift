import Foundation

/// Turns the timestamps the library reports into the dates a row shows.
///
/// Pure, and given its reference instant rather than reading a clock, so what a
/// row reads at any moment can be pinned without a running app.
enum ConversationDate {
    /// The instant `text` names, or `nil` if it is not a timestamp the library
    /// emits.
    ///
    /// Two shapes are accepted because the library emits both: it keeps whatever
    /// sub-second precision a conversation was stored with, so a conversation JP
    /// created from a wall clock carries a fractional-seconds part and one
    /// written by hand usually does not.
    static func parse(_ text: String) -> Date? {
        if let date = try? whole.parse(text) {
            return date
        }

        return try? fractional.parse(text)
    }

    /// How a row dates `conversation`, or `nil` if its timestamp will not parse.
    ///
    /// A row that cannot date itself shows no date rather than showing a
    /// placeholder: the date is a convenience beside the title, and a row of
    /// error text where a person expects "31 Jul" is worse than a gap.
    static func activityLabel(
        for conversation: ConversationSummary,
        now: Date,
        calendar: Calendar = .current,
        locale: Locale = .current
    ) -> String? {
        guard let date = parse(conversation.lastActivatedAt) else { return nil }

        return label(for: date, now: now, calendar: calendar, locale: locale)
    }

    /// How a row labels `date`.
    ///
    /// Three forms, by how far back it is: how long ago on the day it happened
    /// ("21 minutes ago"), the day and month inside the same year ("31 Jul"),
    /// and the year as well before that ("13 May 2024").
    ///
    /// The order of day and month is the locale's, so a reader gets the one they
    /// expect.
    static func label(
        for date: Date,
        now: Date,
        calendar: Calendar = .current,
        locale: Locale = .current
    ) -> String {
        if calendar.isDate(date, inSameDayAs: now) {
            return elapsed(from: date, to: now)
        }

        let dayAndMonth = Date.FormatStyle(
            locale: locale,
            calendar: calendar,
            timeZone: calendar.timeZone
        )
        .day().month(.abbreviated)

        guard
            calendar.component(.year, from: date) == calendar.component(.year, from: now)
        else {
            return date.formatted(dayAndMonth.year())
        }

        return date.formatted(dayAndMonth)
    }

    /// How long before `now` the conversation was active, in the largest unit
    /// that gives a whole number.
    ///
    /// Only ever called for two instants on the same day, so hours is the
    /// coarsest unit it needs. Anything under a minute, and anything a clock
    /// adjustment has put in the future, reads as just now.
    private static func elapsed(from date: Date, to now: Date) -> String {
        let seconds = Int(now.timeIntervalSince(date))
        guard seconds >= 60 else { return "just now" }

        let minutes = seconds / 60
        guard minutes >= 60 else {
            return minutes == 1 ? "1 minute ago" : "\(minutes) minutes ago"
        }

        let hours = minutes / 60
        return hours == 1 ? "1 hour ago" : "\(hours) hours ago"
    }

    /// Parses `2024-09-02T12:30:00Z`.
    ///
    /// A format style rather than an `ISO8601DateFormatter`, because this is a
    /// `Sendable` value and the formatter is a reference type that cannot be
    /// held in a `static let` under strict concurrency checking.
    private static let whole = Date.ISO8601FormatStyle(includingFractionalSeconds: false)

    /// Parses `2024-09-02T12:30:00.123456Z`.
    private static let fractional = Date.ISO8601FormatStyle(includingFractionalSeconds: true)
}
