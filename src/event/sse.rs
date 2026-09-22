//! Axum Server-Sent Events (SSE) streaming transport and subscription handlers.

use axum::extract::{Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::sse::{Event as SseEvent, KeepAlive, Sse};
use axum::routing::{get, post};
use axum::{Json, Router};
use futures_util::stream::Stream;
use serde::Deserialize;
use std::convert::Infallible;
use std::sync::Arc;
use std::time::Duration;
use tokio_stream::wrappers::ReceiverStream;
use tokio_stream::StreamExt;
use tower_http::csrf::CsrfLayer;
use uuid::Uuid;

use super::broker::Broker;
use super::model::{Event, HistoryOptions, SubscriptionRequest, SubscriptionResponse};
use super::topic::Topic;

#[derive(Debug, Deserialize)]
pub struct StreamQuery {
    pub topic: Option<String>,
    pub since: Option<String>,
    pub history_limit: Option<usize>,
}

/// Returns an Axum router mounted with SSE streaming and subscription routes. `POST /sub` is CSRF-protected by
/// default (no trusted cross-origin callers); use [`events_router_with_csrf`] to configure trusted origins.
pub fn events_router(broker: Arc<Broker>) -> Router {
    events_router_with_csrf(broker, CsrfLayer::new())
}

/// [`events_router`] with a caller-configured [`CsrfLayer`], for a web app served from a different origin than
/// this router (`csrf.add_trusted_origin("https://app.example.com")`).
pub fn events_router_with_csrf(broker: Arc<Broker>, csrf: CsrfLayer) -> Router {
    Router::new()
        .route("/stream", get(sse_handler))
        .route("/", get(sse_handler))
        .route("/sub", post(sub_handler).layer(csrf))
        .with_state(broker)
}

async fn sse_handler(
    State(broker): State<Arc<Broker>>,
    headers: HeaderMap,
    Query(query): Query<StreamQuery>,
) -> Sse<impl Stream<Item = Result<SseEvent, Infallible>>> {
    let client_id = Uuid::new_v4().to_string();
    let rx = broker.register(&client_id).await;

    let topics: Vec<Topic> = query
        .topic
        .map(|t| t.split(',').map(|s| Topic::new(s.trim())).collect())
        .unwrap_or_default();

    if !topics.is_empty() {
        let _ = broker.subscribe(&client_id, &topics).await;
    }

    let last_id = headers
        .get("last-event-id")
        .and_then(|h| h.to_str().ok())
        .map(|s| s.to_string())
        .or(query.since);

    let replayed = fetch_replayed(&broker, last_id.as_deref(), &topics, query.history_limit).await;

    let connect_msg = format!(r#"{{"status":"connected","client_id":"{client_id}"}}"#);
    let connect_event = SseEvent::default().event("connect").data(connect_msg);

    let replay_stream = futures_util::stream::iter(
        std::iter::once(connect_event).chain(replayed.into_iter().map(to_sse_event)),
    )
    .map(Ok);

    let live_stream = ReceiverStream::new(rx).map(|e| Ok(to_sse_event(e)));
    let combined = replay_stream.chain(live_stream);

    Sse::new(combined).keep_alive(KeepAlive::new().interval(Duration::from_secs(15)))
}

async fn fetch_replayed(
    broker: &Broker,
    last_id: Option<&str>,
    topics: &[Topic],
    limit: Option<usize>,
) -> Vec<Event> {
    if let Some(id) = last_id {
        if let Some(buffered) = broker.replay_since(id, topics).await {
            return buffered;
        }
        for topic in topics {
            if let Ok(history) = broker
                .history_provider()
                .fetch_history_since_id(topic, id, limit)
                .await
            {
                if !history.is_empty() {
                    return history;
                }
            }
        }
    } else if let Some(lim) = limit {
        let opts = HistoryOptions {
            since: None,
            limit: Some(lim),
        };
        for topic in topics {
            if let Ok(hist) = broker.history_provider().fetch_history(topic, &opts).await {
                if !hist.is_empty() {
                    return hist;
                }
            }
        }
    }
    Vec::new()
}

fn to_sse_event(e: Event) -> SseEvent {
    let payload = serde_json::to_string(&e.data).unwrap_or_default();
    SseEvent::default()
        .id(e.id)
        .event(e.topic.to_string())
        .data(payload)
}

async fn sub_handler(
    State(broker): State<Arc<Broker>>,
    Json(req): Json<SubscriptionRequest>,
) -> Result<Json<SubscriptionResponse>, (StatusCode, &'static str)> {
    if !req.subscribe.is_empty() {
        broker
            .subscribe(&req.client_id, &req.subscribe)
            .await
            .map_err(|e| (StatusCode::BAD_REQUEST, e))?;
    }
    Ok(Json(SubscriptionResponse {
        client_id: req.client_id,
        subscriptions: req.subscribe,
    }))
}
