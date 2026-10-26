//! Issue #221 (#8 of the code review): an email is the same whatever its case, for sign-in, password login and the
//! accounts linked to a user.

use megh::{ConnectedAccount, ConnectedAccountRepo, UpsertUserInput, UserRepo};
use sqlx::PgPool;

fn login(provider: &str, account_id: &str, email: &str) -> UpsertUserInput {
    UpsertUserInput { provider: provider.into(), account_id: account_id.into(), email: email.into(), display_name: None, photo_url: None }
}

#[sqlx::test]
async fn signing_in_with_another_case_is_the_same_user(pool: PgPool) {
    let repo = UserRepo::new(&pool);
    let ann = repo.upsert(&login("password", "ann", "ann@example.com")).await.unwrap();

    let through_provider = repo.upsert(&login("google", "1", "Ann@Example.com")).await.unwrap();

    let users: i64 = sqlx::query_scalar("SELECT count(*) FROM users").fetch_one(&pool).await.unwrap();
    assert_eq!((through_provider.id, users), (ann.id, 1), "not a second user");
}

#[sqlx::test]
async fn password_login_ignores_the_case_of_the_email(pool: PgPool) {
    let repo = UserRepo::new(&pool);
    let ann = repo.upsert(&login("password", "ann", "ann@example.com")).await.unwrap();
    repo.set_password(ann.id, "s3cret-pass").await.unwrap();

    assert_eq!(repo.verify_password("ANN@example.com", "s3cret-pass").await.unwrap().id, ann.id);
    assert_eq!(repo.find_by_email("Ann@Example.COM").await.unwrap().map(|user| user.id), Some(ann.id));
}

#[sqlx::test]
async fn a_users_accounts_are_found_whatever_the_case(pool: PgPool) {
    let repo = ConnectedAccountRepo::new(&pool);
    let account = ConnectedAccount {
        account_id: "1".into(),
        provider: "google".into(),
        email: Some("Ann@Example.com".into()),
        access_token: "at".into(),
        refresh_token: None,
        token_type: None,
        expiry: None,
        created_at: None,
        updated_at: None,
        disconnected_at: None,
    };
    repo.save(&account).await.unwrap();

    assert_eq!(repo.list_by_email("ann@example.com").await.unwrap().len(), 1);
    assert!(repo.find_by_email_or_account("ANN@example.com", "google").await.unwrap().is_some());
}
