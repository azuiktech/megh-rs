# TPD — Realtime events

PRD row 10. Live push to browsers with catch-up on reconnect (durable storage and re-delivery are decided below, not built); the transport for messaging (PRD row 11).

## Scope

A broker that publishes events on topics and streams them to signed-in clients over SSE. Persistence of events belongs to the app, which supplies a history provider.

## Built

- `src/event/broker.rs`: `Broker` (cheap to clone) with `register`, `unregister`, `subscribe`, `publish`, `replay_since`, `opened_by`, `history_provider`, `with_history_provider`, `with_authorizer`, `with_replay_capacity`. In memory, per process, live delivery is at-most-once. Each client has a 64-event channel; `publish` uses `try_send`, so a full channel drops the event silently, and clients whose channel is closed are pruned. An empty event id is assigned from a counter that starts at the current time in microseconds. The authorizer type is `Fn(Option<&AccessToken>, &Topic) -> bool`, default allow-all, applied only in `subscribe`: a denied topic is silently not added. `opened_by` compares the caller's `sub`; two anonymous callers share ownership.
- `src/event/model.rs`: `Event { id, topic, data, timestamp }` with `Event::new` and `with_id`, `HistoryOptions`, `SubscriptionRequest`. `EventSource` is exported and unused.
- `src/event/topic.rs`: `/`-separated topics; `*` matches exactly one segment, `**` zero or more (`courses/**` is valid); the bare `**` matches everything.
- `src/event/ring.rs`: replay buffer, default 1000 events, minimum capacity 1.
- `src/event/history.rs`: `HistoryProvider` (`fetch_history`, `fetch_history_since_id`) returning a boxed future, default `NoopHistoryProvider`.
- `src/event/sse.rs`: `events_router` and `events_router_with_csrf(broker, CsrfLayer)`. `GET /stream` (alias `GET /`) takes `topic`, `since`, `history_limit` and the `Last-Event-ID` header, sends an SSE `connect` event `{"status":"connected","client_id":...}`, then frames with `id` = event id, `event` = topic, `data` = the event's data only; keep-alive every 15 s. Catch-up tries the ring first; history is consulted only when the last id is not in the ring, over the topics the authorizer allowed, de-duplicated by id and ordered by time (the ring replay is neither de-duplicated nor re-sorted). With `history_limit` alone, history is fetched without a start time. `POST /sub` (CSRF-checked) handles `subscribe` only: `unsubscribe` and `last_event_id` are accepted and ignored, the response echoes the requested topics (not the granted ones), and an empty `subscribe` skips the ownership check. It finds only a connection the caller opened.
- `ui/sdk/src/events.ts`: browser client, with the mismatches listed below.
- No example wires `events_router`; tests do (`events_router(Arc<Broker>)` with the identity injected by an `Extension`). Behind `jwt_session` it also sits behind the authorizer, which would require grants such as `stream:read` and `sub:create`.
- Tests: `tests/events_stream_test.rs`, `events_authorizer_test.rs`, `events_csrf_test.rs`, `event_ids_test.rs`. Not tested: the `Last-Event-ID` header, unit behaviour of `topic.rs` and `ring.rs` beyond their own tests.

## Decided, not built

- Fix the `ui/sdk` `Events` client; needed before the messaging UI. It listens only for `connect` and `message`, while the server sends `event: <topic>`; it reads `clientId` and sends `clientId`/`lastEventId` where the server uses `client_id`/`last_event_id`; it expects `{id, topic, data, timestamp}` in the data while the server sends only the event's data (topic and id are in the SSE fields); it sends `history_since` and repeated `topic` parameters the server does not read; its `unsubscribe` does nothing on the server.
- Durable (PRD feature 10): events are held in the database, not in memory. Delivery is practically always exactly once, with very high probability ("at most once makes no sense in practical cases"), with back pressure, and events are held while a connection is poor, for the front end and for back-end consumers.
- Messaging uses this broker only: typing is published without being held; a reconnecting client receives everything pending.

## Open

- How "practically exactly once" is achieved (for example repeat until acknowledged, with the consumer ignoring an id it has already seen); the mechanism is not decided.
- What back pressure does to a slow consumer: pause the sender, hold in the database until the consumer catches up, or refuse new events. Today a client that falls behind drops events silently (per-client buffer of 64).
- How a back-end consumer registers and acknowledges, since today a consumer is only an SSE client.
