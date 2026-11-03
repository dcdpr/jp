# Let web_fetch download PDFs through the headless browser

- **Status**: Todo
- **Authors**: jp
- **Date**: 2026-11-03
- **Label**: domain=tooling
- **Label**: type=enhancement

`web_fetch` converts PDFs with poppler, but only when a plain HTTP request gets
the bytes.
A PDF behind a bot challenge (Anubis, Cloudflare) can't be fetched at all.

The headless pipeline can't help as it stands.
It runs `chrome --headless --dump-dom <url>`, and a PDF has no DOM: tried
against `https://www.rfc-editor.org/rfc/rfc9562.pdf`, `chrome-headless-shell`
waited ~50s, logged `Unexpected renderer destruction`, and printed nothing.
No command-line flag makes it print the downloaded bytes.

Getting them means driving the browser over the DevTools protocol: navigate
(which clears the challenge), then fetch the PDF through the browser's network
stack, or capture the download, and hand the bytes to `pdf::render`, which takes
bytes and needs no change.

Today `.pdf` URLs skip headless entirely, and the empty-document error tells the
assistant a download may be the cause.
Before building this, confirm that blocked PDF downloads actually happen; bot
walls usually sit in front of HTML pages, and PDFs are often served from static
hosts.
