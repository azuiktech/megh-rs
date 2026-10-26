//! Issue #213 (#2 and #3 of the code review). Replayed events pass the authorizer like live ones (Mercure), the
//! authorizer knows who is asking from the caller's access token, and only the caller who opened a stream can add
//! topics to it.

use std::sync::Arc;
use std::time::Duration;

use axum::body::{Body, BodyDataStream};
use axum::http::{Request, StatusCode};
use axum::{Extension, Router};
use megh::auth::AccessToken;
use megh::{events_router, Broker, Event, Topic};
use serde_json::{json, Value};
use tokio_stream::StreamExt;
use tower::ServiceExt;
use uuid::Uuid;

fn token(sub: Uuid) -> AccessToken {
    AccessToken { sub, iss: "megh".into(), aud: vec![Uuid::now_v7().to_string()], exp: u64::MAX, iat: 0, client_id: "megh".into(), scope: String::new() }
}

/// Only `public/` topics are open, to anyone.
fn public_only() -> Arc<Broker> {
    Arc::new(Broker::new().with_authorizer(|_: Option<&AccessToken>, topic: &Topic| topic.starts_with("public/")))
}

/// Each caller may subscribe to its own `users/{sub}/**` only.
fn own_only() -> Arc<Broker> {
    Arc::new(Broker::new().with_authorizer(|caller: Option<&AccessToken>, topic: &Topic| {
        caller.is_some_and(|token| **topic == format!("users/{}/**", token.sub))
    }))
}

fn as_caller(broker: &Arc<Broker>, caller: Option<AccessToken>) -> Router {
    let app = events_router(broker.clone());
    match caller {
        Some(token) => app.layer(Extension(token)),
        None => app,
    }
}

async fn publish(broker: &Broker, id: &str, topic: &str) {
    let mut event = Event::new(topic, json!({}));
    event.id = id.into();
    broker.publish(event).await;
}

async fn open(app: &Router, query: &str) -> BodyDataStream {
    let request = Request::get(format!("/stream?{query}")).body(Body::empty()).unwrap();
    app.clone().oneshot(request).await.unwrap().into_body().into_data_stream()
}

/// What the stream sends until it is quiet for a moment.
async fn received(stream: &mut BodyDataStream) -> String {
    let mut text = String::new();
    while let Ok(Some(Ok(chunk))) = tokio::time::timeout(Duration::from_millis(200), stream.next()).await {
        text.push_str(&String::from_utf8_lossy(&chunk));
    }
    text
}

fn client_id(connect: &str) -> String {
    let data = connect.lines().find_map(|line| line.strip_prefix("data: ")).unwrap();
    serde_json::from_str::<Value>(data).unwrap()["client_id"].as_str().unwrap().to_string()
}

async fn subscribe(app: &Router, client: &str, topic: &str) -> StatusCode {
    let body = json!({"client_id": client, "subscribe": [topic], "unsubscribe": []});
    let request = Request::post("/sub")
        .header("content-type", "application/json")
        .header("sec-fetch-site", "same-origin")
        .body(Body::from(body.to_string()))
        .unwrap();
    app.clone().oneshot(request).await.unwrap().status()
}

#[tokio::test]
async fn replay_sends_only_the_topics_the_authorizer_allowed() {
    let broker = public_only();
    publish(&broker, "1", "public/start").await;
    publish(&broker, "2", "secret/plans").await;
    publish(&broker, "3", "public/news").await;
    let app = as_caller(&broker, None);

    let asked_for_both = received(&mut open(&app, "topic=secret/**,public/**&since=1").await).await;
    assert!(asked_for_both.contains("public/news") && !asked_for_both.contains("secret/plans"), "{asked_for_both}");

    let asked_for_none = received(&mut open(&app, "since=1").await).await;
    assert!(!asked_for_none.contains("secret/plans") && !asked_for_none.contains("public/news"), "no topic, nothing replayed: {asked_for_none}");
}

#[tokio::test]
async fn the_authorizer_is_told_who_is_asking() {
    let broker = own_only();
    let (ann, bob) = (Uuid::now_v7(), Uuid::now_v7());
    let mut stream = open(&as_caller(&broker, Some(token(ann))), &format!("topic=users/{ann}/**,users/{bob}/**")).await;
    received(&mut stream).await;

    publish(&broker, "", &format!("users/{ann}/inbox")).await;
    publish(&broker, "", &format!("users/{bob}/inbox")).await;

    let live = received(&mut stream).await;
    assert!(live.contains(&format!("users/{ann}/inbox")) && !live.contains(&format!("users/{bob}/inbox")), "{live}");
}

#[tokio::test]
async fn only_the_caller_who_opened_a_stream_adds_topics_to_it() {
    let broker = public_only();
    let ann_app = as_caller(&broker, Some(token(Uuid::now_v7())));
    let mut stream = open(&ann_app, "").await;
    let ann_client = client_id(&received(&mut stream).await);

    assert_eq!(subscribe(&as_caller(&broker, Some(token(Uuid::now_v7()))), &ann_client, "public/ads").await, StatusCode::BAD_REQUEST, "another member");
    assert_eq!(subscribe(&as_caller(&broker, None), &ann_client, "public/ads").await, StatusCode::BAD_REQUEST, "an anonymous caller");
    publish(&broker, "", "public/ads").await;
    assert!(!received(&mut stream).await.contains("public/ads"), "nothing was added to ann's stream");

    assert_eq!(subscribe(&ann_app, &ann_client, "public/ads").await, StatusCode::OK);
}
