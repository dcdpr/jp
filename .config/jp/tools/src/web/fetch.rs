use url::Url;

use crate::Error;

pub(crate) async fn web_fetch(url: Url) -> std::result::Result<String, Error> {
    reqwest::get(url).await?.text().await.map_err(Into::into)
}
