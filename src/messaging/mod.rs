//! In-app messaging between a user and the business's agent (docs/TPD-messaging.md).

#[cfg(all(feature = "axum", feature = "postgres"))]
mod attachment;
#[cfg(all(feature = "axum", feature = "postgres"))]
mod authorizer;
#[cfg(all(feature = "axum", feature = "postgres"))]
mod error;
#[cfg(all(feature = "axum", feature = "postgres"))]
mod guest;
#[cfg(all(feature = "axum", feature = "postgres"))]
mod history;
#[cfg(all(feature = "axum", feature = "postgres"))]
mod label;
#[cfg(all(feature = "axum", feature = "postgres"))]
mod message;
#[cfg(all(feature = "axum", feature = "postgres"))]
mod messaging;
#[cfg(all(feature = "axum", feature = "postgres"))]
mod participant;
#[cfg(all(feature = "axum", feature = "postgres"))]
mod receipt;
#[cfg(all(feature = "axum", feature = "postgres"))]
pub mod surface;
#[cfg(all(feature = "axum", feature = "postgres"))]
mod thread;

#[cfg(all(feature = "axum", feature = "postgres"))]
pub use attachment::{Attachment, AttachmentData, AttachmentSummary, Upload};
#[cfg(all(feature = "axum", feature = "postgres"))]
pub use authorizer::authorizer;
#[cfg(all(feature = "axum", feature = "postgres"))]
pub use error::MessagingError;
#[cfg(all(feature = "axum", feature = "postgres"))]
pub use guest::{Arrival, guest_router};
#[cfg(all(feature = "axum", feature = "postgres"))]
pub use history::History;
#[cfg(all(feature = "axum", feature = "postgres"))]
pub use label::{Label, LabelData, Labelled};
#[cfg(all(feature = "axum", feature = "postgres"))]
pub use message::{Content, Message, MessageData, MessageKind};
#[cfg(all(feature = "axum", feature = "postgres"))]
pub use messaging::Messaging;
#[cfg(all(feature = "axum", feature = "postgres"))]
pub use participant::{Participant, ParticipantData, Party};
#[cfg(all(feature = "axum", feature = "postgres"))]
pub use receipt::Unread;
#[cfg(all(feature = "axum", feature = "postgres"))]
pub use routes::{openapi, router};
#[cfg(all(feature = "axum", feature = "postgres"))]
pub use thread::{Thread, ThreadData, ThreadStatus};

#[cfg(all(feature = "axum", feature = "postgres"))]
mod routes {
    use utoipa::{Modify, OpenApi};
    use utoipa_axum::router::OpenApiRouter;

    use crate::auth::openapi::{Granted, SignedIn};
    use crate::auth::rest::AppContext;

    #[derive(OpenApi)]
    #[openapi(
        info(title = "megh messaging", description = "In-app messaging between a user and the business's agent: free text, attachments and a2ui 1.0 surfaces the user fills in. Needs an `Extension<Messaging>`."),
        tags(
            (name = "threads", description = "A conversation between its participants; `:seen` marks it read up to a message."),
            (name = "participants", description = "The members (staff, providers, AI agents) and clients taking part in a thread."),
            (name = "messages", description = "What participants say: text, a2ui surfaces and the answers to them (`:respond`)."),
            (name = "attachments", description = "Files of a thread: a raw upload, sent with a message by id."),
            (name = "labels", description = "A tenant's tags, put on threads and on messages."),
            (name = "typing", description = "Tells the thread's followers the caller is typing; nothing is stored."),
            (name = "guests", description = "Lets someone who is not a client yet talk: a guest client, a thread and a token for it. Public."),
        ),
        modifiers(&SignedIn),
        security(("session" = []), ("bearer" = [])),
    )]
    struct Messaging;

    /// The messaging API's OpenAPI description: title and how requests are signed in.
    pub fn openapi() -> utoipa::openapi::OpenApi {
        Messaging::openapi()
    }

    /// All messaging routes except the public guest one, to be nested under a prefix such as `/api/v1`, each
    /// documented with its grants. Takes the `Messaging` from an `Extension`.
    pub fn router() -> OpenApiRouter<AppContext> {
        let mut router = OpenApiRouter::new()
            .merge(super::thread::routes())
            .merge(super::participant::routes())
            .merge(super::message::routes())
            .merge(super::receipt::routes())
            .merge(super::attachment::routes())
            .merge(super::label::routes());
        Granted.modify(router.get_openapi_mut());
        router
    }
}
