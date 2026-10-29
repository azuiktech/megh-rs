//! OpenAPI pieces shared by the routers of megh's APIs: how a request is signed in, and the grants each route needs.

use utoipa::Modify;
use utoipa::openapi::path::Operation;
use utoipa::openapi::security::{ApiKey, ApiKeyValue, HttpAuthScheme, HttpBuilder, SecurityRequirement, SecurityScheme};

use crate::auth::{JWT_COOKIE, request_grants};

/// Every route needs the signed-in user's token, in the `jwt_token` cookie or as an `Authorization: Bearer` header.
pub struct SignedIn;

impl Modify for SignedIn {
    fn modify(&self, api: &mut utoipa::openapi::OpenApi) {
        let components = api.components.get_or_insert_with(Default::default);
        components.add_security_scheme("session", SecurityScheme::ApiKey(ApiKey::Cookie(ApiKeyValue::new(JWT_COOKIE))));
        components.add_security_scheme("bearer", SecurityScheme::Http(HttpBuilder::new().scheme(HttpAuthScheme::Bearer).bearer_format("JWT").build()));
    }
}

/// Each route's grants, as the authorizer derives them from its path, in the session scheme's scopes; and an
/// operation id unique across routers, `{tag}_{handler}`.
pub struct Granted;

impl Modify for Granted {
    fn modify(&self, api: &mut utoipa::openapi::OpenApi) {
        api.paths.paths.iter_mut().for_each(|(path, item)| {
            [("GET", &mut item.get), ("POST", &mut item.post), ("PUT", &mut item.put), ("PATCH", &mut item.patch), ("DELETE", &mut item.delete)]
                .into_iter()
                .for_each(|(method, operation)| operation.iter_mut().for_each(|operation| {
                    grant(operation, method, path);
                    name(operation);
                }));
        });
    }
}

fn grant(operation: &mut Operation, method: &str, path: &str) {
    let grants: Vec<String> = request_grants(method, path, path).into_iter().map(|grant| grant.0).collect();
    operation.security = Some(["session", "bearer"].map(|scheme| SecurityRequirement::new(scheme, grants.clone())).to_vec());
}

fn name(operation: &mut Operation) {
    let tag = operation.tags.iter().flatten().next().cloned().unwrap_or_default();
    operation.operation_id = operation.operation_id.take().map(|handler| format!("{tag}_{handler}"));
}
