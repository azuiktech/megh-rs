use megh::{UpsertUserInput, UserRepo};
use sqlx::PgPool;

const BEFORE_ALIGNMENT: [&str; 4] = [
    include_str!("../migrations/0001_create_org.sql"),
    include_str!("../migrations/0002_create_users.sql"),
    include_str!("../migrations/0003_create_connected_accounts.sql"),
    include_str!("../migrations/0004_create_sessions.sql"),
];
const ALIGN_USERS: &str = include_str!("../migrations/0005_align_users.sql");

fn login(provider: &str, account_id: &str) -> UpsertUserInput {
    UpsertUserInput {
        provider: provider.into(),
        account_id: account_id.into(),
        email: "ann@example.com".into(),
        display_name: Some("Ann".into()),
        photo_url: None,
    }
}

#[sqlx::test(migrations = false)]
async fn migration_moves_subject_into_account_and_provider(pool: PgPool) {
    for sql in BEFORE_ALIGNMENT {
        sqlx::raw_sql(sql).execute(&pool).await.unwrap();
    }
    sqlx::query("INSERT INTO users (subject, email) VALUES ('google:123', 'ann@example.com')")
        .execute(&pool)
        .await
        .unwrap();

    sqlx::raw_sql(ALIGN_USERS).execute(&pool).await.unwrap();
    sqlx::raw_sql(ALIGN_USERS).execute(&pool).await.unwrap();

    let (account_id, provider): (String, String) =
        sqlx::query_as("SELECT account_id, provider FROM users").fetch_one(&pool).await.unwrap();
    assert_eq!((account_id.as_str(), provider.as_str()), ("123", "google"));
    let has_subject: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM information_schema.columns WHERE table_name = 'users' AND column_name = 'subject')",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(!has_subject);
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

    assert_eq!(text_columns, ["account_id", "email", "password_hash", "provider"]);
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
