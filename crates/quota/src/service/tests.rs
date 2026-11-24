//! The quota service against a pretend Google (wiremock) and a pretend Antigravity install.

use std::fs;

use antigravity::{FakeHost, HostSettings};
use jiff::tz::TimeZone;
use kernel::{FixedFolders, KnownFolders, ManualClock};
use serde_json::json;
use vault::MemoryStore;
use wiremock::matchers::{body_string, body_string_contains, header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use super::*;
use crate::client::tests::{ID_A, ID_B, SECRET_A, SECRET_B};
use crate::proxy::NoProxy;
use crate::secret_box::FakeBox;

const SLOT: &str = "slot:work";
const NOW: &str = "2026-11-23T10:00:00Z";

struct Rig {
    server: MockServer,
    dir: tempfile::TempDir,
    store: Arc<MemoryStore>,
    clock: Arc<ManualClock>,
    service: QuotaService,
}

impl Rig {
    async fn new() -> Self {
        let server = MockServer::start().await;
        let dir = tempfile::tempdir().unwrap();
        let exe = dir.path().join("LocalAppData\\Programs\\antigravity\\Antigravity.exe");
        fs::create_dir_all(exe.parent().unwrap()).unwrap();
        fs::write(&exe, b"x").unwrap();
        let folders: Arc<dyn KnownFolders> = Arc::new(FixedFolders::new(dir.path()));
        let antigravity =
            Antigravity::new(Arc::new(FakeHost::new()), folders, HostSettings::default());
        let cache = ClientCache::new(dir.path().join("oauth-client.bin"), Arc::new(FakeBox), b"e");
        let store = Arc::new(MemoryStore::new());
        let clock = Arc::new(ManualClock::new(NOW.parse().unwrap(), TimeZone::UTC));
        let settings = QuotaSettings {
            token_url: format!("{}/token", server.uri()),
            api_base: format!("{}/v1internal:", server.uri()),
            ..QuotaSettings::default()
        };
        let http = settings.http_client(Arc::new(NoProxy)).unwrap();
        let service =
            QuotaService::new(http, store.clone(), antigravity, cache, clock.clone(), settings);
        Self { server, dir, store, clock, service }
    }

    fn cache(&self) -> ClientCache {
        ClientCache::new(self.dir.path().join("oauth-client.bin"), Arc::new(FakeBox), b"e")
    }

    fn cache_client(&self, id: &str, secret: &str) {
        self.cache().save(&ClientCredentials { id: id.into(), secret: Secret::new(secret) });
    }

    fn language_server(&self, content: &str) {
        let file = self.dir.path().join(
            "LocalAppData\\Programs\\antigravity\\resources\\bin\\language_server_windows_x64.exe",
        );
        fs::create_dir_all(file.parent().unwrap()).unwrap();
        fs::write(file, content).unwrap();
    }

    fn save_sign_in(&self, access: Option<(&str, &str)>) {
        let mut token = json!({ "refresh_token": "1//refresh-work" });
        if let Some((value, expiry)) = access {
            token["access_token"] = json!(value);
            token["expiry"] = json!(expiry);
        }
        let blob = json!({ "id_token_email": "work@example.com", "token": token });
        self.store.insert(SLOT, blob.to_string().as_bytes());
    }

    async fn token_answers(&self, id: &str, secret: &str, status: u16, body: Value, times: u64) {
        Mock::given(method("POST"))
            .and(path("/token"))
            .and(body_string_contains(format!("client_id={id}")))
            .and(body_string_contains(format!("client_secret={secret}")))
            .and(body_string_contains("refresh_token=1%2F%2Frefresh-work"))
            .and(body_string_contains("grant_type=refresh_token"))
            .and(header("content-type", "application/x-www-form-urlencoded"))
            .respond_with(ResponseTemplate::new(status).set_body_json(body))
            .expect(times)
            .mount(&self.server)
            .await;
    }

    async fn other_clients_are_refused(&self) {
        Mock::given(method("POST"))
            .and(path("/token"))
            .respond_with(ResponseTemplate::new(401).set_body_json(
                json!({ "error": "invalid_client", "error_description": "Unauthorized" }),
            ))
            .with_priority(10)
            .mount(&self.server)
            .await;
    }

    async fn api(&self, name: &str, bearer: &str, status: u16, body: Value) {
        Mock::given(method("POST"))
            .and(path(format!("/v1internal:{name}")))
            .and(header("authorization", format!("Bearer {bearer}")))
            .and(header("content-type", "application/json"))
            .respond_with(ResponseTemplate::new(status).set_body_json(body))
            .mount(&self.server)
            .await;
    }

    async fn healthy_api(&self, bearer: &str) {
        self.api(
            "loadCodeAssist",
            bearer,
            200,
            json!({
                "paidTier": { "name": "Google AI Pro" },
                "currentTier": { "id": "standard-tier", "name": "Standard" },
                "cloudaicompanionProject": "proj-1",
            }),
        )
        .await;
        Mock::given(method("POST"))
            .and(path("/v1internal:retrieveUserQuotaSummary"))
            .and(body_string(r#"{"project":"proj-1"}"#))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "groups": [{ "displayName": "Claude and GPT", "buckets": [
                    { "bucketId": "w", "window": "weekly", "remainingFraction": 0.41 },
                    { "bucketId": "f", "window": "5h", "remainingFraction": 0.82,
                      "resetTime": "2026-11-23T12:15:00Z" },
                ]}],
            })))
            .mount(&self.server)
            .await;
        self.api(
            "fetchAvailableModels",
            bearer,
            200,
            json!({ "models": {
                "gemini-3-pro": { "displayName": "Gemini 3 Pro", "recommended": true,
                                  "quotaInfo": { "remainingFraction": 0.55 } },
            }}),
        )
        .await;
    }

    async fn fetch(&self) -> AccountQuota {
        self.service.fetch("work", SLOT).await
    }
}

fn ok_token(value: &str) -> Value {
    json!({ "access_token": value, "expires_in": 3600, "token_type": "Bearer" })
}

#[tokio::test]
async fn reads_everything_with_a_cached_client() {
    let rig = Rig::new().await;
    rig.save_sign_in(None);
    rig.cache_client(ID_A, SECRET_A);
    rig.token_answers(ID_A, SECRET_A, 200, ok_token("at-1"), 1).await;
    rig.healthy_api("at-1").await;

    let quota = rig.fetch().await;
    assert_eq!(quota.error, None);
    assert_eq!(quota.account, "work");
    assert_eq!(quota.email.as_deref(), Some("work@example.com"));
    assert_eq!(quota.plan.as_deref(), Some("Google AI Pro"));
    assert_eq!(quota.tier.as_deref(), Some("standard-tier"));
    assert_eq!(quota.fetched_at, NOW.parse().unwrap());
    assert_eq!(quota.bucket("5h").unwrap().fraction, 0.82);
    assert_eq!(quota.bucket("weekly").unwrap().fraction, 0.41);
    assert_eq!(quota.lowest_gemini().unwrap().fraction, 0.55);
}

#[tokio::test]
async fn a_fresh_stored_access_token_skips_the_refresh() {
    let rig = Rig::new().await;
    rig.save_sign_in(Some(("stored-token", "2026-11-23T11:00:00Z")));
    rig.token_answers(ID_A, SECRET_A, 200, ok_token("unused"), 0).await;
    rig.healthy_api("stored-token").await;
    assert_eq!(rig.fetch().await.error, None);
}

#[tokio::test]
async fn a_nearly_expired_stored_token_is_refreshed() {
    let rig = Rig::new().await;
    rig.save_sign_in(Some(("stored-token", "2026-11-23T10:01:00Z")));
    rig.cache_client(ID_A, SECRET_A);
    rig.token_answers(ID_A, SECRET_A, 200, ok_token("at-1"), 1).await;
    rig.healthy_api("at-1").await;
    assert_eq!(rig.fetch().await.error, None);
}

#[tokio::test]
async fn a_rejected_token_is_refreshed_once() {
    let rig = Rig::new().await;
    rig.save_sign_in(Some(("revoked", "2026-11-23T11:00:00Z")));
    rig.cache_client(ID_A, SECRET_A);
    rig.token_answers(ID_A, SECRET_A, 200, ok_token("at-1"), 1).await;
    rig.api("loadCodeAssist", "revoked", 401, json!({ "error": { "message": "expired" } })).await;
    rig.healthy_api("at-1").await;
    assert_eq!(rig.fetch().await.error, None);
}

#[tokio::test]
async fn a_second_401_is_reported() {
    let rig = Rig::new().await;
    rig.save_sign_in(None);
    rig.cache_client(ID_A, SECRET_A);
    rig.token_answers(ID_A, SECRET_A, 200, ok_token("at-1"), 2).await;
    rig.api("loadCodeAssist", "at-1", 401, json!({})).await;
    let error = rig.fetch().await.error.unwrap();
    assert_eq!(
        error,
        QuotaError::Api { method: "loadCodeAssist".into(), status: 401, message: None }
    );
}

#[tokio::test]
async fn access_tokens_are_reused_until_they_near_expiry() {
    let rig = Rig::new().await;
    rig.save_sign_in(None);
    rig.cache_client(ID_A, SECRET_A);
    rig.token_answers(ID_A, SECRET_A, 200, ok_token("at-1"), 2).await;
    rig.healthy_api("at-1").await;
    assert_eq!(rig.fetch().await.error, None);
    assert_eq!(rig.fetch().await.error, None, "same token, no new request");
    rig.clock.advance(SignedDuration::from_mins(59));
    assert_eq!(rig.fetch().await.error, None, "one more request once it nears expiry");
}

#[tokio::test]
async fn without_a_cache_the_client_is_found_by_scanning_and_cached() {
    let rig = Rig::new().await;
    rig.save_sign_in(None);
    rig.language_server(&format!("\0{ID_B}\0{SECRET_B}\0{ID_A}\0{SECRET_A}\0"));
    rig.token_answers(ID_A, SECRET_A, 200, ok_token("at-1"), 1).await;
    rig.other_clients_are_refused().await;
    rig.healthy_api("at-1").await;

    assert_eq!(rig.fetch().await.error, None);
    let cached = rig.cache().load().unwrap();
    assert_eq!(cached.id, ID_A);
    assert_eq!(cached.secret.expose(), SECRET_A);
}

#[tokio::test]
async fn a_stale_cached_client_is_replaced() {
    let rig = Rig::new().await;
    rig.save_sign_in(None);
    rig.cache_client(ID_B, SECRET_B);
    rig.language_server(&format!("{ID_A} {SECRET_A}"));
    rig.token_answers(ID_A, SECRET_A, 200, ok_token("at-1"), 1).await;
    rig.other_clients_are_refused().await;
    rig.healthy_api("at-1").await;
    assert_eq!(rig.fetch().await.error, None);
    assert_eq!(rig.cache().load().unwrap().id, ID_A);
}

#[tokio::test]
async fn a_missing_client_is_looked_for_again_later() {
    let rig = Rig::new().await;
    rig.save_sign_in(None);
    assert_eq!(rig.fetch().await.error, Some(QuotaError::ClientNotFound));

    rig.language_server(&format!("{ID_A} {SECRET_A}"));
    rig.token_answers(ID_A, SECRET_A, 200, ok_token("at-1"), 1).await;
    rig.healthy_api("at-1").await;
    assert_eq!(rig.fetch().await.error, None, "found after Antigravity was installed");
}

#[tokio::test]
async fn an_expired_sign_in_is_named_as_such() {
    let rig = Rig::new().await;
    rig.save_sign_in(None);
    rig.cache_client(ID_A, SECRET_A);
    let expired = json!({ "error": "invalid_grant", "error_description": "Token has been expired or revoked." });
    rig.token_answers(ID_A, SECRET_A, 400, expired, 1).await;
    assert_eq!(rig.fetch().await.error, Some(QuotaError::SignInExpired));
}

#[tokio::test]
async fn an_expired_sign_in_wins_over_refused_clients_while_scanning() {
    let rig = Rig::new().await;
    rig.save_sign_in(None);
    rig.language_server(&format!("{ID_A} {ID_B} {SECRET_A}"));
    let expired = json!({ "error": "invalid_grant", "error_description": "revoked" });
    rig.token_answers(ID_A, SECRET_A, 400, expired, 1).await;
    rig.other_clients_are_refused().await;
    assert_eq!(rig.fetch().await.error, Some(QuotaError::SignInExpired));
}

#[tokio::test]
async fn other_oauth_refusals_keep_googles_words() {
    let rig = Rig::new().await;
    rig.save_sign_in(None);
    rig.cache_client(ID_A, SECRET_A);
    rig.token_answers(
        ID_A,
        SECRET_A,
        400,
        json!({ "error": "invalid_request", "error_description": "Bad" }),
        1,
    )
    .await;
    assert_eq!(
        rig.fetch().await.error,
        Some(QuotaError::OAuth { code: "invalid_request".into(), description: "Bad".into() })
    );
}

#[tokio::test]
async fn a_scan_where_every_client_is_refused_reports_the_last_refusal() {
    let rig = Rig::new().await;
    rig.save_sign_in(None);
    rig.language_server(&format!("{ID_A} {SECRET_A}"));
    rig.other_clients_are_refused().await;
    assert_eq!(
        rig.fetch().await.error,
        Some(QuotaError::OAuth {
            code: "invalid_client".into(),
            description: "Unauthorized".into()
        })
    );
}

#[tokio::test]
async fn token_answers_that_are_not_json_or_lack_a_token_are_errors() {
    let rig = Rig::new().await;
    rig.save_sign_in(None);
    rig.cache_client(ID_A, SECRET_A);
    Mock::given(method("POST"))
        .and(path("/token"))
        .respond_with(ResponseTemplate::new(502).set_body_string("<html>bad gateway</html>"))
        .up_to_n_times(1)
        .mount(&rig.server)
        .await;
    assert_eq!(
        rig.fetch().await.error,
        Some(QuotaError::OAuth { code: "http_502".into(), description: "http_502".into() })
    );

    rig.server.reset().await;
    rig.token_answers(ID_A, SECRET_A, 200, json!({ "expires_in": 10 }), 1).await;
    let error = rig.fetch().await.error.unwrap();
    assert_eq!(error.code(), "quota.oauth");
    assert!(error.to_string().contains("no_token"), "{error}");
}

#[tokio::test]
async fn a_missing_summary_is_tolerated() {
    let rig = Rig::new().await;
    rig.save_sign_in(Some(("t", "2026-11-23T12:00:00Z")));
    rig.api("loadCodeAssist", "t", 200, json!({})).await;
    Mock::given(method("POST"))
        .and(path("/v1internal:retrieveUserQuotaSummary"))
        .and(body_string("{}"))
        .respond_with(
            ResponseTemplate::new(403).set_body_json(json!({ "error": { "message": "nope" } })),
        )
        .mount(&rig.server)
        .await;
    rig.api("fetchAvailableModels", "t", 200, json!({ "models": {} })).await;
    let quota = rig.fetch().await;
    assert_eq!(quota.error, None);
    assert!(quota.groups.is_empty());
    assert_eq!(quota.plan, None);
}

#[tokio::test]
async fn api_errors_carry_googles_message() {
    let rig = Rig::new().await;
    rig.save_sign_in(Some(("t", "2026-11-23T12:00:00Z")));
    rig.api("loadCodeAssist", "t", 500, json!({ "error": { "message": "backend down" } })).await;
    assert_eq!(
        rig.fetch().await.error,
        Some(QuotaError::Api {
            method: "loadCodeAssist".into(),
            status: 500,
            message: Some("backend down".into()),
        })
    );
}

#[tokio::test]
async fn an_answer_that_is_not_json_is_an_api_error() {
    let rig = Rig::new().await;
    rig.save_sign_in(Some(("t", "2026-11-23T12:00:00Z")));
    Mock::given(method("POST"))
        .and(path("/v1internal:loadCodeAssist"))
        .respond_with(ResponseTemplate::new(200).set_body_string("<html/>"))
        .mount(&rig.server)
        .await;
    let error = rig.fetch().await.error.unwrap();
    assert_eq!(error.to_string(), "loadCodeAssist: HTTP 200 - the answer was not JSON");
}

#[tokio::test]
async fn a_network_failure_is_reported() {
    let rig = Rig::new().await;
    rig.save_sign_in(Some(("t", "2026-11-23T12:00:00Z")));
    let dead = QuotaSettings {
        api_base: "http://127.0.0.1:9/v1internal:".into(),
        timeout: Duration::from_secs(5),
        ..QuotaSettings::default()
    };
    let service = QuotaService::new(
        dead.http_client(Arc::new(NoProxy)).unwrap(),
        rig.store.clone(),
        rig.service.antigravity.clone(),
        rig.cache(),
        rig.clock.clone(),
        dead,
    );
    let error = service.fetch("work", SLOT).await.error.unwrap();
    assert_eq!(error.code(), "quota.network");
}

#[tokio::test]
async fn problems_with_the_saved_sign_in_are_named() {
    let rig = Rig::new().await;
    assert_eq!(rig.fetch().await.error, Some(QuotaError::NotSaved));
    rig.store.insert(SLOT, b"not json");
    assert_eq!(rig.fetch().await.error, Some(QuotaError::Unreadable));
    rig.store.insert(SLOT, br#"{"id_token_email":"a@example.com","token":{}}"#);
    let quota = rig.fetch().await;
    assert_eq!(quota.error, Some(QuotaError::NoRefreshToken));
    assert_eq!(quota.email.as_deref(), Some("a@example.com"), "the email is known anyway");
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
    let rig = Rig::new().await;
    rig.save_sign_in(Some(("t", "2026-11-23T12:00:00Z")));
    rig.api("loadCodeAssist", "t", 500, json!({ "error": { "message": "seen by the proxy" } }))
        .await;
    let settings = QuotaSettings {
        api_base: "http://google.invalid/v1internal:".into(),
        ..QuotaSettings::default()
    };
    let http = settings.http_client(Arc::new(Through(rig.server.uri()))).unwrap();
    let service = QuotaService::new(
        http,
        rig.store.clone(),
        rig.service.antigravity.clone(),
        rig.cache(),
        rig.clock.clone(),
        settings,
    );
    let error = service.fetch("work", SLOT).await.error.unwrap();
    assert_eq!(error.to_string(), "loadCodeAssist: HTTP 500 - seen by the proxy");
}

#[test]
fn defaults_match_the_real_service() {
    let settings = QuotaSettings::default();
    assert_eq!(settings.token_url, "https://oauth2.googleapis.com/token");
    assert_eq!(settings.api_base, "https://cloudcode-pa.googleapis.com/v1internal:");
    assert_eq!(
        settings.user_agent,
        format!("antigravity-account-switcher/{}", env!("CARGO_PKG_VERSION"))
    );
    assert_eq!(settings.timeout, Duration::from_secs(25));
    assert_eq!(settings.token_margin, SignedDuration::from_mins(2));
    assert_eq!(settings.cache_entropy, b"antigravity-switcher-oauth-client");
    assert!(settings.client_metadata.contains("\"ideType\":\"ANTIGRAVITY\""));
}

#[test]
fn forms_encode_like_uri_escape_data_string() {
    assert_eq!(form(&[("a", "b c"), ("x", "1//r+/=&")]), "a=b%20c&x=1%2F%2Fr%2B%2F%3D%26");
    assert_eq!(form(&[("k", "AZaz09-._~")]), "k=AZaz09-._~");
    assert_eq!(form(&[]), "");
}

#[test]
fn later_saturates() {
    assert_eq!(later(Timestamp::MAX, SignedDuration::from_secs(1)), Timestamp::MAX);
}
