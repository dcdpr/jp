import Foundation
import Testing

@testable import JP

@Suite("ConversationDate")
struct ConversationDateTests {
    /// UTC, so a fixed timestamp lands on the same calendar day wherever the test
    /// runs. A machine in Auckland would otherwise read "12:30Z on 2 September" as
    /// a different day from one in Los Angeles, and the same-day branch is the
    /// whole point of half these tests.
    private static let calendar: Calendar = {
        var calendar = Calendar(identifier: .gregorian)
        guard let utc = TimeZone(identifier: "UTC") else { return calendar }
        calendar.timeZone = utc
        return calendar
    }()

    /// Fixed, because a formatted date's word order and month name are the
    /// locale's: "2 Sept" in one place, "Sep 2" in another.
    private static let locale = Locale(identifier: "en_GB")

    /// The instant `text` names, or a failure naming what would not parse.
    private func date(_ text: String) throws -> Date {
        try #require(ConversationDate.parse(text), "\(text) did not parse")
    }

    private func label(_ text: String, now: String) throws -> String {
        ConversationDate.label(
            for: try date(text),
            now: try date(now),
            calendar: Self.calendar,
            locale: Self.locale
        )
    }

    @Test("parses a whole-second timestamp")
    func parsesWholeSeconds() throws {
        #expect(try date("2024-09-02T12:30:00Z").timeIntervalSince1970 == 1_725_280_200)
    }

    /// Any conversation JP created from a wall clock carries sub-second
    /// precision, so this is the shape the app sees in practice.
    @Test("parses a timestamp with fractional seconds")
    func parsesFractionalSeconds() throws {
        let parsed = try date("2024-09-02T12:30:00.500000Z")

        #expect(parsed.timeIntervalSince1970 == 1_725_280_200.5)
    }

    @Test("reports a timestamp it cannot read")
    func rejectsNonsense() {
        #expect(ConversationDate.parse("") == nil)
        #expect(ConversationDate.parse("yesterday") == nil)
        #expect(ConversationDate.parse("2024-09-02") == nil)
    }

    @Test("says how long ago a conversation active today was")
    func minutesAgoToday() throws {
        #expect(
            try label("2026-08-03T09:39:00Z", now: "2026-08-03T10:00:00Z") == "21 minutes ago")
        #expect(
            try label("2026-08-03T09:59:00Z", now: "2026-08-03T10:00:00Z") == "1 minute ago")
        #expect(try label("2026-08-03T08:00:00Z", now: "2026-08-03T10:00:00Z") == "2 hours ago")
        #expect(try label("2026-08-03T09:00:00Z", now: "2026-08-03T10:00:00Z") == "1 hour ago")
    }

    /// Under a minute has no useful number to show, and a clock adjustment can
    /// put a stored timestamp slightly in the future.
    @Test("says just now for anything under a minute, in either direction")
    func justNow() throws {
        #expect(try label("2026-08-03T09:59:30Z", now: "2026-08-03T10:00:00Z") == "just now")
        #expect(try label("2026-08-03T10:00:30Z", now: "2026-08-03T10:00:00Z") == "just now")
    }

    /// Yesterday is a different day even when it is only minutes ago, because a
    /// row saying "40 minutes ago" for something dated yesterday reads as wrong.
    @Test("dates a conversation from another day rather than timing it")
    func earlierThisYear() throws {
        #expect(try label("2026-08-02T23:40:00Z", now: "2026-08-03T00:20:00Z") == "2 Aug")
        #expect(try label("2026-05-13T09:00:00Z", now: "2026-08-03T10:00:00Z") == "13 May")
    }

    /// Without the year, a conversation from last July and one from this July
    /// read identically.
    ///
    /// May, like the months the tests above use, is abbreviated the same way by
    /// every ICU version. September is not — `Sep` and `Sept` are both current —
    /// and pinning one of those would make this test an OS-update tripwire
    /// rather than a check on the format.
    @Test("adds the year for a conversation from an earlier one")
    func earlierYear() throws {
        #expect(try label("2024-05-13T09:00:00Z", now: "2026-08-03T10:00:00Z") == "13 May 2024")
    }

    @Test("dates a conversation from its summary")
    func labelsASummary() throws {
        let conversation = ConversationSummary(
            id: "17251488000",
            title: "Reading list",
            lastActivatedAt: "2026-05-13T09:00:00Z",
            pinnedAt: nil,
            eventsCount: 3
        )

        #expect(
            ConversationDate.activityLabel(
                for: conversation,
                now: try date("2026-08-03T10:00:00Z"),
                calendar: Self.calendar,
                locale: Self.locale
            ) == "13 May"
        )
    }

    /// A row shows no date rather than error text when the library reports
    /// something this cannot read.
    @Test("dates nothing when the summary's timestamp will not parse")
    func labelsAnUnreadableSummary() throws {
        let conversation = ConversationSummary(
            id: "17251488000",
            title: "Reading list",
            lastActivatedAt: "not a timestamp",
            pinnedAt: nil,
            eventsCount: 3
        )

        #expect(
            ConversationDate.activityLabel(
                for: conversation,
                now: try date("2026-08-03T10:00:00Z"),
                calendar: Self.calendar,
                locale: Self.locale
            ) == nil
        )
    }
}
