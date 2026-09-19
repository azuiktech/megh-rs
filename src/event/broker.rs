//! Realtime pub/sub broker with transient replay and client management.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tokio::sync::{mpsc, RwLock};

use super::history::{HistoryProvider, NoopHistoryProvider};
use super::model::Event;
use super::ring::RingBuffer;
use super::topic::Topic;

const DEFAULT_BUFFER_SIZE: usize = 64;
const DEFAULT_REPLAY_BUFFER_SIZE: usize = 1000;

pub type AuthorizerFn = Arc<dyn Fn(&str, &Topic) -> bool + Send + Sync>;

struct ClientState {
    topics: Vec<Topic>,
    tx: mpsc::Sender<Event>,
}

struct BrokerState {
    clients: HashMap<String, ClientState>,
}

/// Pub/sub event broker coordinating message broadcasting, subscriptions, and replay.
#[derive(Clone)]
pub struct Broker {
    state: Arc<RwLock<BrokerState>>,
    replay_buffer: Arc<RwLock<RingBuffer<Event>>>,
    history_provider: Arc<dyn HistoryProvider>,
    authorizer: AuthorizerFn,
    buffer_size: usize,
    seq: Arc<AtomicU64>,
}

impl Broker {
    /// Creates a new broker with default settings.
    pub fn new() -> Self {
        Self {
            state: Arc::new(RwLock::new(BrokerState {
                clients: HashMap::new(),
            })),
            replay_buffer: Arc::new(RwLock::new(RingBuffer::new(DEFAULT_REPLAY_BUFFER_SIZE))),
            history_provider: Arc::new(NoopHistoryProvider),
            authorizer: Arc::new(|_, _| true),
            buffer_size: DEFAULT_BUFFER_SIZE,
            seq: Arc::new(AtomicU64::new(0)),
        }
    }

    /// Attaches a custom history provider for persistent history fallback.
    pub fn with_history_provider(mut self, hp: impl HistoryProvider + 'static) -> Self {
        self.history_provider = Arc::new(hp);
        self
    }

    /// Attaches a custom subscription authorizer filter.
    pub fn with_authorizer<F>(mut self, auth: F) -> Self
    where
        F: Fn(&str, &Topic) -> bool + Send + Sync + 'static,
    {
        self.authorizer = Arc::new(auth);
        self
    }

    /// Sets the replay ring buffer capacity.
    pub fn with_replay_capacity(mut self, capacity: usize) -> Self {
        self.replay_buffer = Arc::new(RwLock::new(RingBuffer::new(capacity)));
        self
    }

    /// Registers a new subscriber client and returns their receiver channel.
    pub async fn register(&self, client_id: &str) -> mpsc::Receiver<Event> {
        let (tx, rx) = mpsc::channel(self.buffer_size);
        let mut state = self.state.write().await;
        state.clients.insert(
            client_id.to_string(),
            ClientState {
                topics: Vec::new(),
                tx,
            },
        );
        rx
    }

    /// Unregisters and disconnects a subscriber client.
    pub async fn unregister(&self, client_id: &str) {
        let mut state = self.state.write().await;
        state.clients.remove(client_id);
    }

    /// Subscribes a client to topic patterns.
    pub async fn subscribe(&self, client_id: &str, topics: &[Topic]) -> Result<(), &'static str> {
        let mut state = self.state.write().await;
        let client = state.clients.get_mut(client_id).ok_or("client not found")?;
        for topic in topics {
            if (self.authorizer)(client_id, topic) && !client.topics.contains(topic) {
                client.topics.push(topic.clone());
            }
        }
        Ok(())
    }

    /// Broadcasts an event to all clients with matching subscription patterns.
    pub async fn publish(&self, mut event: Event) {
        if event.id.is_empty() {
            let next_seq = self.seq.fetch_add(1, Ordering::Relaxed) + 1;
            event.id = next_seq.to_string();
        }

        self.replay_buffer.write().await.push(event.clone());

        let state = self.state.read().await;
        for client in state.clients.values() {
            if client.topics.iter().any(|p| event.topic.matches(p)) {
                let _ = client.tx.try_send(event.clone());
            }
        }
    }

    /// Replays events published strictly after `last_id` matching subscriber topics.
    /// Returns Some(events) if `last_id` was in the transient ring buffer, or None if missing.
    pub async fn replay_since(&self, last_id: &str, patterns: &[Topic]) -> Option<Vec<Event>> {
        let buffer = self.replay_buffer.read().await;
        buffer.replay_after(|e| e.id == last_id).map(|events| {
            events
                .into_iter()
                .filter(|e| patterns.is_empty() || patterns.iter().any(|p| e.topic.matches(p)))
                .collect()
        })
    }

    /// Accesses the underlying history provider.
    pub fn history_provider(&self) -> &Arc<dyn HistoryProvider> {
        &self.history_provider
    }
}

impl Default for Broker {
    fn default() -> Self {
        Self::new()
    }
}
