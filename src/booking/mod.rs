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
            .merge(super::appointment::routes())
            .merge(super::availability::routes())
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
