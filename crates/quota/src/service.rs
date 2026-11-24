//! Reading one account's quota: tokens, the sign-in client, and the three API calls.

use std::collections::HashMap;
use std::fmt::Write as _;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use antigravity::Antigravity;
use jiff::{SignedDuration, Timestamp};
use kernel::{Clock, Secret, json};
use reqwest::header::{ACCEPT, CONTENT_TYPE};
use serde_json::Value;
use vault::{CredentialStore, SignIn, SignInError};

use crate::client::{ClientCache, ClientCredentials, Discovered, scan};
use crate::error::QuotaError;
use crate::model::AccountQuota;
use crate::parse;
use crate::proxy::ProxyResolver;

/// Endpoints and limits.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuotaSettings {
    /// Google's OAuth token endpoint.
    pub token_url: String,
    /// The Cloud Code API, ending with `v1internal:`.
    pub api_base: String,
    /// The `User-Agent` header.
    pub user_agent: String,
    /// The limit for each HTTP request.
    pub timeout: Duration,
    /// A token expiring sooner than this is refreshed first.
    pub token_margin: SignedDuration,
    /// Mixed into the encryption of the sign-in client cache.
    pub cache_entropy: Vec<u8>,
    /// The `loadCodeAssist` request body.
    pub client_metadata: String,
}

impl Default for QuotaSettings {
    fn default() -> Self {
        Self {
            token_url: "https://oauth2.googleapis.com/token".into(),
            api_base: "https://cloudcode-pa.googleapis.com/v1internal:".into(),
            user_agent: format!("antigravity-account-switcher/{}", env!("CARGO_PKG_VERSION")),
            timeout: Duration::from_secs(25),
            token_margin: SignedDuration::from_mins(2),
            cache_entropy: b"antigravity-switcher-oauth-client".to_vec(),
            client_metadata: r#"{"metadata":{"ideType":"ANTIGRAVITY","platform":"PLATFORM_UNSPECIFIED","pluginType":"GEMINI"}}"#.into(),
        }
    }
}

impl QuotaSettings {
    /// The HTTP client for these settings, sending through `proxy`.
    pub fn http_client(
        &self,
        proxy: Arc<dyn ProxyResolver>,
    ) -> Result<reqwest::Client, QuotaError> {
        reqwest::Client::builder()
            .user_agent(self.user_agent.clone())
            .timeout(self.timeout)
            .gzip(true)
            .deflate(true)
            .no_proxy()
            .proxy(reqwest::Proxy::custom(move |url| proxy.proxy_for(url.as_str())))
            .tls_backend_native()
            .build()
            .map_err(|error| QuotaError::Network(error.to_string()))
    }
}

#[derive(Debug, Clone)]
struct CachedToken {
    token: Secret,
    expires: Timestamp,
}

#[derive(Debug, Default)]
struct ClientState {
    loaded: bool,
    current: Option<ClientCredentials>,
    scanned: Option<Discovered>,
}

/// A failed token request.
#[derive(Debug)]
enum TokenFailure {
    OAuth { code: String, description: String },
    ClientNotFound,
    Network(String),
}

impl TokenFailure {
    fn is_client_problem(&self) -> bool {
        matches!(self, Self::OAuth { code, .. } if code == "invalid_client" || code == "unauthorized_client")
    }

    fn into_error(self) -> QuotaError {
        match self {
            Self::OAuth { code, .. } if code == "invalid_grant" => QuotaError::SignInExpired,
            Self::OAuth { code, description } => QuotaError::OAuth { code, description },
            Self::ClientNotFound => QuotaError::ClientNotFound,
            Self::Network(reason) => QuotaError::Network(reason),
        }
    }
}

/// The access token kept inside the saved sign-in.
struct StoredAccess {
    token: Option<Secret>,
    expiry: Option<Timestamp>,
}

fn network(error: &reqwest::Error) -> String {
    let mut text = error.to_string();
    let mut source = std::error::Error::source(error);
    while let Some(cause) = source {
        text = format!("{text}: {cause}");
        source = cause.source();
    }
    text
}

/// `application/x-www-form-urlencoded`, encoding everything but the unreserved characters.
fn form(pairs: &[(&str, &str)]) -> String {
    let encode = |text: &str| {
        text.bytes().fold(String::new(), |mut out, byte| {
            if byte.is_ascii_alphanumeric() || b"-._~".contains(&byte) {
                out.push(char::from(byte));
            } else {
                let _ = write!(out, "%{byte:02X}");
            }
            out
        })
    };
    pairs
        .iter()
        .map(|(key, value)| format!("{}={}", encode(key), encode(value)))
        .collect::<Vec<_>>()
        .join("&")
}

fn later(now: Timestamp, by: SignedDuration) -> Timestamp {
    now.checked_add(by).unwrap_or(Timestamp::MAX)
}

/// Reads quota for saved accounts. Share one instance: it remembers access tokens and the
/// sign-in client for the whole run.
#[derive(Debug)]
pub struct QuotaService {
    http: reqwest::Client,
    store: Arc<dyn CredentialStore>,
    antigravity: Antigravity,
    cache: ClientCache,
    clock: Arc<dyn Clock>,
    settings: QuotaSettings,
    tokens: Mutex<HashMap<String, CachedToken>>,
    client: tokio::sync::Mutex<ClientState>,
}

impl QuotaService {
    /// The service over its parts.
    #[must_use]
    pub fn new(
        http: reqwest::Client,
        store: Arc<dyn CredentialStore>,
        antigravity: Antigravity,
        cache: ClientCache,
        clock: Arc<dyn Clock>,
        settings: QuotaSettings,
    ) -> Self {
        Self {
            http,
            store,
            antigravity,
            cache,
            clock,
            settings,
            tokens: Mutex::new(HashMap::new()),
            client: tokio::sync::Mutex::new(ClientState::default()),
        }
    }

    /// The quota of the account `account`, whose sign-in is the credential `target`. Never
    /// fails: problems are reported in [`AccountQuota::error`].
    pub async fn fetch(&self, account: &str, target: &str) -> AccountQuota {
        let mut quota = AccountQuota::new(account, self.clock.now());
        if let Err(error) = self.fill(&mut quota, target).await {
            tracing::warn!(account, code = error.code(), %error, "quota could not be read");
            quota.error = Some(error);
        }
        quota
    }

    async fn fill(&self, quota: &mut AccountQuota, target: &str) -> Result<(), QuotaError> {
        let sign_in = match SignIn::read(self.store.as_ref(), target) {
            Ok(Some(sign_in)) => sign_in,
            Ok(None) => return Err(QuotaError::NotSaved),
            Err(SignInError::Unreadable | SignInError::Vault(_)) => {
                return Err(QuotaError::Unreadable);
            }
        };
        quota.email.clone_from(&sign_in.email);
        let refresh = sign_in.refresh_token.ok_or(QuotaError::NoRefreshToken)?;
        let mut stored =
            StoredAccess { token: sign_in.access_token, expiry: sign_in.access_expiry };

        let metadata = self.settings.client_metadata.clone();
        let assist = parse::code_assist(
            &self.call("loadCodeAssist", &metadata, &refresh, &mut stored).await?,
        );
        quota.plan = assist.plan;
        quota.tier = assist.tier;
        let body = parse::project_body(assist.project.as_deref());

        match self.call("retrieveUserQuotaSummary", &body, &refresh, &mut stored).await {
            Ok(summary) => quota.groups = parse::summary(&summary),
            // Some accounts and regions have no summary; the models still work.
            Err(QuotaError::Api { status, .. }) => tracing::info!(status, "no quota summary"),
            Err(error) => return Err(error),
        }
        quota.models =
            parse::models(&self.call("fetchAvailableModels", &body, &refresh, &mut stored).await?);
        Ok(())
    }

    async fn call(
        &self,
        method: &str,
        body: &str,
        refresh: &Secret,
        stored: &mut StoredAccess,
    ) -> Result<Value, QuotaError> {
        for attempt in 0..2 {
            let token = self.access_token(refresh, stored, attempt > 0).await?;
            let response = self
                .http
                .post(format!("{}{method}", self.settings.api_base))
                .bearer_auth(token.expose())
                .header(CONTENT_TYPE, "application/json")
                .header(ACCEPT, "application/json")
                .body(body.to_owned())
                .send()
                .await
                .map_err(|error| QuotaError::Network(network(&error)))?;
            let status = response.status().as_u16();
            let text =
                response.text().await.map_err(|error| QuotaError::Network(network(&error)))?;
            if status == 200 {
                return serde_json::from_str(&text).map_err(|_| QuotaError::Api {
                    method: method.to_owned(),
                    status,
                    message: Some("the answer was not JSON".into()),
                });
            }
            if status == 401 && attempt == 0 {
                stored.token = None;
                continue;
            }
            let message = serde_json::from_str::<Value>(&text).ok().and_then(|value| {
                json::get(&value, "error").and_then(|error| json::text(error, "message"))
            });
            return Err(QuotaError::Api { method: method.to_owned(), status, message });
        }
        Err(QuotaError::Api { method: method.to_owned(), status: 401, message: None })
    }

    async fn access_token(
        &self,
        refresh: &Secret,
        stored: &StoredAccess,
        force: bool,
    ) -> Result<Secret, QuotaError> {
        let now = self.clock.now();
        let fresh_until = later(now, self.settings.token_margin);
        {
            let mut tokens = self.tokens.lock().unwrap_or_else(PoisonError::into_inner);
            if force {
                tokens.remove(refresh.expose());
            } else if let Some(cached) = tokens.get(refresh.expose())
                && cached.expires > fresh_until
            {
                return Ok(cached.token.clone());
            }
        }
        if !force
            && let (Some(token), Some(expiry)) = (&stored.token, stored.expiry)
            && expiry > fresh_until
        {
            return Ok(token.clone());
        }
        let (token, lifetime) = self.refresh(refresh).await.map_err(TokenFailure::into_error)?;
        self.tokens.lock().unwrap_or_else(PoisonError::into_inner).insert(
            refresh.expose().to_owned(),
            CachedToken { token: token.clone(), expires: later(now, lifetime) },
        );
        Ok(token)
    }

    /// Exchanges the refresh token, finding the sign-in client first when needed.
    async fn refresh(&self, refresh: &Secret) -> Result<(Secret, SignedDuration), TokenFailure> {
        let known = {
            let mut state = self.client.lock().await;
            if !state.loaded {
                state.loaded = true;
                state.current = self.cache.load();
            }
            state.current.clone()
        };
        if let Some(client) = &known {
            match self.request_token(client, refresh).await {
                Err(failure) if failure.is_client_problem() => {}
                other => return other,
            }
        }

        let mut state = self.client.lock().await;
        if let Some(client) = state.current.clone().filter(|client| Some(client) != known.as_ref())
        {
            match self.request_token(&client, refresh).await {
                Err(failure) if failure.is_client_problem() => {}
                other => return other,
            }
        }
        // Scan once per run, and again whenever the last scan found nothing usable.
        if !state.scanned.as_ref().is_some_and(Discovered::is_usable) {
            let files = self.antigravity.language_server_files();
            state.scanned =
                Some(tokio::task::spawn_blocking(move || scan(&files)).await.unwrap_or_default());
        }
        let found = state.scanned.clone().unwrap_or_default();
        if !found.is_usable() {
            return Err(TokenFailure::ClientNotFound);
        }

        let mut last = None;
        let mut expired = false;
        for id in &found.ids {
            for secret in &found.secrets {
                let candidate =
                    ClientCredentials { id: id.clone(), secret: Secret::new(secret.as_str()) };
                match self.request_token(&candidate, refresh).await {
                    Ok(token) => {
                        self.cache.save(&candidate);
                        state.current = Some(candidate);
                        return Ok(token);
                    }
                    Err(TokenFailure::OAuth { code, description }) => {
                        expired |= code == "invalid_grant";
                        last = Some(TokenFailure::OAuth { code, description });
                    }
                    Err(network) => return Err(network),
                }
            }
        }
        if expired {
            return Err(TokenFailure::OAuth {
                code: "invalid_grant".into(),
                description: String::new(),
            });
        }
        Err(last.unwrap_or(TokenFailure::OAuth {
            code: String::new(),
            description: "the token refresh failed".into(),
        }))
    }

    async fn request_token(
        &self,
        client: &ClientCredentials,
        refresh: &Secret,
    ) -> Result<(Secret, SignedDuration), TokenFailure> {
        let body = form(&[
            ("client_id", &client.id),
            ("client_secret", client.secret.expose()),
            ("refresh_token", refresh.expose()),
            ("grant_type", "refresh_token"),
        ]);
        let response = self
            .http
            .post(&self.settings.token_url)
            .header(CONTENT_TYPE, "application/x-www-form-urlencoded")
            .header(ACCEPT, "application/json")
            .body(body)
            .send()
            .await
            .map_err(|error| TokenFailure::Network(network(&error)))?;
        let status = response.status().as_u16();
        let text = response.text().await.map_err(|error| TokenFailure::Network(network(&error)))?;
        let answer = serde_json::from_str::<Value>(&text).unwrap_or(Value::Null);
        if status != 200 {
            let code = json::text(&answer, "error").unwrap_or_else(|| format!("http_{status}"));
            let description =
                json::text(&answer, "error_description").unwrap_or_else(|| code.clone());
            return Err(TokenFailure::OAuth { code, description });
        }
        let token =
            json::text(&answer, "access_token").filter(|t| !t.is_empty()).ok_or_else(|| {
                TokenFailure::OAuth {
                    code: "no_token".into(),
                    description: "the token answer had no access token".into(),
                }
            })?;
        #[allow(clippy::cast_possible_truncation, reason = "clamped to a sane range first")]
        let seconds =
            json::number(&answer, "expires_in").map_or(3000, |n| n.clamp(0.0, 86_400.0) as i64);
        Ok((Secret::new(token), SignedDuration::from_secs(seconds)))
    }
}

#[cfg(test)]
mod tests;
