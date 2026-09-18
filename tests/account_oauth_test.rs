use chrono::{Duration, Utc};
use megh::auth::{
    build_authorization_url, AuthUrlOptions, BasicTokenResponse, CsrfToken, OAuthFlowMode,
    OAuthProviderConfig, OAuthUserInfo, PkceCodeChallenge, TokenResponse,
};
use megh::account::{ConnectedAccount, OAuth2Tokens};
use megh::Table;

#[test]
fn test_connected_account_table_metadata() {
    assert_eq!(ConnectedAccount::TABLE_NAME, "connected_accounts");
    assert_eq!(ConnectedAccount::CONFLICT_COLUMNS, &["account_id", "provider"]);
    assert!(ConnectedAccount::COLUMNS.contains(&"account_id"));
    assert!(ConnectedAccount::COLUMNS.contains(&"provider"));
    assert!(ConnectedAccount::COLUMNS.contains(&"access_token"));
}

#[test]
fn test_connected_account_helpers() {
    let now = Utc::now();
    let acc = ConnectedAccount {
        account_id: "google-uid-123".to_string(),
        provider: "google".to_string(),
        email: Some("user@example.com".to_string()),
        access_token: "test-token".to_string(),
        refresh_token: Some("refresh-token".to_string()),
        token_type: Some("Bearer".to_string()),
        expiry: Some(now + Duration::hours(1)),
        created_at: Some(now),
        updated_at: Some(now),
        disconnected_at: None,
    };

    assert!(acc.is_connected());
    assert!(!acc.is_expired(0));

    let disconnected = ConnectedAccount {
        disconnected_at: Some(now),
        ..acc.clone()
    };
    assert!(!disconnected.is_connected());

    let expired = ConnectedAccount {
        expiry: Some(now - Duration::seconds(10)),
        ..acc
    };
    assert!(expired.is_expired(0));
}

#[test]
fn test_oauth2_tokens_from_library_response() {
    let json = r#"{
        "access_token": "ya29.sample-token",
        "token_type": "Bearer",
        "expires_in": 3600,
        "refresh_token": "1//sample-refresh-token",
        "scope": "openid email https://www.googleapis.com/auth/calendar"
    }"#;

    let token_resp: BasicTokenResponse = serde_json::from_str(json).unwrap();
    assert_eq!(token_resp.access_token().secret(), "ya29.sample-token");

    // Direct conversion into domain OAuth2Tokens
    let domain_tokens = OAuth2Tokens::from(&token_resp);
    assert_eq!(domain_tokens.access_token, "ya29.sample-token");
    assert_eq!(
        domain_tokens.refresh_token.as_deref(),
        Some("1//sample-refresh-token")
    );
    assert!(domain_tokens.token_expires_at.is_some());
    assert!(!domain_tokens.is_expired(0));
}

#[test]
fn test_pkce_challenge_from_library() {
    let (challenge, verifier) = PkceCodeChallenge::new_random_sha256();
    assert_eq!(challenge.method().as_str(), "S256");
    assert!(!challenge.as_str().is_empty());
    assert!(!verifier.secret().is_empty());
}

#[test]
fn test_build_auth_url_web_and_desktop_modes() {
    let provider_config = OAuthProviderConfig::google(
        "google-client-id-123.apps.googleusercontent.com",
        Some("secret".to_string()),
    );

    // 1. Web redirect flow with incremental scopes and offline access
    let web_flow = OAuthFlowMode::WebRedirect {
        redirect_uri: "https://kyrios.example.com/api/auth/google/callback".to_string(),
    };
    let web_client = provider_config
        .build_client(Some(web_flow.to_redirect_url().unwrap()))
        .unwrap();

    let web_url = build_authorization_url(
        &web_client,
        CsrfToken::new("csrf-state-abc".to_string()),
        AuthUrlOptions {
            scopes: &[
                "openid",
                "email",
                "https://www.googleapis.com/auth/calendar.events",
            ],
            pkce: None,
            offline_access: true,
            incremental: true,
            prompt: Some("consent"),
        },
    );

    let web_url_str = web_url.as_str();
    assert!(web_url_str.starts_with("https://accounts.google.com/o/oauth2/v2/auth"));
    assert!(web_url_str.contains("client_id=google-client-id-123.apps.googleusercontent.com"));
    assert!(web_url_str.contains("redirect_uri=https%3A%2F%2Fkyrios.example.com%2Fapi%2Fauth%2Fgoogle%2Fcallback"));
    assert!(web_url_str.contains("response_type=code"));
    assert!(web_url_str.contains("state=csrf-state-abc"));
    assert!(web_url_str.contains("access_type=offline"));
    assert!(web_url_str.contains("include_granted_scopes=true"));
    assert!(web_url_str.contains("prompt=consent"));
    assert!(web_url_str.contains("calendar.events"));

    // 2. Desktop loopback flow with PKCE
    let desktop_flow = OAuthFlowMode::DesktopLoopback {
        redirect_uri: "http://127.0.0.1:8765/callback".to_string(),
    };
    let desktop_client = provider_config
        .build_client(Some(desktop_flow.to_redirect_url().unwrap()))
        .unwrap();

    let (pkce_challenge, _) = PkceCodeChallenge::new_random_sha256();
    let desktop_url = build_authorization_url(
        &desktop_client,
        CsrfToken::new("desktop-state-xyz".to_string()),
        AuthUrlOptions {
            scopes: &["openid", "email"],
            pkce: Some(&pkce_challenge),
            offline_access: false,
            incremental: false,
            prompt: None,
        },
    );

    let desktop_url_str = desktop_url.as_str();
    assert!(desktop_url_str.contains("redirect_uri=http%3A%2F%2F127.0.0.1%3A8765%2Fcallback"));
    assert!(desktop_url_str.contains("code_challenge_method=S256"));
    assert!(desktop_url_str.contains(&format!("code_challenge={}", pkce_challenge.as_str())));

    // 3. Custom native URI scheme
    let native_flow = OAuthFlowMode::CustomScheme {
        redirect_uri: "kyrios://oauth/callback".to_string(),
    };
    let native_client = provider_config
        .build_client(Some(native_flow.to_redirect_url().unwrap()))
        .unwrap();

    let native_url = build_authorization_url(
        &native_client,
        CsrfToken::new("native-state".to_string()),
        AuthUrlOptions {
            scopes: &["openid"],
            pkce: None,
            offline_access: false,
            incremental: false,
            prompt: None,
        },
    );

    let native_url_str = native_url.as_str();
    assert!(native_url_str.contains("redirect_uri=kyrios%3A%2F%2Foauth%2Fcallback"));
}

#[test]
fn test_oauth_user_info_deserialization() {
    let json = r#"{
        "sub": "10987654321",
        "email": "abirbasak@example.com",
        "email_verified": true,
        "name": "Abir Basak",
        "picture": "https://lh3.googleusercontent.com/a/sample"
    }"#;

    #[derive(serde::Deserialize)]
    struct RawGoogleUserInfo {
        sub: String,
        email: String,
        email_verified: Option<bool>,
        name: Option<String>,
        picture: Option<String>,
    }

    let raw: RawGoogleUserInfo = serde_json::from_str(json).unwrap();
    let user_info = OAuthUserInfo {
        subject: raw.sub,
        email: raw.email,
        email_verified: raw.email_verified,
        name: raw.name,
        picture: raw.picture,
    };

    assert_eq!(user_info.subject, "10987654321");
    assert_eq!(user_info.email, "abirbasak@example.com");
    assert_eq!(user_info.email_verified, Some(true));
    assert_eq!(user_info.name.as_deref(), Some("Abir Basak"));
}

#[tokio::test]
async fn test_connected_account_repo_persistence() {
    let database_url = std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgres://kyrios:kyrios@localhost:5432/kyrios".to_string());

    let Ok(pool) = sqlx::PgPool::connect(&database_url).await else {
        eprintln!("Database not reachable, skipping repo persistence test");
        return;
    };

    let repo = megh::ConnectedAccountRepo::new(&pool);
    let account_id = format!("test_acc_{}", uuid::Uuid::new_v4());
    let email = format!("{}@example.com", account_id);

    let acc = ConnectedAccount {
        account_id: account_id.clone(),
        provider: "google".to_string(),
        email: Some(email.clone()),
        access_token: "token-123".to_string(),
        refresh_token: Some("refresh-123".to_string()),
        token_type: Some("Bearer".to_string()),
        expiry: Some(Utc::now() + Duration::hours(1)),
        created_at: None,
        updated_at: None,
        disconnected_at: None,
    };

    let saved = repo.save(&acc).await.expect("saving connected account");
    assert_eq!(saved.account_id, account_id);
    assert_eq!(saved.access_token, "token-123");

    // Lookup by PK
    let fetched = repo.get(&account_id, "google").await.expect("get account").expect("account exists");
    assert_eq!(fetched.email.as_deref(), Some(email.as_str()));

    // Lookup by email
    let by_email = repo.find_by_email_or_account(&email, "google").await.expect("find by email").expect("found");
    assert_eq!(by_email.account_id, account_id);

    // Disconnect
    let disconnected = repo.disconnect(&account_id, "google").await.expect("disconnect");
    assert!(disconnected);

    let after_disconnect = repo.find_by_email_or_account(&email, "google").await.expect("find after disconnect");
    assert!(after_disconnect.is_none());

    // Cleanup
    let _ = sqlx::query("DELETE FROM connected_accounts WHERE account_id = $1")
        .bind(&account_id)
        .execute(&pool)
        .await;
}
