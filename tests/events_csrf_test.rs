use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use megh::{events_router, events_router_with_csrf, Broker};
use tower::ServiceExt;
use tower_http::csrf::CsrfLayer;

fn sub_body() -> Body {
    Body::from(r#"{"client_id":"c1","subscribe":[],"unsubscribe":[]}"#)
}

async fn post_sub(app: &axum::Router, site: &str) -> StatusCode {
    let request = Request::post("/sub")
        .header("content-type", "application/json")
        .header("sec-fetch-site", site)
        .body(sub_body())
        .unwrap();
    app.clone().oneshot(request).await.unwrap().status()
}

#[tokio::test]
async fn cross_site_subscription_requests_are_rejected_by_default() {
    let app = events_router(Arc::new(Broker::new()));

    assert_eq!(post_sub(&app, "same-origin").await, StatusCode::OK);
    assert_eq!(post_sub(&app, "cross-site").await, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn a_trusted_origin_can_be_configured() {
    let csrf = CsrfLayer::new().add_trusted_origin("https://app.example.com").unwrap();
    let app = events_router_with_csrf(Arc::new(Broker::new()), csrf);

    let request = Request::post("/sub")
        .header("content-type", "application/json")
        .header("origin", "https://app.example.com")
        .body(sub_body())
        .unwrap();
    assert_eq!(app.clone().oneshot(request).await.unwrap().status(), StatusCode::OK);
    assert_eq!(post_sub(&app, "cross-site").await, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn streaming_is_unaffected_a_get_request_needs_no_csrf_check() {
    let app = events_router(Arc::new(Broker::new()));
    let request = Request::get("/stream").header("sec-fetch-site", "cross-site").body(Body::empty()).unwrap();

    assert_eq!(app.oneshot(request).await.unwrap().status(), StatusCode::OK);
}
