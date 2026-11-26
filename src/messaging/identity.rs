//! Client identity: a guest is a client with placeholder names, kept in the browser's session; when the chat shows who
//! the guest is, its messages and attachments move to that client and the guest is soft-deleted with where it moved.

use axum::extract::{Path, State};
use axum::{Extension, Json};
use axum_extra::extract::cookie::CookieJar;
use futures_util::{TryStreamExt, stream};
use serde::Serialize;
use crate::Db;
use tower_sessions::Session;
use utoipa::ToSchema;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;
use uuid::Uuid;

use super::message::{Messaging, MessagingError, Party, Signal, scope};
use crate::Topic;
use crate::auth::rest::AppContext;
use crate::auth::{AccessToken, JwtSession};

/// The session key of the browser's client: the tenant and the client.
pub(crate) const CLIENT: &str = "client";

/// The live client `client` stands for now: itself, or the client it moved to.
pub(crate) async fn current(pool: &Db, tenant_id: Uuid, client: Uuid) -> Result<Option<Uuid>, sqlx::Error> {
    Db::query_scalar("SELECT COALESCE(moved_to, id) FROM clients WHERE tenant_id = $1 AND id = $2 AND (deleted_at IS NULL OR moved_to IS NOT NULL)")
        .bind(tenant_id)
        .bind(client)
        .fetch_optional(pool)
        .await
}

async fn new_guest(pool: &Db, tenant_id: Uuid) -> Result<Uuid, sqlx::Error> {
    Db::query_scalar("INSERT INTO clients (tenant_id, id, first_name, last_name, name) VALUES ($1, $2, 'Guest', 'Guest', 'Guest') RETURNING id")
        .bind(tenant_id)
        .bind(Uuid::now_v7())
        .fetch_one(pool)
        .await
}

impl Messaging {
    /// Moves the guest's messages and attachments to `client` and soft-deletes the guest with where it moved; tells
    /// each member the guest talked to, on the guest's topic, so an open widget switches to the client.
    pub async fn identify(&self, guest: Uuid, client: Uuid) -> Result<(), MessagingError> {
        let mut moving = self.pool.begin().await?;
        let removed = Db::query("UPDATE clients SET deleted_at = now(), moved_to = $3 WHERE tenant_id = $1 AND id = $2 AND deleted_at IS NULL").sqlite("UPDATE clients SET deleted_at = strftime('%Y-%m-%dT%H:%M:%f+00:00'), moved_to = $3 WHERE tenant_id = $1 AND id = $2 AND deleted_at IS NULL")
            .bind(self.tenant_id)
            .bind(guest)
            .bind(client)
            .execute(&mut moving)
            .await?;
        (removed.rows_affected() == 1).then_some(()).ok_or(MessagingError::NotFound)?;
        let members: Vec<Uuid> = Db::query_scalar(
            "WITH moved AS (UPDATE messages SET client_id = $3 WHERE tenant_id = $1 AND client_id = $2 RETURNING member_id) SELECT DISTINCT member_id FROM moved",
        )
        .sqlite(
            "SELECT DISTINCT member_id FROM messages WHERE tenant_id = $1 AND client_id = $2; \
             UPDATE messages SET client_id = $3 WHERE tenant_id = $1 AND client_id = $2",
        )
        .bind(self.tenant_id)
        .bind(guest)
        .bind(client)
        .fetch_all(&mut moving)
        .await?;
        Db::query("UPDATE attachments SET client_id = $3 WHERE tenant_id = $1 AND client_id = $2")
            .bind(self.tenant_id)
            .bind(guest)
            .bind(client)
            .execute(&mut moving)
            .await?;
        moving.commit().await?;
        let told = |member| self.send_event(Signal::Moved(client), Party::Member(member), Party::Client(guest));
        stream::iter(members.into_iter().map(Ok)).try_for_each(told).await
    }
}

/// The broker's authorizer for messaging: a token follows only the topics in its tenant whose client or member it is.
pub fn follows(token: Option<&AccessToken>, topic: &Topic) -> bool {
    let own = |token: &AccessToken, (tenant, client, member): (Uuid, Option<Uuid>, Option<Uuid>)| {
        token.aud.contains(&tenant.to_string()) && [client, member].contains(&Some(token.sub))
    };
    token.zip(scope(topic)).is_some_and(|(token, parts)| own(token, parts))
}

/// The public guest route, mounted inside the session layer: `POST /tenants/{tenant_id}/guests`.
pub fn guests(jwt: JwtSession) -> OpenApiRouter<AppContext> {
    OpenApiRouter::new().routes(routes!(guest)).layer(Extension(jwt))
}

#[derive(Serialize, ToSchema)]
struct Guest {
    client_id: Uuid,
}

/// The browser's client in the tenant: the one its session holds (or the client it moved to), or a new guest; sets
/// the token cookie for it.
#[utoipa::path(
    post,
    path = "/tenants/{tenant_id}/guests",
    tag = "messages",
    params(("tenant_id" = Uuid, Path)),
    responses((status = 200, body = Guest)),
)]
async fn guest(
    State(ctx): State<AppContext>,
    Extension(jwt): Extension<JwtSession>,
    session: Session,
    jar: CookieJar,
    Path(tenant_id): Path<Uuid>,
) -> Result<(CookieJar, Json<Guest>), MessagingError> {
    let kept: Option<(Uuid, Uuid)> = session.get(CLIENT).await?;
    let known = match kept.filter(|(tenant, _)| *tenant == tenant_id) {
        Some((_, client)) => current(&ctx.pool, tenant_id, client).await?,
        None => None,
    };
    let client_id = match known {
        Some(client) => client,
        None => new_guest(&ctx.pool, tenant_id).await?,
    };
    session.insert(CLIENT, (tenant_id, client_id)).await?;
    Ok((jwt.client_cookie(jar, tenant_id, client_id)?, Json(Guest { client_id })))
}
