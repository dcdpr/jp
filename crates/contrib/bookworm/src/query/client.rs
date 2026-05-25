use std::{path::PathBuf, sync::LazyLock};

use reqwest::header::{self, USER_AGENT};

use crate::dl;

pub(crate) static GLOBAL_CLIENT: LazyLock<Client> = LazyLock::new(Client::default);

pub(crate) struct Client {
    pub crates_path: PathBuf,
    pub http_client: reqwest::Client,
}

impl Default for Client {
    fn default() -> Self {
        let mut headers = header::HeaderMap::new();
        headers.insert(
            USER_AGENT,
            header::HeaderValue::from_static("bookworm (https://github.com/dcdpr/bookworm)"),
        );

        let http_client = reqwest::Client::builder()
            .default_headers(headers)
            .build()
            .expect("Client::default()");

        Self {
            crates_path: dl::default_crates_root(),
            http_client,
        }
    }
}
