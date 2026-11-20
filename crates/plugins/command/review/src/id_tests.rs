use pretty_assertions::assert_eq;

use super::*;

#[test]
fn a_comment_reads_its_review_in_any_spelling() {
    for text in ["R-1h4k2m9#2", "r1h4k2m9#2", "1h4k2m9#2", " R-1h4k2m9#2 "] {
        let comment: CommentRef = text.parse().unwrap();

        assert_eq!(comment.to_string(), "R-1h4k2m9#2", "{text}");
        assert_eq!(comment.number, 2);
    }
}

#[test]
fn a_comment_needs_a_review_and_a_positive_number() {
    for text in [
        "R-1h4k2m9",
        "R-1h4k2m9#",
        "R-1h4k2m9#0",
        "R-1h4k2m9#-1",
        "#2",
        "R-1h4#2",
    ] {
        let error = text.parse::<CommentRef>().unwrap_err();

        assert_eq!(
            error,
            format!("`{text}` is not a comment (try R-1h4k2m9#2)."),
            "{text}"
        );
    }
}

#[test]
fn a_comment_serializes_as_it_is_written() {
    let comment: CommentRef = "R-1h4k2m9#12".parse().unwrap();

    assert_eq!(serde_json::to_string(&comment).unwrap(), "\"R-1h4k2m9#12\"");
}

/// Tool calls carry the conversation as JP prints it, so `--id` is read into
/// the same spelling.
#[test]
fn a_conversation_is_written_the_way_jp_prints_it() {
    for text in [
        "jp-c17629384710",
        " jp-c17629384710 ",
        "jp-c17629384710-otvo8",
        "17629384710",
    ] {
        let conversation: Conversation = text.parse().unwrap();

        assert_eq!(conversation.to_string(), "jp-c17629384710", "{text}");
    }
}

#[test]
fn something_else_is_no_conversation() {
    for text in ["jp-p17629384710", "jp-cabc", "c17629384710", "0", "-5", ""] {
        assert_eq!(
            text.parse::<Conversation>().unwrap_err(),
            format!("`{text}` is not a conversation (try jp-c17629384710)."),
            "{text}"
        );
    }
}

#[test]
fn a_review_id_is_written_with_its_prefix() {
    let id: ReviewId = "1H4K2M9".parse().unwrap();

    assert_eq!(id.to_string(), "R-1h4k2m9");
    assert_eq!(id.as_str(), "1h4k2m9");
}
