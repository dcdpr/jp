use tracing::trace;
use url::Url;

use crate::error::Result;

pub(crate) fn attachment_url(uri: &str) -> Result<Url> {
    let uri = if let Ok(uri) = Url::parse(uri) {
        uri
    } else {
        // Special case for file attachments
        trace!(uri, "URI is not a valid URL, treating as file path.");
        let s = if let Some(uri) = uri.strip_prefix('!') {
            format!("file:{uri}?exclude=true")
        } else {
            format!("file:{uri}")
        };

        Url::parse(&s)?
    };

    Ok(uri)
}
