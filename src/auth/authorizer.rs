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

/// One path segment of the request: whether the template makes it an id, and its value without any `:verb`.
struct Segment {
    is_param: bool,
    value: String,
}

fn segments(matched_template: &str, uri_path: &str) -> (Vec<Segment>, Option<String>) {
    let names = |path: &str| path.split('/').filter(|s| !s.is_empty()).map(String::from).collect::<Vec<_>>();
    let verb = names(uri_path).last().and_then(|last| last.split_once(':').map(|(_, verb)| verb.to_string()));
    let segments = names(matched_template)
        .iter()
        .zip(names(uri_path))
        .map(|(template, value)| Segment { is_param: template.starts_with('{'), value: value.split(':').next().unwrap_or_default().to_string() })
        .collect();
    (segments, verb)
}

/// The resources of a path with the id of each: a literal is a resource when a parameter follows it or it ends the path.
fn resources(segments: &[Segment]) -> Vec<(&str, Option<&str>)> {
    segments
        .iter()
        .enumerate()
        .filter(|(_, segment)| !segment.is_param)
        .filter_map(|(i, segment)| match segments.get(i + 1) {
            Some(next) if next.is_param => Some((segment.value.as_str(), Some(next.value.as_str()))),
            None => Some((segment.value.as_str(), None)),
            Some(_) => None,
        })
        .collect()
}

/// The grants a request needs, one per resource level of its path, outermost first.
///
/// The last level takes the `:verb` of the last segment, else the action of the HTTP method; every level above it needs `read`.
/// A path with no resource needs `root`.
pub fn request_grants(method: &str, matched_template: &str, uri_path: &str) -> Vec<Grant> {
    let (segments, verb) = segments(matched_template, uri_path);
    let leaf_action = verb.as_deref().unwrap_or_else(|| request_action(method));
    let levels = resources(&segments);
    let leaf = levels.len().saturating_sub(1);
    let grants: Vec<Grant> = levels
        .iter()
        .enumerate()
        .map(|(i, (resource, id))| Grant::from_parts(resource, if i == leaf { leaf_action } else { "read" }, *id))
        .collect();
    if grants.is_empty() { vec![Grant::from_parts("root", leaf_action, None)] } else { grants }
}

#[cfg(feature = "axum")]
pub use axum_middleware::authorizer;

#[cfg(feature = "axum")]
mod axum_middleware {
    use axum::extract::{MatchedPath, OriginalUri, Request};
    use axum::http::StatusCode;
    use axum::middleware::Next;
    use axum::response::Response;

    use crate::auth::grant::Grant;
    use crate::org::Member;
    use super::request_grants;

    /// Zero-declaration Axum middleware that inspects the authenticated Member in request extensions
    /// and requires every grant `request_grants` derives from the matched route and the request path.
    pub async fn authorizer(
        req: Request,
        next: Next,
    ) -> Result<Response, (StatusCode, &'static str)> {
        let member = req.extensions().get::<Member>().cloned();
        let grants = req.extensions().get::<Vec<Grant>>().cloned();

        let (parts, body) = req.into_parts();
        let matched = parts
            .extensions
            .get::<MatchedPath>()
            .cloned()
            .ok_or((StatusCode::NOT_FOUND, "route not found"))?;
        let uri_path = parts.extensions.get::<OriginalUri>().map_or_else(|| parts.uri.path().to_string(), |uri| uri.0.path().to_string());

        let required = request_grants(parts.method.as_str(), matched.as_str(), &uri_path);
        let req = Request::from_parts(parts, body);

        let permitted = if let Some(m) = member {
            required.iter().all(|grant| m.has_grant(grant))
        } else if let Some(g_list) = grants {
            required.iter().all(|grant| g_list.iter().any(|g| g.implies(grant)))
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
