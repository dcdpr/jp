//! Base HTML shell shared by all pages.

use maud::{DOCTYPE, Markup, html};

use crate::style;

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
                // `viewport-fit=cover` so the safe-area insets below have
                // something to report on a notched screen.
                meta name="viewport"
                    content="width=device-width, initial-scale=1, viewport-fit=cover";
                title { (title) " - JP" }

                // Installed to a home screen, this runs without browser chrome
                // and keeps its own history, which is what makes it usable as an
                // app rather than a bookmark.
                meta name="apple-mobile-web-app-capable" content="yes";
                meta name="apple-mobile-web-app-title" content="JP";
                meta name="apple-mobile-web-app-status-bar-style"
                    content="black-translucent";
                meta name="mobile-web-app-capable" content="yes";
                meta name="theme-color" content="#1a1a1a";

                link rel="icon" type="image/svg+xml" href="/assets/icon.svg";
                link rel="apple-touch-icon" href="/assets/icon.svg";
                link rel="manifest" href="/manifest.webmanifest";

                link rel="stylesheet"
                    href=(format!("/assets/style.css?v={}", style::css_version()));
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
