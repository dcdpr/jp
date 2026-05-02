//! Base HTML shell shared by all pages.

use maud::{DOCTYPE, Markup, html};

/// Wrap page content in the common HTML shell.
#[expect(
    clippy::needless_pass_by_value,
    reason = "maud templates consume Markup"
)]
pub(crate) fn page(title: &str, body: Markup) -> Markup {
    html! {
        (DOCTYPE)
        html lang="en" {
            head {
                meta charset="utf-8";
                meta name="viewport" content="width=device-width, initial-scale=1";
                title { (title) " - JP" }
                link rel="stylesheet" href="/assets/style.css";
            }
            body {
                (body)
            }
        }
    }
}

/// Render an error page.
pub(crate) fn error_page(title: &str, message: &str) -> Markup {
    page(title, html! {
        main class="error-page" {
            h1 { (title) }
            p { (message) }
            a href="/conversations" { "← Back to conversations" }
        }
    })
}
