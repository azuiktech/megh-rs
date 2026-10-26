//! Issue #217 (#1 and #11 of the code review). A closed stream leaves the broker, and a reconnecting client catches up
//! on the history of every topic it follows.

use std::sync::Arc;
use std::time::Duration;

use axum::body::{Body, BodyDataStream};
use axum::http::{Request, StatusCode};
use axum::Router;
use chrono::{TimeZone, Utc};
use megh::{events_router, Broker, Event, HistoryFuture, HistoryOptions, HistoryProvider, Topic};
use serde_json::{json, Value};
use tokio_stream::StreamExt;
use tower::ServiceExt;

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
async fn a_closed_stream_leaves_the_broker() {
    let broker = Arc::new(Broker::new());
    let app = events_router(broker.clone());
    let mut stream = open(&app, "topic=news/**").await;
    let client = client_id(&received(&mut stream).await);

    drop(stream);
    broker.publish(Event::new("news/today", json!({}))).await;

    assert_eq!(subscribe(&app, &client, "news/**").await, StatusCode::BAD_REQUEST, "the closed client is gone");
}

/// History kept elsewhere: `a/one/1` under both `a/**` and `a/one/**`, and the earlier `b/1` under `b/**`.
struct Stored;

fn stored(id: &str, topic: &str, minute: u32) -> Event {
    Event { id: id.into(), topic: Topic::new(topic), data: json!({}), timestamp: Utc.with_ymd_and_hms(2026, 10, 26, 9, minute, 0).unwrap() }
}

impl HistoryProvider for Stored {
    fn fetch_history<'a>(&'a self, _topic: &'a Topic, _opts: &'a HistoryOptions) -> HistoryFuture<'a> {
        Box::pin(async { Ok(Vec::new()) })
    }

    fn fetch_history_since_id<'a>(&'a self, topic: &'a Topic, _last_id: &'a str, _limit: Option<usize>) -> HistoryFuture<'a> {
        let events = match &**topic {
            "a/**" | "a/one/**" => vec![stored("a1", "a/one/1", 2)],
            "b/**" => vec![stored("b1", "b/1", 1)],
            _ => Vec::new(),
        };
        Box::pin(async move { Ok(events) })
    }
}

#[tokio::test]
async fn a_reconnecting_client_catches_up_on_every_topic_once_in_time_order() {
    let app = events_router(Arc::new(Broker::new().with_history_provider(Stored)));

    let caught_up = received(&mut open(&app, "topic=a/**,a/one/**,b/**&since=gone").await).await;

    assert_eq!(caught_up.matches("id: a1").count(), 1, "a1 once: {caught_up}");
    let (b, a) = (caught_up.find("id: b1"), caught_up.find("id: a1"));
    assert!(b.is_some() && b < a, "b/1 came first: {caught_up}");
}
