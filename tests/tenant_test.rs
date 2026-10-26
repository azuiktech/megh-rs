use megh::booking::value::Contact;
use megh::{Tenant, Tenants};
use serde_json::json;
use sqlx::PgPool;
use uuid::Uuid;

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
async fn tenant_tables_have_their_columns(pool: PgPool) {
    const TS: &str = "timestamp with time zone";
    assert_eq!(
        columns(&pool, "tenants").await,
        format!("address:jsonb,created_at:{TS},created_by:uuid,description:text,email:text,home_phone:text,id:uuid,mobile_phone:text,name:text,plan_id:uuid,sub_status:text,timezone:text,trial_ends_at:{TS},updated_at:{TS},work_phone:text")
    );
    assert_eq!(
        columns(&pool, "tenant_members").await,
        format!("grants:text,id:uuid,invited_by:uuid,joined_at:{TS},role:text,tenant_id:uuid,user_id:uuid")
    );
    assert_eq!(
        columns(&pool, "tenant_invites").await,
        format!("accepted_at:{TS},created_at:{TS},email:text,expires_at:{TS},id:uuid,invited_by:uuid,status:text,tenant_id:uuid,token_hash:text")
    );
    assert_eq!(columns(&pool, "tenant_configs").await, "auto_join_domains:text,tenant_id:uuid");

    let tables = table_names(&pool).await;
    for billing in ["plans", "plan_prices", "add_ons", "plan_add_ons", "subscriptions", "subscription_items"] {
        assert!(tables.iter().any(|t| t == billing), "missing {billing}");
    }
}

#[sqlx::test]
async fn memberships_reads_rows_written_directly(pool: PgPool) {
    let user = Uuid::now_v7();
    for (grants, joined) in [(Some(r#"["invoices:read"]"#), "2026-01-02"), (Some("[]"), "2026-01-01"), (None, "2026-01-03")] {
        let tenant: Uuid = sqlx::query_scalar("INSERT INTO tenants (name) VALUES ('Acme') RETURNING id").fetch_one(&pool).await.unwrap();
        sqlx::query("INSERT INTO tenant_members (id, tenant_id, user_id, grants, joined_at) VALUES ($1, $2, $3, $4, $5::date)")
            .bind(Uuid::now_v7())
            .bind(tenant)
            .bind(user)
            .bind(grants)
            .bind(joined)
            .execute(&pool)
            .await
            .unwrap();
    }

    let members = Tenants::new(pool).memberships(user).await.unwrap();

    let grants: Vec<_> = members.iter().map(|m| m.grants.clone()).collect();
    assert_eq!(grants, [vec![], vec!["invoices:read".to_string()], vec![]]);
    assert!(members.windows(2).all(|pair| pair[0].joined_at < pair[1].joined_at));
    assert!(members.iter().all(|m| m.user_id == user && m.role == "member"));
}

#[sqlx::test]
async fn a_tenant_keeps_a_contact_and_an_address(pool: PgPool) {
    let address = json!({"line_1": "1 Main St", "line_2": null, "city": "Austin", "region": "TX", "postal_code": "78701", "country": "US"});
    sqlx::query("INSERT INTO tenants (name, email, home_phone, work_phone, mobile_phone, address) VALUES ('Acme', 'hello@acme.test', '+14155552671', '+14155552672', '+14155552673', $1)")
        .bind(&address)
        .execute(&pool)
        .await
        .unwrap();

    let tenant: Tenant = sqlx::query_as("SELECT * FROM tenants").fetch_one(&pool).await.unwrap();

    let contact = tenant.contact;
    let phones = [contact.home_phone, contact.work_phone, contact.mobile_phone].map(|phone| phone.unwrap().to_string());
    assert_eq!((contact.email.unwrap().to_string().as_str(), phones), ("hello@acme.test", ["+14155552671".into(), "+14155552672".into(), "+14155552673".into()]));
    assert_eq!(serde_json::to_value(tenant.address).unwrap(), address);
}

#[sqlx::test]
async fn a_tenant_without_a_contact_or_an_address_reads_empty(pool: PgPool) {
    sqlx::query("INSERT INTO tenants (name) VALUES ('Acme')").execute(&pool).await.unwrap();

    let tenant: Tenant = sqlx::query_as("SELECT * FROM tenants").fetch_one(&pool).await.unwrap();

    assert_eq!((tenant.contact, tenant.address), (Contact::default(), None));
}

#[sqlx::test]
async fn migrating_again_keeps_a_contact_and_an_address(pool: PgPool) {
    megh::migrate(&pool).await.unwrap();
    sqlx::query("INSERT INTO tenants (name, email) VALUES ('Acme', 'hello@acme.test')").execute(&pool).await.unwrap();

    megh::migrate(&pool).await.unwrap();

    let email: String = sqlx::query_scalar("SELECT email FROM tenants").fetch_one(&pool).await.unwrap();
    assert_eq!(email, "hello@acme.test");
}
