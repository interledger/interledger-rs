use futures::Future;
use std::sync::Arc;
use reqwest::{
  async::{Client, ClientBuilder},
  header::{HeaderMap, HeaderValue}
};

pub struct HttpClientService {
  client: Arc<Client>,
}

impl HttpClientService {
  pub fn new() -> Self {
    let mut headers = HeaderMap::with_capacity(1);
    headers.insert(
      CONTENT_TYPE,
      HeaderValue::from_static("application/octet-string"),
    );
    let client = ClientBuilder::new()
      .default_headers(headers)
      .timeout(Duration::from_secs(30))
      .build()
      .unwrap();

    HttpClientService {
      client,
    }
  }
}
