//! A soft-deleted user (`deleted_at` set) is gone for every lookup, and cannot log in.

use megh::{PasswordError, UpsertUserInput, User, UserRepo};
use sqlx::PgPool;

async fn deleted_ann(pool: &PgPool) -> User {
    let repo = UserRepo::new(pool);
    let login = UpsertUserInput { provider: "google".into(), account_id: "1".into(), email: "ann@example.com".into(), display_name: None, photo_url: None };
    let ann = repo.upsert(&login).await.unwrap();
    repo.set_password(ann.id, "s3cret-pass").await.unwrap();
    sqlx::query("UPDATE users SET deleted_at = NOW() WHERE id = $1").bind(ann.id).execute(pool).await.unwrap();
    ann
}

#[sqlx::test]
async fn a_deleted_user_is_not_found_by_id(pool: PgPool) {
    let ann = deleted_ann(&pool).await;

    assert!(UserRepo::new(&pool).get_by_id(ann.id).await.unwrap().is_none());
}

#[sqlx::test]
async fn a_deleted_user_is_not_found_by_email(pool: PgPool) {
    deleted_ann(&pool).await;

    assert!(UserRepo::new(&pool).find_by_email("ann@example.com").await.unwrap().is_none());
}

#[sqlx::test]
async fn a_deleted_user_is_not_found_by_provider_account(pool: PgPool) {
    deleted_ann(&pool).await;

    assert!(UserRepo::new(&pool).find_by_account("google", "1").await.unwrap().is_none());
}

#[sqlx::test]
async fn a_deleted_user_cannot_log_in_with_the_right_password(pool: PgPool) {
    deleted_ann(&pool).await;

    let result = UserRepo::new(&pool).verify_password("ann@example.com", "s3cret-pass").await;

    assert!(matches!(result, Err(PasswordError::UserNotFound)), "{result:?}");
}

#[sqlx::test]
async fn signing_in_again_does_not_bring_a_deleted_user_back(pool: PgPool) {
    let ann = deleted_ann(&pool).await;
    let login = UpsertUserInput { provider: "google".into(), account_id: "1".into(), email: "ann@example.com".into(), display_name: None, photo_url: None };

    let result = UserRepo::new(&pool).upsert(&login).await;

    assert!(result.is_err(), "{result:?}");
    let still_deleted: bool = sqlx::query_scalar("SELECT deleted_at IS NOT NULL FROM users WHERE id = $1").bind(ann.id).fetch_one(&pool).await.unwrap();
    assert!(still_deleted);
}
