//! OpenAPI pieces shared by the routers of megh's APIs: how a request is signed in, and the grants each route needs.

use axum_tower_sessions_csrf::TOKEN_HEADER;
use utoipa::Modify;
use utoipa::openapi::path::Operation;
use utoipa::openapi::security::{ApiKey, ApiKeyValue, SecurityRequirement, SecurityScheme};

use crate::auth::{JWT_COOKIE, request_grants};

/// Every route needs the signed-in user's token, kept in the `jwt_token` cookie; writes also need the CSRF token.
pub struct SignedIn;

impl Modify for SignedIn {
    fn modify(&self, api: &mut utoipa::openapi::OpenApi) {
        let components = api.components.get_or_insert_with(Default::default);
        components.add_security_scheme("session", SecurityScheme::ApiKey(ApiKey::Cookie(ApiKeyValue::new(JWT_COOKIE))));
        components.add_security_scheme("csrf", SecurityScheme::ApiKey(ApiKey::Header(ApiKeyValue::new(TOKEN_HEADER))));
    }
}

/// Each route's grants, as the authorizer derives them from its path, in the session scheme's scopes; a
/// write also needs the CSRF token.
pub struct Granted;

impl Modify for Granted {
    fn modify(&self, api: &mut utoipa::openapi::OpenApi) {
        api.paths.paths.iter_mut().for_each(|(path, item)| {
            [("GET", &mut item.get), ("POST", &mut item.post), ("PUT", &mut item.put), ("PATCH", &mut item.patch), ("DELETE", &mut item.delete)]
                .into_iter()
                .for_each(|(method, operation)| operation.iter_mut().for_each(|operation| grant(operation, method, path)));
        });
    }
}

fn grant(operation: &mut Operation, method: &str, path: &str) {
    let grants = request_grants(method, path, path).into_iter().map(|grant| grant.0);
    let session = SecurityRequirement::new("session", grants);
    let required = match method {
        "GET" => session,
        _ => session.add("csrf", Vec::<String>::new()),
    };
    operation.security = Some(vec![required]);
}
