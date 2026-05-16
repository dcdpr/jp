use std::{fs, sync::LazyLock};

use scraper::{Html, Selector};
use url::Url;

use crate::{dl, error::Error, query::GLOBAL_CLIENT};

static PRE_RUST: LazyLock<Selector> =
    LazyLock::new(|| Selector::parse("pre.rust").expect("static selector"));

/// Get the source resource for a crate.
pub async fn get_crate_source_resource(uri: &Url) -> Result<String, Error> {
    let dl_cfg = dl::Config::try_from(uri)?
        .root(&GLOBAL_CLIENT.crates_path)
        .client(GLOBAL_CLIENT.http_client.clone());

    let root = dl::download(dl_cfg).await?;

    // Convert from `/0.1.0/src/lib.rs` to `src/lib.rs`. Uri is guaranteed to
    // be valid, since we parsed it in `Config::try_from`.
    let path = &uri.path()[1..]
        .split_once('/')
        .map_or(uri.path(), |(_, v)| v);

    let html = fs::read_to_string(root.join(path))?;
    let document = Html::parse_document(&html);

    // rustdoc renders the source code inside `<pre class="rust">`. `.text()`
    // strips all the syntax-highlighting span wrappers and returns plain text.
    let source = document
        .select(&PRE_RUST)
        .next()
        .map(|el| el.text().collect::<String>())
        .unwrap_or_default();

    // The source is plain text, but rustdoc prefixes each line with a line
    // number followed by the source code. Strip the line numbers and skip
    // anything that doesn't start with a digit (e.g. line-number separators).
    let mut clean_source = String::new();
    for line in source.lines() {
        if !line.starts_with(|c: char| c.is_ascii_digit()) {
            continue;
        }

        let line = line.trim_start_matches(|c: char| c.is_ascii_digit());
        clean_source.push_str(line);
        clean_source.push('\n');
    }

    Ok(clean_source)
}
