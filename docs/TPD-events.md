# TPD — Realtime events

PRD row 6. Live push to browsers with catch-up on reconnect; the transport for messaging (PRD row 8).

## Scope

A broker that publishes events on topics and streams them to signed-in clients over SSE. Persistence of events belongs to the app, which supplies a history provider.

## Built

- `src/event/broker.rs`: `Broker` with `register`, `unregister`, `subscribe`, `publish`, `replay_since`, `with_history_provider`, `with_authorizer`, `with_replay_capacity`. `publish` drops clients whose channel is closed, then sends to the rest. In memory, per process, at-most-once.
- `src/event/topic.rs`: `/`-separated topics; `*` matches exactly one segment, `**` matches everything.
- `src/event/ring.rs`: replay buffer, default 1000 events.
- `src/event/history.rs`: `HistoryProvider` (`fetch_history`, `fetch_history_since_id`), default `NoopHistoryProvider`. Replay and history carry only topics the authorizer allowed, de-duplicated by event id and ordered by time.
- `src/event/sse.rs`: `GET /stream` (`topic`, `since`, `history_limit`, `Last-Event-ID`) and CSRF-protected `POST /sub`; a connection records who opened it and `POST /sub` finds only the caller's own.
- The event counter starts at the current time in microseconds.
- `ui/sdk/src/events.ts`: browser client.
- Tests: `tests/events_stream_test.rs`, `events_authorizer_test.rs`, `events_csrf_test.rs`, `event_ids_test.rs`.

## Decided, not built

- Fix the `ui/sdk` `Events` client: it ignores named SSE events (the server sends `event: <topic>`) and uses camelCase where the server uses `client_id`. Needed before the messaging UI.
- Durable (PRD feature 10): events are held in the database, not in memory. Delivery is practically always exactly once, with very high probability ("at most once makes no sense in practical cases"), with back pressure, and events are held while a connection is poor, for the front end and for back-end consumers.
- Messaging uses this broker only: typing is published without being held; a reconnecting client receives everything pending.

## Open

- How "practically exactly once" is achieved (for example repeat until acknowledged, with the consumer ignoring an id it has already seen); the mechanism is not decided.
- What back pressure does to a slow consumer: pause the sender, hold in the database until the consumer catches up, or refuse new events. Today a client that falls behind drops events silently (per-client buffer of 64).
- How a back-end consumer registers and acknowledges, since today a consumer is only an SSE client.
