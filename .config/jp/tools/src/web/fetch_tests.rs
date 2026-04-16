use super::*;

mod is_binary {
    use super::*;

    #[test]
    fn image_types() {
        assert!(is_binary("image/png"));
        assert!(is_binary("image/jpeg"));
        assert!(is_binary("Image/PNG"));
    }

    #[test]
    fn audio_video() {
        assert!(is_binary("audio/mpeg"));
        assert!(is_binary("video/mp4"));
    }

    #[test]
    fn application_types() {
        assert!(is_binary("application/octet-stream"));
        assert!(is_binary("application/pdf"));
        assert!(is_binary("application/zip"));
    }

    #[test]
    fn text_types_are_not_binary() {
        assert!(!is_binary("text/html; charset=utf-8"));
        assert!(!is_binary("text/plain"));
        assert!(!is_binary("application/json"));
        assert!(!is_binary("application/xml"));
    }
}

mod truncate {
    use super::*;

    #[test]
    fn under_limit_unchanged() {
        let s = "short content";
        assert_eq!(truncate(s, 100), s);
    }

    #[test]
    fn over_limit_truncates_with_note() {
        let s = "a".repeat(50);
        let result = truncate(&s, 20);
        assert!(result.starts_with("aaaaaaaaaaaaaaaaaaaa"));
        assert!(result.contains("[Content truncated:"));
        assert!(result.contains("20 of 50 bytes"));
    }

    #[test]
    fn exact_limit() {
        let s = "exactly10!";
        assert_eq!(truncate(s, 10), s);
    }
}
