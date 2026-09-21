use chrono::{Duration, Utc};
use megh::auth::{User, UserProfile};
use megh::session::{
    generate_session_token, hash_session_token, FullSession, Session, SessionData, SessionExt,
    SessionSecrets, SessionView,
};
use megh::Entity;
use uuid::Uuid;

#[test]
fn test_user_entity_deref_and_flat_json() {
    let id = Uuid::parse_str("a0000000-0000-0000-0000-000000000001").unwrap();
    let user: User = Entity::new(
        id,
        UserProfile {
            account_id: Some("1234".to_string()),
            provider: Some("google".to_string()),
            email: "abir@example.com".to_string(),
            display_name: "Abir Basak".to_string(),
            photo_url: "https://example.com/photo.png".to_string(),
        },
    );

    // Direct field access via Deref
    assert_eq!(user.id, id);
    assert_eq!(user.provider.as_deref(), Some("google"));
    assert_eq!(user.email, "abir@example.com");
    assert_eq!(user.display_name, "Abir Basak");
    assert_eq!(user.photo_url, "https://example.com/photo.png");

    // Serialization must be flat without nested "data" wrapper
    let json = serde_json::to_string(&user).unwrap();
    assert!(json.contains(r#""id":"a0000000-0000-0000-0000-000000000001""#));
    assert!(json.contains(r#""email":"abir@example.com""#));
    assert!(json.contains(r#""display_name":"Abir Basak""#));
    assert!(!json.contains(r#""data":{"#));

    let deserialized: User = serde_json::from_str(&json).unwrap();
    assert_eq!(user, deserialized);
}

#[test]
fn test_session_tokens_generation_and_hashing() {
    let t1 = generate_session_token();
    let t2 = generate_session_token();
    assert_ne!(t1, t2);
    assert!(!t1.is_empty());

    let h1 = hash_session_token(&t1);
    let h2 = hash_session_token(&t1);
    let h3 = hash_session_token(&t2);

    // Deterministic hashing
    assert_eq!(h1, h2);
    assert_eq!(h1.len(), 64);
    assert_ne!(h1, h3);
}

#[test]
fn test_session_entity_deref_and_expiration() {
    let user_id = Uuid::new_v4();
    let id = Uuid::new_v4();
    let active_session: Session = Entity::new(
        id,
        FullSession {
            data: SessionData {
                user_id,
                expires_at: Utc::now() + Duration::hours(2),
                user_agent: "Mozilla/5.0 (Macintosh)".to_string(),
                ip_address: "192.168.1.1".to_string(),
                metadata: serde_json::json!({ "theme": "dark" }),
            },
            secrets: SessionSecrets {
                token_hash: "hash12345678".to_string(),
            },
        },
    );

    // Deref access
    assert_eq!(active_session.id, id);
    assert_eq!(active_session.user_id, user_id);
    assert_eq!(active_session.user_agent, "Mozilla/5.0 (Macintosh)");
    assert_eq!(active_session.ip_address, "192.168.1.1");
    assert!(!active_session.is_expired());

    let expired_session: Session = Entity::new(
        Uuid::new_v4(),
        FullSession {
            data: SessionData {
                user_id,
                expires_at: Utc::now() - Duration::minutes(5),
                user_agent: "curl/7.68.0".to_string(),
                ip_address: "10.0.0.1".to_string(),
                metadata: serde_json::Value::Null,
            },
            secrets: SessionSecrets {
                token_hash: "hash_expired".to_string(),
            },
        },
    );
    assert!(expired_session.is_expired());
}

#[test]
fn test_session_view_strips_secret_token_hash() {
    let user_id = Uuid::new_v4();
    let id = Uuid::new_v4();
    let session: Session = Entity::new(
        id,
        FullSession {
            data: SessionData {
                user_id,
                expires_at: Utc::now() + Duration::days(7),
                user_agent: "Chrome/120.0".to_string(),
                ip_address: "127.0.0.1".to_string(),
                metadata: serde_json::json!({ "role": "admin" }),
            },
            secrets: SessionSecrets {
                token_hash: "super-secret-token-hash-999".to_string(),
            },
        },
    );

    let view: SessionView = session.to_view();
    assert_eq!(view.id, id);
    assert_eq!(view.user_id, user_id);
    assert_eq!(view.user_agent, "Chrome/120.0");

    let json = serde_json::to_string(&view).unwrap();
    assert!(!json.contains("super-secret-token-hash-999"));
    assert!(!json.contains("token_hash"));
    assert!(json.contains("127.0.0.1"));
    assert!(json.contains("Chrome/120.0"));
}

#[cfg(feature = "postgres")]
#[tokio::test]
async fn test_session_repo_construction() {
    let pool = sqlx::PgPool::connect_lazy("postgres://localhost/test").unwrap();
    let repo = megh::session::SessionRepo::new(&pool);
    assert!(repo.list_by_user(Uuid::new_v4()).await.is_err());
}
