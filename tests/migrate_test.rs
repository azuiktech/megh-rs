//! `Db::migrate` leaves every table megh maps ready for soft delete.

#[macro_use]
mod common;

use megh::Db;

const TABLES: [&str; 8] = ["users", "connected_accounts", "plans", "plan_prices", "add_ons", "plan_add_ons", "subscriptions", "subscription_items"];

async fn has_deleted_at(pool: &Db, table: &str) -> bool {
    Db::query_scalar("SELECT EXISTS (SELECT 1 FROM information_schema.columns WHERE table_name = $1 AND column_name = 'deleted_at')")
        .sqlite("SELECT EXISTS (SELECT 1 FROM pragma_table_info($1) WHERE name = 'deleted_at')")
        .bind(table)
        .fetch_one(pool)
        .await
        .unwrap()
}

dual!(migrating_an_empty_database_gives_every_mapped_table_deleted_at);
async fn migrating_an_empty_database_gives_every_mapped_table_deleted_at(pool: Db) {
    for table in TABLES {
        assert!(has_deleted_at(&pool, table).await, "{table}");
    }
}

dual!(migrating_twice_is_harmless);
async fn migrating_twice_is_harmless(pool: Db) {
    pool.migrate().await.unwrap();
    pool.migrate().await.unwrap();
}
