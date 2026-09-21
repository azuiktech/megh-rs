use axum::body::Body;
use axum::http::{header, Request, Response, StatusCode};
use megh::auth::http::{auth_router, MeghAuthState};
use tower::ServiceExt;
use tower_sessions::{MemoryStore, SessionManagerLayer};

fn app() -> axum::Router {
    let state = MeghAuthState::new(sqlx::PgPool::connect_lazy("postgres://localhost/test").unwrap());
    auth_router(state).layer(SessionManagerLayer::new(MemoryStore::default()))
}

async fn logout(app: &axum::Router, cookie: &str, token: &str) -> Response<Body> {
    let request = Request::post("/auth/logout").header(header::COOKIE, cookie).header("x-csrf-token", token);
    app.clone().oneshot(request.body(Body::empty()).unwrap()).await.unwrap()
}

#[tokio::test]
async fn writes_need_the_token_issued_to_the_session() {
    let app = app();
    let issued = app.clone().oneshot(Request::get("/auth/csrf-token").body(Body::empty()).unwrap()).await.unwrap();
    let cookie = issued.headers()[header::SET_COOKIE].to_str().unwrap().split(';').next().unwrap().to_string();
    let token = String::from_utf8(axum::body::to_bytes(issued.into_body(), usize::MAX).await.unwrap().to_vec()).unwrap();

    assert_eq!(logout(&app, &cookie, "").await.status(), StatusCode::FORBIDDEN);
    assert_eq!(logout(&app, &cookie, "forged").await.status(), StatusCode::FORBIDDEN);
    assert_eq!(logout(&app, &cookie, &token).await.status(), StatusCode::OK);
}
