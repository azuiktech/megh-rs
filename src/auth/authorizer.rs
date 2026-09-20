//! Generic route-based Grant authorizer and permission derivation using standard parsers.

use std::path::Path as FilePath;
use super::grant::Grant;

/// Maps HTTP method to a CRUD action verb ("create", "update", "delete", "read").
pub fn request_action(method: &str) -> &'static str {
    match method.to_ascii_uppercase().as_str() {
        "POST" => "create",
        "PUT" | "PATCH" => "update",
        "DELETE" => "delete",
        _ => "read",
    }
}

/// Derives the requested Grant using std::path::Path hierarchical component parsing.
pub fn request_grant(method: &str, matched_template: Option<&str>, uri_path: &str) -> Grant {
    let action = request_action(method);
    let template = matched_template.unwrap_or(uri_path);
    let path = FilePath::new(template);

    let is_parameterized = path
        .file_name()
        .and_then(|n| n.to_str())
        .map_or(false, |s| s.starts_with(':') || (s.starts_with('{') && s.ends_with('}')));

    if is_parameterized {
        let uri = FilePath::new(uri_path);
        let instance = uri.file_name().and_then(|n| n.to_str());
        let resource = path
            .parent()
            .and_then(FilePath::file_name)
            .and_then(|n| n.to_str())
            .unwrap_or("root");

        return Grant::from_parts(resource, action, instance);
    }

    let resource = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("root");

    Grant::from_parts(resource, action, None)
}

#[cfg(feature = "axum")]
pub use axum_middleware::authorizer;

#[cfg(feature = "axum")]
mod axum_middleware {
    use std::path::Path as FilePath;
    use axum::extract::{FromRequestParts, MatchedPath, RawPathParams, Request};
    use axum::http::StatusCode;
    use axum::middleware::Next;
    use axum::response::Response;

    use crate::auth::grant::Grant;
    use crate::org::OrgMember;
    use super::request_action;

    /// Zero-declaration Axum middleware that inspects the authenticated OrgMember in request extensions,
    /// derives the requested Grant automatically from Axum's RawPathParams and std::path::Path, and verifies permissions.
    pub async fn authorizer(
        req: Request,
        next: Next,
    ) -> Result<Response, (StatusCode, &'static str)> {
        let member = req.extensions().get::<OrgMember>().cloned();
        let grants = req.extensions().get::<Vec<Grant>>().cloned();

        let (mut parts, body) = req.into_parts();
        let matched = parts
            .extensions
            .get::<MatchedPath>()
            .cloned()
            .ok_or((StatusCode::NOT_FOUND, "route not found"))?;

        let params = RawPathParams::from_request_parts(&mut parts, &())
            .await
            .ok();

        let action = request_action(parts.method.as_str());
        let path = FilePath::new(matched.as_str());

        let (resource, instance) = match params.as_ref().and_then(|p| p.iter().last()) {
            Some((_key, val)) => {
                let res = path
                    .parent()
                    .and_then(FilePath::file_name)
                    .and_then(|n| n.to_str())
                    .unwrap_or("root");
                (res, Some(val))
            }
            None => {
                let res = path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("root");
                (res, None)
            }
        };

        let requested = Grant::from_parts(resource, action, instance);
        let req = Request::from_parts(parts, body);

        let permitted = if let Some(m) = member {
            m.has_grant(&requested)
        } else if let Some(g_list) = grants {
            g_list.iter().any(|g| g.implies(&requested))
        } else {
            return Err((StatusCode::UNAUTHORIZED, "not authenticated"));
        };

        if permitted {
            Ok(next.run(req).await)
        } else {
            Err((StatusCode::FORBIDDEN, "not permitted"))
        }
    }
}
