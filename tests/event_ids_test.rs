//! Event ids keep rising when the process restarts, so a `Last-Event-ID` from before a restart never names a new event.

use megh::{Broker, Event};
use megh::event::topic::Topic;

async fn published_id(broker: &Broker) -> u64 {
    let mut events = broker.register("client").await;
    broker.subscribe("client", &[Topic::new("news")]).await.unwrap();
    broker.publish(Event::new("news", serde_json::json!({}))).await;
    events.recv().await.unwrap().id.parse().unwrap()
}

#[tokio::test]
async fn a_new_broker_continues_after_the_ids_of_the_last_one() {
    let before_restart = published_id(&Broker::new()).await;
    let after_restart = published_id(&Broker::new()).await;

    assert!(after_restart > before_restart, "{after_restart} after {before_restart}");
}
