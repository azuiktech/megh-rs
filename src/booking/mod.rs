//! Generic booking: values and entities for appointments across domains (docs/TPD-booking.md).

pub mod value;
#[cfg(all(feature = "axum", feature = "postgres"))]
pub mod appointment;
#[cfg(all(feature = "axum", feature = "postgres"))]
pub mod availability;
#[cfg(all(feature = "axum", feature = "postgres"))]
pub mod blockout;
#[cfg(all(feature = "axum", feature = "postgres"))]
pub mod client;
#[cfg(all(feature = "axum", feature = "postgres"))]
pub mod provider;
#[cfg(all(feature = "axum", feature = "postgres"))]
pub mod reservation;
#[cfg(all(feature = "axum", feature = "postgres"))]
pub mod resource;
#[cfg(all(feature = "axum", feature = "postgres"))]
pub mod schedule;
#[cfg(all(feature = "axum", feature = "postgres"))]
pub mod service;

#[cfg(all(feature = "axum", feature = "postgres"))]
pub use api::{openapi, rewrite_aip_136, router};

#[cfg(all(feature = "axum", feature = "postgres"))]
mod api {
    use axum::extract::{OriginalUri, Request};
    use axum::http::Uri;
    use utoipa::{Modify, OpenApi};
    use utoipa_axum::router::OpenApiRouter;

    use crate::auth::rest::AppContext;
    use crate::auth::openapi::{Granted, SignedIn};

    #[derive(OpenApi)]
    #[openapi(
        info(title = "megh booking", description = include_str!("openapi.md")),
        tags(
            (name = "resources", description = "Rooms, chairs, seats, tables, pods: what an appointment takes up. `capacity` is how many at once, `waitlist` how many may wait."),
            (name = "providers", description = "The people an appointment is with. A service step names the `role` it needs."),
            (name = "clients", description = "Whom an appointment is for: a person or a pet."),
            (name = "services", description = "What is booked: setup and cleanup take only the room; each step takes the client and up to two provider roles."),
            (name = "schedules", description = "Open time: the days a recurrence gives, from `starts_at` to `ends_at`, for a provider, a room and services, or any when left out."),
            (name = "blockouts", description = "Time taken out of the schedules: holidays, lunch, a broken instrument."),
            (name = "appointments", description = "Booked time of one client, one service, one room and its providers. Status moves through `:book`, `:confirm`, `:complete`, and DELETE cancels."),
            (name = "reservations", description = "Holds: a slot's places kept for 10 minutes while it is being booked; not an appointment."),
        ),
        modifiers(&SignedIn),
        security(("session" = [])),
    )]
    struct Booking;

    /// The booking API's OpenAPI description: title and how requests are signed in.
    pub fn openapi() -> utoipa::openapi::OpenApi {
        Booking::openapi()
    }

    /// All booking routes, to be nested under a prefix such as `/api/v1`, each documented with its grants.
    pub fn router() -> OpenApiRouter<AppContext> {
        let mut router = OpenApiRouter::new()
            .merge(super::resource::routes())
            .merge(super::provider::routes())
            .merge(super::client::routes())
            .merge(super::service::routes())
            .merge(super::schedule::routes())
            .merge(super::blockout::routes())
            .merge(super::appointment::routes())
            .merge(super::availability::routes())
            .merge(super::reservation::routes());
        Granted.modify(router.get_openapi_mut());
        router
    }

    /// AIP-136 custom methods on an item: routes `…/{id}:verb` as `…/{id}/:verb`, keeping the original URI for the
    /// authorizer. A method on a collection (`…/appointments:available`) is one literal segment, routed as it is.
    /// Any other `:` is rewritten too, so the routed form `…/{id}/:verb` cannot be called directly.
    ///
    /// Wraps the whole app, as routing happens before any `Router::layer`:
    /// `MapRequestLayer::new(megh::booking::rewrite_aip_136).layer(app)`.
    pub fn rewrite_aip_136(mut request: Request) -> Request {
        let original = request.uri().clone();
        if let Some(uri) = custom_method_uri(&original) {
            request.extensions_mut().get_or_insert_with(|| OriginalUri(original));
            *request.uri_mut() = uri;
        }
        request
    }

    fn custom_method_uri(uri: &Uri) -> Option<Uri> {
        let (parent, last) = uri.path().rsplit_once('/')?;
        let (resource, verb) = last.split_once(':')?;
        let collection = !resource.is_empty() && uuid::Uuid::try_parse(resource).is_err();
        (!collection).then_some(())?;
        let query = uri.query().map(|query| format!("?{query}")).unwrap_or_default();
        format!("{parent}/{resource}/:{verb}{query}").parse().ok()
    }
}
