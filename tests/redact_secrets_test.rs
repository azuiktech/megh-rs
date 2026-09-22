//! `Debug` never prints a secret; the secret-carrying types no longer derive `Serialize` at all (checked by
//! this file compiling, since it would not if any of them still did and something tried to serialize one).

use megh::account::{ConnectedAccount, OAuth2Tokens};
use megh::auth::OAuthProviderConfig;

#[test]
fn provider_config_debug_never_prints_the_client_secret() {
    let provider = OAuthProviderConfig::google("client-id-123", Some("s3cret-client-secret".to_string()));

    let printed = format!("{provider:?}");

    assert!(printed.contains("client-id-123"));
    assert!(!printed.contains("s3cret-client-secret"));
}

#[test]
fn a_provider_with_no_secret_configured_says_so() {
    let provider = OAuthProviderConfig::google("client-id-123", None);

    assert!(format!("{provider:?}").contains("client_secret: None"));
}

#[test]
fn connected_account_debug_never_prints_its_tokens() {
    let account = ConnectedAccount {
        account_id: "acct".into(),
        provider: "google".into(),
        email: Some("ann@example.com".into()),
        access_token: "s3cret-access-token".into(),
        refresh_token: Some("s3cret-refresh-token".into()),
        token_type: Some("Bearer".into()),
        expiry: None,
        created_at: None,
        updated_at: None,
        disconnected_at: None,
    };

    let printed = format!("{account:?}");

    assert!(printed.contains("acct") && printed.contains("ann@example.com"));
    assert!(!printed.contains("s3cret-access-token") && !printed.contains("s3cret-refresh-token"));
}

#[test]
fn oauth2_tokens_debug_never_prints_its_tokens() {
    let tokens = OAuth2Tokens { access_token: "s3cret-access".into(), refresh_token: Some("s3cret-refresh".into()), token_expires_at: None };

    let printed = format!("{tokens:?}");

    assert!(!printed.contains("s3cret-access") && !printed.contains("s3cret-refresh"));
}
