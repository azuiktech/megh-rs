//! Generic booking: values and entities for appointments across domains (docs/TPD-booking.md).

pub mod value;
#[cfg(all(feature = "axum", feature = "postgres"))]
pub mod blockout;
#[cfg(all(feature = "axum", feature = "postgres"))]
pub mod client;
#[cfg(all(feature = "axum", feature = "postgres"))]
pub mod provider;
#[cfg(all(feature = "axum", feature = "postgres"))]
pub mod resource;
#[cfg(all(feature = "axum", feature = "postgres"))]
pub mod schedule;
#[cfg(all(feature = "axum", feature = "postgres"))]
pub mod service;

#[cfg(all(feature = "axum", feature = "postgres"))]
pub use api::{openapi, router};

#[cfg(all(feature = "axum", feature = "postgres"))]
mod api {
    use utoipa::openapi::security::{ApiKey, ApiKeyValue, SecurityScheme};
    use axum_tower_sessions_csrf::TOKEN_HEADER;
    use utoipa::{Modify, OpenApi};
    use utoipa_axum::router::OpenApiRouter;

    use crate::auth::rest::AppContext;
    use crate::auth::JWT_COOKIE;

    #[derive(OpenApi)]
    #[openapi(info(title = "megh booking", description = "Booking API of an org."), modifiers(&SignedIn), security(("session" = [], "csrf" = [])))]
    struct Booking;

    /// Every route needs the signed-in user's token, kept in the `jwt_token` cookie; writes also need the CSRF token.
    struct SignedIn;

    impl Modify for SignedIn {
        fn modify(&self, api: &mut utoipa::openapi::OpenApi) {
            let components = api.components.get_or_insert_with(Default::default);
            components.add_security_scheme("session", SecurityScheme::ApiKey(ApiKey::Cookie(ApiKeyValue::new(JWT_COOKIE))));
            components.add_security_scheme("csrf", SecurityScheme::ApiKey(ApiKey::Header(ApiKeyValue::new(TOKEN_HEADER))));
        }
    }

    /// The booking API's OpenAPI description: title and how requests are signed in.
    pub fn openapi() -> utoipa::openapi::OpenApi {
        Booking::openapi()
    }

    /// All booking routes, to be nested under a prefix such as `/api/v1`.
    pub fn router() -> OpenApiRouter<AppContext> {
        OpenApiRouter::new()
            .merge(super::resource::routes())
            .merge(super::provider::routes())
            .merge(super::client::routes())
            .merge(super::service::routes())
            .merge(super::schedule::routes())
            .merge(super::blockout::routes())
    }
}
