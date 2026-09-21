use megh::audit::{Audit, AuditAction, AuditTrigger};
use sqlx::{Connection, PgPool};
use uuid::Uuid;

async fn orders(pool: &PgPool) -> Uuid {
    sqlx::raw_sql("CREATE TABLE orders (id UUID PRIMARY KEY, total BIGINT)").execute(pool).await.unwrap();
    Uuid::new_v4()
}

async fn change(pool: &PgPool, id: Uuid) {
    sqlx::query("INSERT INTO orders VALUES ($1, 10)").bind(id).execute(pool).await.unwrap();
    sqlx::query("UPDATE orders SET total = 20 WHERE id = $1").bind(id).execute(pool).await.unwrap();
    sqlx::query("DELETE FROM orders WHERE id = $1").bind(id).execute(pool).await.unwrap();
}

async fn has_trigger(pool: &PgPool, name: &str) -> bool {
    sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM pg_trigger WHERE tgname = $1)").bind(name).fetch_one(pool).await.unwrap()
}

#[sqlx::test]
async fn audit_entries_has_the_columns_megh_go_created(pool: PgPool) {
    let columns: String = sqlx::query_scalar(
        "SELECT string_agg(column_name || ':' || data_type, ',' ORDER BY column_name) FROM information_schema.columns WHERE table_name = 'audit_entries'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();

    assert_eq!(columns, "action:text,actor_id:text,created_at:timestamp with time zone,id:uuid,new_data:text,old_data:text,record_id:text,table_name:text");
}

#[sqlx::test]
async fn a_shared_trigger_records_every_change_with_old_and_new_rows(pool: PgPool) {
    let (audit, id) = (Audit::new(pool.clone()), orders(&pool).await);
    let trigger = AuditTrigger::on("orders");
    audit.install(&trigger).await.unwrap();

    change(&pool, id).await;

    let history = audit.history(&trigger, &id.to_string()).await.unwrap();
    let actions: Vec<_> = history.iter().map(|entry| entry.action).collect();
    assert_eq!(actions, [AuditAction::Insert, AuditAction::Update, AuditAction::Delete]);
    assert!(history.iter().all(|entry| entry.table_name == "orders" && entry.actor_id.is_none()));
    assert!(history[0].old_data.0.is_null());
    assert_eq!(history[0].new_data.0["total"], 10);
    assert_eq!((history[1].old_data.0["total"].clone(), history[1].new_data.0["total"].clone()), (10.into(), 20.into()));
    assert!(history[2].new_data.0.is_null());
    assert!(has_trigger(&pool, "trg_orders_audit").await);
}

#[sqlx::test]
async fn the_actor_is_attributed_for_one_transaction_only(pool: PgPool) {
    let (audit, id) = (Audit::new(pool.clone()), orders(&pool).await);
    let trigger = AuditTrigger::on("orders");
    audit.install(&trigger).await.unwrap();
    let mut connection = pool.acquire().await.unwrap();
    sqlx::query("INSERT INTO orders VALUES ($1, 10)").bind(id).execute(&mut *connection).await.unwrap();

    let mut tx = connection.begin().await.unwrap();
    Audit::actor(&mut tx, "ann").await.unwrap();
    sqlx::query("UPDATE orders SET total = 20 WHERE id = $1").bind(id).execute(&mut *tx).await.unwrap();
    tx.commit().await.unwrap();
    sqlx::query("UPDATE orders SET total = 30 WHERE id = $1").bind(id).execute(&mut *connection).await.unwrap();

    let actors: Vec<_> = audit.history(&trigger, &id.to_string()).await.unwrap().into_iter().map(|entry| entry.actor_id).collect();
    assert_eq!(actors, [None, Some("ann".to_string()), None]);
}

#[sqlx::test]
async fn a_dedicated_trigger_writes_to_its_own_table(pool: PgPool) {
    let (audit, id) = (Audit::new(pool.clone()), orders(&pool).await);
    let trigger = AuditTrigger::dedicated("orders");
    audit.install(&trigger).await.unwrap();

    change(&pool, id).await;

    let own: i64 = sqlx::query_scalar("SELECT count(*) FROM orders_audit").fetch_one(&pool).await.unwrap();
    let shared: i64 = sqlx::query_scalar("SELECT count(*) FROM audit_entries").fetch_one(&pool).await.unwrap();
    assert_eq!((own, shared, audit.history(&trigger, &id.to_string()).await.unwrap().len()), (3, 0, 3));
}

#[sqlx::test]
async fn every_name_can_be_overridden(pool: PgPool) {
    sqlx::raw_sql("CREATE TABLE orders (ref TEXT PRIMARY KEY, total BIGINT)").execute(&pool).await.unwrap();
    let audit = Audit::new(pool.clone());
    let trigger = AuditTrigger::on("orders").audit_table("order_log").name("orders_watch").key("ref");
    audit.install(&trigger).await.unwrap();

    sqlx::query("INSERT INTO orders VALUES ('A-1', 10)").execute(&pool).await.unwrap();

    let history = audit.history(&trigger, "A-1").await.unwrap();
    assert_eq!((history.len(), has_trigger(&pool, "orders_watch").await, has_trigger(&pool, "trg_orders_audit").await), (1, true, false));
    let logged: i64 = sqlx::query_scalar("SELECT count(*) FROM order_log").fetch_one(&pool).await.unwrap();
    assert_eq!(logged, 1);
}

#[sqlx::test]
async fn installing_twice_is_harmless_and_an_existing_trigger_is_left_alone(pool: PgPool) {
    let (audit, id) = (Audit::new(pool.clone()), orders(&pool).await);
    sqlx::raw_sql(
        "CREATE FUNCTION noop() RETURNS TRIGGER AS $$ BEGIN RETURN NEW; END $$ LANGUAGE plpgsql;
         CREATE TRIGGER trg_orders_audit AFTER INSERT ON orders FOR EACH ROW EXECUTE FUNCTION noop()",
    )
    .execute(&pool)
    .await
    .unwrap();
    let trigger = AuditTrigger::on("orders");

    audit.install(&trigger).await.unwrap();
    audit.install(&trigger).await.unwrap();
    sqlx::query("INSERT INTO orders VALUES ($1, 10)").bind(id).execute(&pool).await.unwrap();

    assert!(audit.history(&trigger, &id.to_string()).await.unwrap().is_empty());
}

#[sqlx::test]
async fn a_hostile_table_name_is_quoted_not_executed(pool: PgPool) {
    orders(&pool).await;
    sqlx::raw_sql("CREATE TABLE victim (x INT)").execute(&pool).await.unwrap();
    let hostile = AuditTrigger::on("orders FOR EACH ROW EXECUTE FUNCTION megh_audit('audit_entries', 'id'); DROP TABLE victim; --");

    assert!(Audit::new(pool.clone()).install(&hostile).await.is_err());

    let survives: bool = sqlx::query_scalar("SELECT to_regclass('victim') IS NOT NULL").fetch_one(&pool).await.unwrap();
    assert!(survives);
}
