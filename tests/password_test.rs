use megh::{PasswordError, UpsertUserInput, User, UserRepo};
#[macro_use]
mod common;

use megh::Db;
use uuid::Uuid;

/// `bcrypt.GenerateFromPassword("s3cret-pass", bcrypt.DefaultCost)` from megh-go.
const HASH_FROM_MEGH_GO: &str = "$2a$10$G2o17wABpotsusQGkGe4Vu9jkYrufP2C8e9JcFQZZfuKHfVNWUMeC";

async fn ann(repo: &UserRepo<'_>) -> User {
    let login = UpsertUserInput {
        provider: "google".into(),
        account_id: "1".into(),
        email: "ann@example.com".into(),
        display_name: Some("Ann".into()),
        photo_url: None,
    };
    repo.upsert(&login).await.unwrap()
}

dual!(verify_returns_the_user_once_a_password_is_set);
async fn verify_returns_the_user_once_a_password_is_set(pool: Db) {
    let repo = UserRepo::new(&pool);
    let ann = ann(&repo).await;

    repo.set_password(ann.id, "s3cret-pass").await.unwrap();

    assert_eq!(repo.verify_password("ann@example.com", "s3cret-pass").await.unwrap().id, ann.id);
    let stored: String = Db::query_scalar("SELECT password_hash FROM users").fetch_one(&pool).await.unwrap();
    assert!(stored.starts_with("$2") && !stored.contains("s3cret-pass"));
}

dual!(verify_says_why_it_failed);
async fn verify_says_why_it_failed(pool: Db) {
    let repo = UserRepo::new(&pool);
    let ann = ann(&repo).await;

    assert!(matches!(repo.verify_password("nobody@example.com", "x").await, Err(PasswordError::UserNotFound)));
    assert!(matches!(repo.verify_password("ann@example.com", "x").await, Err(PasswordError::NoPassword)));
    repo.set_password(ann.id, "s3cret-pass").await.unwrap();
    assert!(matches!(repo.verify_password("ann@example.com", "wrong").await, Err(PasswordError::WrongPassword)));
    assert!(matches!(repo.set_password(Uuid::now_v7(), "x").await, Err(PasswordError::UserNotFound)));
}

dual!(verify_accepts_hashes_written_by_megh_go_and_treats_its_empty_string_as_no_password);
async fn verify_accepts_hashes_written_by_megh_go_and_treats_its_empty_string_as_no_password(pool: Db) {
    let repo = UserRepo::new(&pool);
    ann(&repo).await;

    Db::query("UPDATE users SET password_hash = ''").execute(&pool).await.unwrap();
    assert!(matches!(repo.verify_password("ann@example.com", "s3cret-pass").await, Err(PasswordError::NoPassword)));

    Db::query("UPDATE users SET password_hash = $1").bind(HASH_FROM_MEGH_GO).execute(&pool).await.unwrap();
    assert!(repo.verify_password("ann@example.com", "s3cret-pass").await.is_ok());
}
