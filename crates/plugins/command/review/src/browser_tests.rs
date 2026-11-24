use pretty_assertions::assert_eq;

use super::*;

#[test]
fn each_system_opens_a_url_its_own_way() {
    let url = "http://127.0.0.1:3001/reviews/R-1h4k2m9";

    assert_eq!(opener("macos", url), Some(("open", vec![url.to_owned()])));
    assert_eq!(
        opener("linux", url),
        Some(("xdg-open", vec![url.to_owned()]))
    );
    assert_eq!(
        opener("windows", url),
        Some(("rundll32", vec![
            "url.dll,FileProtocolHandler".to_owned(),
            url.to_owned()
        ]))
    );
    assert_eq!(opener("wasi", url), None);
}
