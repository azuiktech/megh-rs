//! Guests: someone who talks before being a client gets a client of kind `guest`, a thread and a token for that thread.

use axum::extract::Path;
use axum::{Extension, Json};
use serde::Serialize;
use utoipa::ToSchema;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;
use uuid::Uuid;

use super::error::MessagingError;
use super::messaging::Messaging;
use super::participant::{Participant, Party};
use super::thread::{Thread, start};
use crate::auth::JwtSession;
use crate::auth::rest::AppContext;
use crate::booking::client::{Client, ClientData, ClientKind};
use crate::booking::value::{Attributes, Contact, Gender, Name};
use crate::{Entity, TableEntity};

/// A guest as it arrives.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Arrival {
    pub client: Client,
    pub thread: Thread,
    pub participant: Participant,
}

impl Messaging {
    /// A new guest client (placeholder names made from the start of its id), its thread and its place in it.
    pub async fn arrive(&self, tenant_id: Uuid) -> Result<Arrival, MessagingError> {
        let id = Uuid::now_v7();
        let four = id.simple().to_string().chars().take(4).collect::<String>();
        let guest = |text: String| Name::try_new(text).expect("a guest's name is not empty");
        let data = ClientData {
            tenant_id,
            first_name: guest("Guest".into()),
            last_name: guest(four.clone()),
            name: guest(format!("Guest {four}")),
            role: None,
            kind: ClientKind::Guest,
            contact: Contact::default(),
            gender: Gender::Unknown,
            birth_date: None,
            address: None,
            attributes: Attributes::new(),
        };
        let mut transaction = self.pool.begin().await?;
        let client = TableEntity::insert(&Entity::new(id, data), &mut *transaction).await?;
        let (thread, participant) = start(&mut transaction, tenant_id, Party::Client(id), None).await?;
        transaction.commit().await?;
        self.announce_thread(&thread).await;
        Ok(Arrival { client, thread, participant })
    }
}

/// The public route that lets a guest in: `POST /tenants/{tenant_id}/guests`. Mount it outside the authorizer.
pub fn guest_router() -> OpenApiRouter<AppContext> {
    OpenApiRouter::new().routes(routes!(arrive))
}

#[derive(Debug, Serialize, ToSchema)]
struct Welcome {
    #[schema(value_type = Object)]
    client: Client,
    #[schema(value_type = Object)]
    thread: Thread,
    #[schema(value_type = Object)]
    participant: Participant,
    /// A bearer token that reaches this thread only.
    token: String,
    expires_in: u64,
}

/// A new guest with a thread and a token for it.
#[utoipa::path(
    post,
    path = "/tenants/{tenant_id}/guests",
    tag = "guests",
    params(("tenant_id" = Uuid, Path)),
    responses((status = 200, body = Welcome), (status = 404)),
)]
async fn arrive(
    Extension(messaging): Extension<Messaging>,
    Extension(jwt): Extension<JwtSession>,
    Path(tenant_id): Path<Uuid>,
) -> Result<Json<Welcome>, MessagingError> {
    let Arrival { client, thread, participant } = messaging.arrive(tenant_id).await?;
    let issued = jwt.guest(tenant_id, client.id, thread.id)?;
    Ok(Json(Welcome { client, thread, participant, token: issued.jwt, expires_in: issued.expires_in.as_secs() }))
}
