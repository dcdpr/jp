//! Base HTML shell shared by all pages.

use maud::{DOCTYPE, Markup, html};

use crate::style;

/// How a page handles being taller than the window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Scroll {
    /// The document scrolls, as a plain web page does.
    ///
    /// Gets the platform's scrolling behaviour for free — on iOS that includes
    /// tapping the status bar to return to the top, which is not an event a
    /// page can subscribe to.
    /// It only works when there is a window scroll position for the system to
    /// reset.
    Document,

    /// The document is fixed to the window and something inside it scrolls.
    ///
    /// Required wherever a virtual keyboard is involved: with no page scroll
    /// there is nothing for iOS to pan when a field is focused, which is what
    /// keeps the header from sliding off the top.
    /// The cost is the platform gestures above.
    Inner,
}

/// Wrap page content in the common HTML shell.
pub(crate) fn page(title: &str, body: Markup) -> Markup {
    shell(title, Scroll::Inner, body)
}

/// [`page`], for a page that lets the document scroll.
pub(crate) fn scrolling_page(title: &str, body: Markup) -> Markup {
    shell(title, Scroll::Document, body)
}

#[expect(
    clippy::needless_pass_by_value,
    reason = "maud templates consume Markup"
)]
fn shell(title: &str, scroll: Scroll, body: Markup) -> Markup {
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
            body class=[(scroll == Scroll::Document).then_some("scrolls")] {
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
