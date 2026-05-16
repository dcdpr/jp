use rusqlite::Connection;
use url::Url;

use crate::{dl, docs, docs::Item, error::Error, index, query::GLOBAL_CLIENT};

/// Get the documentation for a specific crate item.
pub async fn get_crate_item_resource(uri: &Url) -> Result<Item, Error> {
    // Convert from `/0.1.0/items/path/to/item.html` to `path/to/item.html`
    // Uri is guaranteed to be valid, since we parsed it in `Config::try_from`.
    let path = &uri.path()[1..]
        .split_once('/')
        .and_then(|(_, rest)| rest.split_once('/'))
        .map_or(uri.path(), |(_, v)| v);

    // Download the crate.
    let dl_cfg = dl::Config::try_from(uri)?
        .root(&GLOBAL_CLIENT.crates_path)
        .client(GLOBAL_CLIENT.http_client.clone());
    let root = dl::download(dl_cfg).await?;

    // Index the crate.
    let index_file = root.join("index.sqlite");
    let index_cfg = index::Config::default().source(&root).output(&index_file);
    index::index(index_cfg)?;

    // Get the item details.
    let conn = Connection::open(index_file)?;
    docs::Docs::new(root, &conn)?.item(path)
}
