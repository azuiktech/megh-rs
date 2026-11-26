//! A soft-deleted user (`deleted_at` set) is gone for every lookup, and cannot log in.

#[macro_use]
mod common;

use megh::{PasswordError, UpsertUserInput, User, UserRepo};
use megh::Db;

async fn deleted_ann(pool: &Db) -> User {
    let repo = UserRepo::new(pool);
    let login = UpsertUserInput { provider: "google".into(), account_id: "1".into(), email: "ann@example.com".into(), display_name: None, photo_url: None };
    let ann = repo.upsert(&login).await.unwrap();
    repo.set_password(ann.id, "s3cret-pass").await.unwrap();
    Db::query("UPDATE users SET deleted_at = NOW() WHERE id = $1").sqlite("UPDATE users SET deleted_at = strftime('%Y-%m-%dT%H:%M:%f+00:00') WHERE id = $1").bind(ann.id).execute(pool).await.unwrap();
    ann
}

dual!(a_deleted_user_is_not_found_by_id);
async fn a_deleted_user_is_not_found_by_id(pool: Db) {
    let ann = deleted_ann(&pool).await;

    assert!(UserRepo::new(&pool).get_by_id(ann.id).await.unwrap().is_none());
}

dual!(a_deleted_user_is_not_found_by_email);
async fn a_deleted_user_is_not_found_by_email(pool: Db) {
    deleted_ann(&pool).await;

    assert!(UserRepo::new(&pool).find_by_email("ann@example.com").await.unwrap().is_none());
}

dual!(a_deleted_user_is_not_found_by_provider_account);
async fn a_deleted_user_is_not_found_by_provider_account(pool: Db) {
    deleted_ann(&pool).await;

    assert!(UserRepo::new(&pool).find_by_account("google", "1").await.unwrap().is_none());
}

dual!(a_deleted_user_cannot_log_in_with_the_right_password);
async fn a_deleted_user_cannot_log_in_with_the_right_password(pool: Db) {
    deleted_ann(&pool).await;

    let result = UserRepo::new(&pool).verify_password("ann@example.com", "s3cret-pass").await;

    assert!(matches!(result, Err(PasswordError::UserNotFound)), "{result:?}");
}

dual!(signing_in_again_does_not_bring_a_deleted_user_back);
async fn signing_in_again_does_not_bring_a_deleted_user_back(pool: Db) {
    let ann = deleted_ann(&pool).await;
    let login = UpsertUserInput { provider: "google".into(), account_id: "1".into(), email: "ann@example.com".into(), display_name: None, photo_url: None };

    let result = UserRepo::new(&pool).upsert(&login).await;

    assert!(result.is_err(), "{result:?}");
    let still_deleted: bool = Db::query_scalar("SELECT deleted_at IS NOT NULL FROM users WHERE id = $1").bind(ann.id).fetch_one(&pool).await.unwrap();
    assert!(still_deleted);
}

dual!(a_deleted_user_gets_no_new_password_or_profile);
async fn a_deleted_user_gets_no_new_password_or_profile(pool: Db) {
    let ann = deleted_ann(&pool).await;
    let repo = UserRepo::new(&pool);

    assert!(matches!(repo.set_password(ann.id, "new-pass").await, Err(PasswordError::UserNotFound)));
    assert!(matches!(repo.update_profile(ann.id, "Mallory", "x").await, Err(sqlx::Error::RowNotFound)));

    let (hash, name): (String, String) = Db::query_as("SELECT password_hash, display_name FROM users WHERE id = $1").bind(ann.id).fetch_one(&pool).await.unwrap();
    assert!(bcrypt::verify("s3cret-pass", &hash).unwrap() && name.is_empty(), "the stored row is unchanged");
}
