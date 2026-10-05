//! `megh::migrate` leaves every table megh maps ready for soft delete.

use sqlx::PgPool;

const TABLES: [&str; 8] = ["users", "connected_accounts", "plans", "plan_prices", "add_ons", "plan_add_ons", "subscriptions", "subscription_items"];

async fn has_deleted_at(pool: &PgPool, table: &str) -> bool {
    sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM information_schema.columns WHERE table_name = $1 AND column_name = 'deleted_at')")
        .bind(table)
        .fetch_one(pool)
        .await
        .unwrap()
}

#[sqlx::test(migrations = false)]
async fn migrating_an_empty_database_gives_every_mapped_table_deleted_at(pool: PgPool) {
    megh::migrate(&pool).await.unwrap();
    for table in TABLES {
        assert!(has_deleted_at(&pool, table).await, "{table}");
    }
}

#[sqlx::test(migrations = false)]
async fn migrating_twice_is_harmless(pool: PgPool) {
    megh::migrate(&pool).await.unwrap();
    megh::migrate(&pool).await.unwrap();
}
