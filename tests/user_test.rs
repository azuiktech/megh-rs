use megh::{UpsertUserInput, UserRepo};
use sqlx::PgPool;

fn login(provider: &str, account_id: &str) -> UpsertUserInput {
    UpsertUserInput {
        provider: provider.into(),
        account_id: account_id.into(),
        email: "ann@example.com".into(),
        display_name: Some("Ann".into()),
        photo_url: None,
    }
}

#[sqlx::test]
async fn users_columns_match_megh_go(pool: PgPool) {
    let text_columns: Vec<String> = sqlx::query_scalar(
        "SELECT column_name FROM information_schema.columns
         WHERE table_name = 'users' AND data_type = 'text' ORDER BY column_name",
    )
    .fetch_all(&pool)
    .await
    .unwrap();

    assert_eq!(text_columns, ["account_id", "password_hash", "provider"]);
    let email: String = sqlx::query_scalar("SELECT udt_name::text FROM information_schema.columns WHERE table_name = 'users' AND column_name = 'email'")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(email, "citext", "emails compare without case");
}

#[sqlx::test]
async fn one_user_per_email_across_providers_keeps_first_identity(pool: PgPool) {
    let users = UserRepo::new(&pool);

    let google = users.upsert(&login("google", "g-1")).await.unwrap();
    let github = users.upsert(&login("github", "h-9")).await.unwrap();

    assert_eq!(github.id, google.id);
    assert_eq!(github.provider.as_deref(), Some("google"));
    assert_eq!(github.account_id.as_deref(), Some("g-1"));
}

#[sqlx::test]
async fn find_by_account_returns_the_user(pool: PgPool) {
    let users = UserRepo::new(&pool);
    let created = users.upsert(&login("google", "g-1")).await.unwrap();

    assert_eq!(users.find_by_account("google", "g-1").await.unwrap().map(|u| u.id), Some(created.id));
    assert!(users.find_by_account("github", "g-1").await.unwrap().is_none());
}
