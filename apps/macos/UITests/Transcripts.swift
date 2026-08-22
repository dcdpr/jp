/// What each fixture conversation looks like once the app has drawn it.
///
/// Written out in full rather than assembled from the fixture's messages, so a
/// reader sees exactly what is on screen and a change to the transcript's shape
/// shows up here as a diff. Building these from ``ConversationFixtures`` would
/// follow a change in the app's formatting instead of catching one.
///
/// The shape: each message is its speaker's name on one line, then the message,
/// with nothing between one message and the next but a newline. The spacing a
/// reader sees is paragraph spacing, which is not in the text.
enum Transcripts {
    static let readingList = """
        Jean
        What is on the reading list?
        Assistant
        Three books and a paper.
        """

    static let configPipeline = """
        Jean
        How does the config pipeline layer?
        Assistant
        Later layers win, field by field.
        """

    static let releaseNotes = """
        Jean
        Draft the release notes.
        Assistant
        Drafted, with one open question.
        Jean
        Answer it yourself.
        Assistant
        Answered.
        """
}
