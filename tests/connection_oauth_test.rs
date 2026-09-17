use chrono::{Duration, Utc};
use megh::auth::{
    build_authorization_url, AuthUrlOptions, BasicTokenResponse, CsrfToken, OAuthFlowMode,
    OAuthProviderConfig, OAuthUserInfo, PkceCodeChallenge, TokenResponse,
};
use megh::connection::{
    Connection, ConnectionData, ConnectionExt, FullConnection, OAuth2Tokens,
};
use megh::Entity;
use uuid::Uuid;

#[test]
fn test_connection_scope_checks_and_deref() {
    let id = Uuid::new_v4();
    let user_id = Uuid::new_v4();
    let conn: Connection = Entity::new(
        id,
        FullConnection {
            data: ConnectionData {
                user_id,
                org_id: None,
                provider: "google".to_string(),
                provider_account_id: "google-uid-123".to_string(),
                scopes: vec![
                    "openid".to_string(),
                    "https://www.googleapis.com/auth/userinfo.email".to_string(),
                    "https://www.googleapis.com/auth/calendar.events".to_string(),
                ],
                metadata: serde_json::json!({ "email": "user@example.com" }),
            },
            tokens: OAuth2Tokens {
                access_token: "secret-access-token".to_string(),
                refresh_token: Some("secret-refresh-token".to_string()),
                token_expires_at: Some(Utc::now() + Duration::hours(1)),
            },
        },
    );

    // Deref gives direct field access
    assert_eq!(conn.provider, "google");
    assert_eq!(conn.provider_account_id, "google-uid-123");
    assert_eq!(conn.user_id, user_id);

    // Scope checking via Deref
    assert!(conn.has_scope("openid"));
    assert!(conn.has_scope("https://www.googleapis.com/auth/calendar.events"));
    assert!(!conn.has_scope("https://www.googleapis.com/auth/gmail.readonly"));

    assert!(conn.has_all_scopes(&[
        "openid",
        "https://www.googleapis.com/auth/calendar.events",
    ]));
    assert!(!conn.has_all_scopes(&[
        "openid",
        "https://www.googleapis.com/auth/gmail.readonly",
    ]));
}

#[test]
fn test_oauth2_tokens_expiration() {
    let now = Utc::now();
    let expired_tokens = OAuth2Tokens {
        access_token: "token".to_string(),
        refresh_token: None,
        token_expires_at: Some(now - Duration::seconds(10)),
    };
    assert!(expired_tokens.is_expired(0));

    let valid_tokens = OAuth2Tokens {
        access_token: "token".to_string(),
        refresh_token: None,
        token_expires_at: Some(now + Duration::seconds(300)),
    };
    assert!(!valid_tokens.is_expired(0));
    assert!(valid_tokens.is_expired(600));

    let no_expiry_tokens = OAuth2Tokens {
        access_token: "token".to_string(),
        refresh_token: None,
        token_expires_at: None,
    };
    assert!(!no_expiry_tokens.is_expired(0));
}

#[test]
fn test_connection_view_strips_secrets() {
    let id = Uuid::new_v4();
    let user_id = Uuid::new_v4();
    let conn: Connection = Entity::new(
        id,
        FullConnection {
            data: ConnectionData {
                user_id,
                org_id: None,
                provider: "google".to_string(),
                provider_account_id: "google-uid-123".to_string(),
                scopes: vec!["https://www.googleapis.com/auth/calendar".to_string()],
                metadata: serde_json::json!({ "email": "user@example.com" }),
            },
            tokens: OAuth2Tokens {
                access_token: "super-secret-access-token".to_string(),
                refresh_token: Some("super-secret-refresh-token".to_string()),
                token_expires_at: Some(Utc::now() + Duration::hours(1)),
            },
        },
    );

    let view = conn.to_view();
    let json = serde_json::to_string(&view).unwrap();

    assert!(!json.contains("super-secret-access-token"));
    assert!(!json.contains("super-secret-refresh-token"));
    assert!(json.contains("https://www.googleapis.com/auth/calendar"));
    assert!(json.contains("user@example.com"));
    assert_eq!(view.provider, "google");
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
