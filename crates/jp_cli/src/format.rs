pub(crate) mod conversation;
pub(crate) mod datetime;

use jp_config::types::color::Color;
use jp_term::table::DetailItem;
use serde_json::json;
use url::Url;

/// Build a list item for an attachment URL.
///
/// The terminal text reads as `scheme (description): url` when the attachment
/// carries a `description` query parameter, and as the bare URL otherwise.
/// The JSON form is always an object with `scheme`, `description` (null when
/// absent), and the canonical `url`.
pub(crate) fn attachment_detail_item(url: &Url) -> DetailItem {
    let scheme = url.scheme();
    let description = url
        .query_pairs()
        .find(|(key, _)| key == "description")
        .map(|(_, value)| value.into_owned());
    let url_str = url.to_string();

    let text = match &description {
        Some(description) => format!("{scheme} ({description}): {url_str}"),
        None => url_str.clone(),
    };

    DetailItem::new(
        text,
        json!({
            "scheme": scheme,
            "description": description,
            "url": url_str,
        }),
    )
}

/// Convert a [`Color`] to an SGR background parameter string.
pub(crate) fn color_to_bg_param(color: Color) -> String {
    match color {
        Color::Ansi256(n) => format!("48;5;{n}"),
        Color::Rgb { r, g, b } => format!("48;2;{r};{g};{b}"),
    }
}
