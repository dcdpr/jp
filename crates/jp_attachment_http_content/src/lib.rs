use std::{collections::BTreeSet, error::Error, marker::PhantomData, path::Path};

use async_trait::async_trait;
use jp_attachment::{
    Attachment, BoxedHandler, HANDLERS, Handler, distributed_slice, linkme, typetag,
};
use jp_mcp::Client;
use serde::{Deserialize, Serialize};
use tracing::{debug, error};
use url::Url;

#[distributed_slice(HANDLERS)]
#[linkme(crate = linkme)]
static HTTP_HANDLER: fn() -> BoxedHandler = http_handler;

#[distributed_slice(HANDLERS)]
#[linkme(crate = linkme)]
static HTTPS_HANDLER: fn() -> BoxedHandler = https_handler;

fn http_handler() -> BoxedHandler {
    (Box::new(HttpContent::<Http>::default()) as Box<dyn Handler>).into()
}

fn https_handler() -> BoxedHandler {
    (Box::new(HttpContent::<Https>::default()) as Box<dyn Handler>).into()
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct HttpContent<T> {
    urls: BTreeSet<Url>,
    _phantom: PhantomData<T>,
}

/// Marker type for HTTP URLs.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
struct Http;

/// Marker type for HTTPS URLs.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
struct Https;

#[typetag::serde(name = "http_content")]
#[async_trait]
impl Handler for HttpContent<Http> {
    fn scheme(&self) -> &'static str {
        "http"
    }

    async fn add(&mut self, uri: &Url) -> Result<(), Box<dyn Error + Send + Sync>> {
        self.urls.insert(uri.clone());
        Ok(())
    }

    async fn remove(&mut self, uri: &Url) -> Result<(), Box<dyn Error + Send + Sync>> {
        self.urls.remove(uri);
        Ok(())
    }

    async fn list(&self) -> Result<Vec<Url>, Box<dyn Error + Send + Sync>> {
        Ok(self.urls.iter().cloned().collect())
    }

    async fn get(
        &self,
        _: &Path,
        _: Client,
    ) -> Result<Vec<Attachment>, Box<dyn Error + Send + Sync>> {
        debug!(id = "http", "Getting http attachment contents.");
        fetch_all(&self.urls).await
    }
}

#[typetag::serde(name = "https_content")]
#[async_trait]
impl Handler for HttpContent<Https> {
    fn scheme(&self) -> &'static str {
        "https"
    }

    async fn add(&mut self, uri: &Url) -> Result<(), Box<dyn Error + Send + Sync>> {
        self.urls.insert(uri.clone());

        Ok(())
    }

    async fn remove(&mut self, uri: &Url) -> Result<(), Box<dyn Error + Send + Sync>> {
        self.urls.remove(uri);

        Ok(())
    }

    async fn list(&self) -> Result<Vec<Url>, Box<dyn Error + Send + Sync>> {
        Ok(self.urls.iter().cloned().collect())
    }

    async fn get(
        &self,
        _: &Path,
        _: Client,
    ) -> Result<Vec<Attachment>, Box<dyn Error + Send + Sync>> {
        debug!(id = "https", "Getting https attachment contents.");
        fetch_all(&self.urls).await
    }
}

async fn fetch_all(urls: &BTreeSet<Url>) -> Result<Vec<Attachment>, Box<dyn Error + Send + Sync>> {
    let mut attachments = vec![];
    for url in urls {
        let content = match reqwest::get(url.clone()).await {
            Ok(response) => response.text().await?,
            Err(error) => {
                error!(uri = %url, %error, "Failed to fetch HTTP content.");
                continue;
            }
        };

        attachments.push(Attachment {
            source: url.to_string(),
            content,
            ..Default::default()
        });
    }

    Ok(attachments)
}
