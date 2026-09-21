use megh::{Org, Orgs};
use sqlx::PgPool;
use uuid::Uuid;

const BEFORE_ORG_ALIGNMENT: [&str; 5] = [
    include_str!("../migrations/0001_create_org.sql"),
    include_str!("../migrations/0002_create_users.sql"),
    include_str!("../migrations/0003_create_connected_accounts.sql"),
    include_str!("../migrations/0004_create_sessions.sql"),
    include_str!("../migrations/0005_align_users.sql"),
];
const ALIGN_ORG: &str = include_str!("../migrations/0006_align_org.sql");

async fn table_names(pool: &PgPool) -> Vec<String> {
    sqlx::query_scalar("SELECT table_name FROM information_schema.tables WHERE table_schema = 'public' ORDER BY 1")
        .fetch_all(pool)
        .await
        .unwrap()
}

async fn columns(pool: &PgPool, table: &str) -> String {
    sqlx::query_scalar(
        "SELECT string_agg(column_name || ':' || data_type, ',' ORDER BY column_name)
         FROM information_schema.columns WHERE table_schema = 'public' AND table_name = $1",
    )
    .bind(table)
    .fetch_one(pool)
    .await
    .unwrap()
}

#[sqlx::test]
async fn tables_match_megh_go(pool: PgPool) {
    const TS: &str = "timestamp with time zone";
    assert_eq!(
        columns(&pool, "organizations").await,
        format!("created_at:{TS},created_by:uuid,description:text,id:uuid,name:text,plan_id:uuid,sub_status:text,timezone:text,trial_ends_at:{TS},updated_at:{TS}")
    );
    assert_eq!(
        columns(&pool, "organization_members").await,
        format!("grants:text,id:uuid,invited_by:uuid,joined_at:{TS},organization_id:uuid,role:text,user_id:uuid")
    );
    assert_eq!(
        columns(&pool, "organization_invites").await,
        format!("accepted_at:{TS},created_at:{TS},email:text,expires_at:{TS},id:uuid,invited_by:uuid,organization_id:uuid,status:text,token_hash:text")
    );
    assert_eq!(columns(&pool, "org_configs").await, "auto_join_domains:text,organization_id:uuid");

    let tables = table_names(&pool).await;
    for billing in ["plans", "plan_prices", "add_ons", "plan_add_ons", "subscriptions", "subscription_items"] {
        assert!(tables.iter().any(|t| t == billing), "missing {billing}");
    }
    assert!(!tables.iter().any(|t| t == "org" || t == "org_members"));
}

async fn legacy_org_with_member(pool: &PgPool) -> Uuid {
    for sql in BEFORE_ORG_ALIGNMENT {
        sqlx::raw_sql(sql).execute(pool).await.unwrap();
    }
    let org = Uuid::new_v4();
    sqlx::query("INSERT INTO org (id, name) VALUES ($1, 'Acme')").bind(org).execute(pool).await.unwrap();
    sqlx::query("INSERT INTO org_members (organization_id, user_id, grants) VALUES ($1, $2, '{invoices:read,invoices:create}')")
        .bind(org)
        .bind(Uuid::new_v4())
        .execute(pool)
        .await
        .unwrap();
    org
}

#[sqlx::test(migrations = false)]
async fn migration_renames_legacy_tables_and_keeps_their_dependents(pool: PgPool) {
    let org = legacy_org_with_member(&pool).await;
    sqlx::raw_sql("CREATE TABLE courses (org_id UUID REFERENCES org (id))").execute(&pool).await.unwrap();

    sqlx::raw_sql(ALIGN_ORG).execute(&pool).await.unwrap();
    sqlx::raw_sql(ALIGN_ORG).execute(&pool).await.unwrap();

    let name: String = sqlx::query_scalar("SELECT name FROM organizations WHERE id = $1").bind(org).fetch_one(&pool).await.unwrap();
    let grants: String = sqlx::query_scalar("SELECT grants FROM organization_members WHERE organization_id = $1").bind(org).fetch_one(&pool).await.unwrap();
    assert_eq!(name, "Acme");
    assert_eq!(grants, r#"["invoices:read","invoices:create"]"#);
    assert!(sqlx::query("INSERT INTO courses (org_id) VALUES ($1)").bind(Uuid::new_v4()).execute(&pool).await.is_err());
}

#[sqlx::test(migrations = false)]
async fn migration_copies_into_existing_megh_go_tables_and_keeps_legacy_ones(pool: PgPool) {
    let org = legacy_org_with_member(&pool).await;
    sqlx::raw_sql(
        "CREATE TABLE organizations (id UUID PRIMARY KEY, name TEXT, description TEXT, timezone TEXT, created_at TIMESTAMPTZ, updated_at TIMESTAMPTZ);
         CREATE TABLE organization_members (id UUID PRIMARY KEY, organization_id UUID, user_id UUID, grants TEXT, joined_at TIMESTAMPTZ)",
    )
    .execute(&pool)
    .await
    .unwrap();

    sqlx::raw_sql(ALIGN_ORG).execute(&pool).await.unwrap();
    sqlx::raw_sql(ALIGN_ORG).execute(&pool).await.unwrap();

    let copied: i64 = sqlx::query_scalar("SELECT count(*) FROM organizations WHERE id = $1").bind(org).fetch_one(&pool).await.unwrap();
    let members: i64 = sqlx::query_scalar("SELECT count(*) FROM organization_members WHERE organization_id = $1").bind(org).fetch_one(&pool).await.unwrap();
    let legacy_left: i64 = sqlx::query_scalar("SELECT count(*) FROM org").fetch_one(&pool).await.unwrap();
    assert_eq!((copied, members, legacy_left), (1, 1, 1));
}

#[sqlx::test]
async fn memberships_reads_rows_written_by_megh_go(pool: PgPool) {
    let user = Uuid::new_v4();
    for (grants, joined) in [(Some(r#"["invoices:read"]"#), "2026-01-02"), (Some("[]"), "2026-01-01"), (None, "2026-01-03")] {
        let org: Uuid = sqlx::query_scalar("INSERT INTO organizations (name) VALUES ('Acme') RETURNING id").fetch_one(&pool).await.unwrap();
        sqlx::query("INSERT INTO organization_members (id, organization_id, user_id, grants, joined_at) VALUES ($1, $2, $3, $4, $5::date)")
            .bind(Uuid::new_v4())
            .bind(org)
            .bind(user)
            .bind(grants)
            .bind(joined)
            .execute(&pool)
            .await
            .unwrap();
    }

    let members = Orgs::new(pool).memberships(user).await.unwrap();

    let grants: Vec<_> = members.iter().map(|m| m.grants.clone()).collect();
    assert_eq!(grants, [vec![], vec!["invoices:read".to_string()], vec![]]);
    assert!(members.windows(2).all(|pair| pair[0].joined_at < pair[1].joined_at));
    assert!(members.iter().all(|m| m.user_id == user && m.role == "member"));
}

#[sqlx::test]
async fn org_reads_rows_written_by_megh_go(pool: PgPool) {
    sqlx::query("INSERT INTO organizations (id, name, created_at, updated_at) VALUES ($1, NULL, NOW(), NULL)")
        .bind(Uuid::new_v4())
        .execute(&pool)
        .await
        .unwrap();

    let org: Org = sqlx::query_as("SELECT * FROM organizations").fetch_one(&pool).await.unwrap();

    assert_eq!((org.name, org.updated_at, org.timezone.as_str(), org.sub_status.as_str()), (None, None, "UTC", "trialing"));
}
