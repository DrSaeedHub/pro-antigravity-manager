//! One way to build the HTTP client, and one way to describe its failures.

use std::sync::Arc;
use std::time::Duration;

use crate::proxy::ProxyResolver;

/// How a client behaves.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpOptions {
    /// The `User-Agent` header.
    pub user_agent: String,
    /// The limit for a whole request, body included (`None`: no limit).
    pub timeout: Option<Duration>,
    /// The limit for connecting (`None`: the system's).
    pub connect_timeout: Option<Duration>,
    /// The longest wait for the next piece of a response (`None`: no limit). Downloads use this
    /// instead of `timeout`, so a slow but steady connection still finishes.
    pub read_timeout: Option<Duration>,
    /// Refuse every plain-HTTP URL, redirects included.
    pub https_only: bool,
}

impl HttpOptions {
    /// Options with only a user agent and a limit for the whole request.
    #[must_use]
    pub fn new(user_agent: impl Into<String>, timeout: Duration) -> Self {
        Self {
            user_agent: user_agent.into(),
            timeout: Some(timeout),
            connect_timeout: None,
            read_timeout: None,
            https_only: false,
        }
    }
}

/// A client that sends through `proxy`, accepts gzip and deflate, and uses Windows' TLS.
pub fn client(
    options: &HttpOptions,
    proxy: Arc<dyn ProxyResolver>,
) -> Result<reqwest::Client, reqwest::Error> {
    let mut builder = reqwest::Client::builder()
        .user_agent(options.user_agent.clone())
        .gzip(true)
        .deflate(true)
        .no_proxy()
        .proxy(reqwest::Proxy::custom(move |url| proxy.proxy_for(url.as_str())))
        .tls_backend_native()
        .https_only(options.https_only);
    if let Some(timeout) = options.timeout {
        builder = builder.timeout(timeout);
    }
    if let Some(timeout) = options.connect_timeout {
        builder = builder.connect_timeout(timeout);
    }
    if let Some(timeout) = options.read_timeout {
        builder = builder.read_timeout(timeout);
    }
    builder.build()
}

/// The error with every cause behind it, for logs and the UI.
#[must_use]
pub fn describe(error: &reqwest::Error) -> String {
    let mut text = error.to_string();
    let mut source = std::error::Error::source(error);
    while let Some(cause) = source {
        text = format!("{text}: {cause}");
        source = cause.source();
    }
    text
}

#[cfg(test)]
mod tests {
    use wiremock::matchers::{header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use super::*;
    use crate::proxy::NoProxy;

    #[tokio::test]
    async fn the_client_sends_its_user_agent() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/x"))
            .and(header("user-agent", "tester/1"))
            .respond_with(ResponseTemplate::new(200).set_body_string("ok"))
            .mount(&server)
            .await;
        let options = HttpOptions {
            connect_timeout: Some(Duration::from_secs(5)),
            read_timeout: Some(Duration::from_secs(5)),
            ..HttpOptions::new("tester/1", Duration::from_secs(5))
        };
        let http = client(&options, Arc::new(NoProxy)).unwrap();
        let body = http.get(format!("{}/x", server.uri())).send().await.unwrap().text().await;
        assert_eq!(body.unwrap(), "ok");
    }

    #[tokio::test]
    async fn requests_go_through_the_chosen_proxy() {
        #[derive(Debug)]
        struct Through(String);
        impl ProxyResolver for Through {
            fn proxy_for(&self, _url: &str) -> Option<String> {
                Some(self.0.clone())
            }
        }
        let proxy = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_string("via the proxy"))
            .mount(&proxy)
            .await;
        let options = HttpOptions { timeout: None, ..HttpOptions::new("t", Duration::ZERO) };
        let http = client(&options, Arc::new(Through(proxy.uri()))).unwrap();
        let body = http.get("http://example.invalid/anything").send().await.unwrap().text().await;
        assert_eq!(body.unwrap(), "via the proxy");
    }

    #[tokio::test]
    async fn https_only_refuses_plain_http() {
        let options =
            HttpOptions { https_only: true, ..HttpOptions::new("t", Duration::from_secs(5)) };
        let http = client(&options, Arc::new(NoProxy)).unwrap();
        assert!(http.get("http://127.0.0.1:9/x").send().await.is_err());
    }

    #[tokio::test]
    async fn failures_name_their_causes() {
        let http = client(&HttpOptions::new("t", Duration::from_secs(5)), Arc::new(NoProxy));
        let error = http.unwrap().get("http://127.0.0.1:9/nothing").send().await.unwrap_err();
        let text = describe(&error);
        assert!(text.starts_with(&error.to_string()), "{text}");
        assert!(text.len() > error.to_string().len(), "the cause is added: {text}");
    }
}
