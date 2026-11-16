# TPD — Messaging

PRD feature 11. Messaging between a client of a tenant and a member of that tenant, for support, booking and the like. Not a user-to-user chat room. What is said is a2ui 1.0: the member (usually an AI agent) sends surfaces, the client answers with the renderer's action, the same JSON filled in. Rendering is the client's (Lit, Flutter and the like).

## Scope

Decided by the user:

- **Who talks:** a client (`guest`, `prospect`, `client`) and a member of the tenant (staff, a provider, or a machine such as an agent). The conversation is the client–member pair; no thread id is named.
- **Tenant:** a scope bound once, when messaging is constructed. One tenant's messages never reach another.
- **API:** `send(card, from, to)`; `send_event(signal, from, to)` for what is not a message (typing now, more such as waiting later); the backend listens with `on` and `on_event` the same way the browser does. No ids, surface ids or broker calls in user code.
- **a2ui:** minimal code, structs and serde, a2ui 1.0 (trika-rs is a reference, not a dependency). Plain text is an a2ui `Text`. A fluent builder with canned helpers; one button alone, rows, columns and several cards work.
- **Display, set once at construction, not per message:** the avatar is a function of the sender id returning an image, a vector or text (`variant: "avatar"`); a timestamp; a read mark that is just an icon (`check`, `doneAll`). The client maps icon names to its icon set; `svgPath` is the rare case.
- **Transport:** HTTP and SSE over the existing broker, very minimal glue, as kyrios-rs does it (store, then publish with the message id; a `HistoryProvider` reads the store). (a) a browser that is offline queues its sends; (b) the broker holds what is sent while the browser is away and delivers it on reconnect; (c) on reconnect the browser gets everything since the last it had, from the broker's queue or from the database. After a restart the browser fetches with `since`; the queue is not stored.
- **Forms from a schema:** little helpers layered on the builder. A typed value is the body of a card, with an optional title and subtitle and a row of buttons (approve/reject, or send). A filled value is shown as fields; an empty or prefilled form is inputs from a JSON schema, an entity's own (`ToSchema`) or any schema. A receipt or a payment is a filled form; a client intake is a form from the client's schema. Raw a2ui is a schema with optional data filled.
- **Labels:** a message may carry labels, plain strings shown with it.
- **Field order:** an entity's fields show in the type's order (`serde_json` and `utoipa` with `preserve_order`). The caller drops fields a form should not show (`Schema::without`).
- Unread counts are dropped. External connectors are future scope, not part of this TPD.
- One feature, one PR.

## Built

- `migrations/0009_messages.sql`: `messages (tenant_id, id, client_id, member_id, sender, body jsonb, read_at, created_at)`.
- `src/messaging/surface.rs`:
  - `Surface` (`new`, `text`, `title`, `subtitle`, `image`, `field`, `fields`, `rows`, `button`, `row`, `column`, `card`, `carousel`, `input`, `number`, `text_area` (a multi-line `TextField`, variant `longText`), `choice`, `calendar`, `inputs`, `filled`, `labels`, `messages`), canned `approval`, `selection`, `login`, `logout`, `times`, `date`, `range`, `intake`, `feedback(Scale)` (a rating, a comment `text_area` and Send).
  - `title` is a bold `Text` (a2ui 1.0 `Text` has only `body` and `caption`), `subtitle` a caption. `image(url, text)` is an `Image` with `text` as its description. `fields(&value)`: one `field` per property of a serializable value, in its order; a property with no value is left out. `rows(&[T])`: a header row of the property names, then one row of values per item, every cell of equal weight so the columns line up.
  - A label is the schema's `title`, else the property name with `_` as spaces and a capital first letter (`postal_code` is "Postal code"), in `inputs`, `fields` and `rows`.
  - `row`, `column`, `card` and `carousel` rename the components their closure returns under the nesting component's id (`c1_c0`), so any surface nests, canned ones included (`column(|_| Surface::intake())`).
  - `inputs(Schema)`: per property, in order: an enum is a `choice`, a `date`/`date-time` a `calendar`, a number a `number`, a boolean a `CheckBox`, an object a nested column of its own inputs (bound under `/{name}/…`), anything else an `input`; each is labelled, choices and dates too. `filled(&data)`: the data model the inputs show.
  - `Schema::of::<T: ToSchema>()` resolves `$ref`, merges `allOf` (a flattened struct's fields come first) and takes a nullable `oneOf` or `type` as its type; `Schema::from(Value)` does the same for any schema; `without(&[..])` drops properties. `intake()` is the `ClientData` schema without `tenant_id` and `attributes`.
  - `labels(&[..])`: a row of caption texts first in the message.
  - Every button's `event.context` binds each top-level key of the surface's inputs, so the action carries what was filled.
  - a2ui 1.0 as `@a2ui/web_core` 0.15.0 bundles it: `"version": "1.0"`, a binding is `{"@path": …}`, a function call `{"@call": …}`, and a component id is an identifier (a nested id is `c0_c1`).
  - The frame of avatar, timestamp and read mark wraps a sent surface; the mark is an `Icon` bound to `/meta/mark` in the data model, which holds the sent and read icons beside the filled data.
- `Cargo.toml`: `serde_json` and `utoipa` with `preserve_order`, so JSON keys and schema properties keep their declared order.
- `src/messaging/message.rs`:
  - `Messaging::new(pool, broker, tenant_id)`, `with_avatar(fn(Uuid) -> Avatar)`, `with_timestamp(TimeFormat)`, `with_read_mark(ReadMark)`.
  - `send(surface, from, to) -> Message`: stored, then published on `tenants/{t}/clients/{client}/members/{member}` with the message id as event id. A pair of two members or two clients is `MessagingError::NotAPair`.
  - `send_event(Signal, from, to)`: `TypingOn`, `TypingOff`, `Read(up_to)`; published, not stored. A read marks what the other side sent up to `up_to` (`read_at`, the stored mark turned to the read icon) and carries the a2ui `updateDataModel` messages that turn the mark.
  - `on(me, |Message|)`, `on_event(me, |Notice|)`: follow `me`'s topics on the broker (a member hears all its clients) and are not handed `me`'s own.
  - `History`: the broker's `HistoryProvider` for these topics, from `messages`; `*` for either side is any. A first look (`history_limit`) gives the latest `limit` messages, oldest first; a catch-up after an id gives the oldest after it.
  - `router()`: the browser's routes, signed in as a client (token `sub`): `POST /tenants/{tenant_id}/members/{member_id}/messages` `{body}` and `…/events` `{signal}`; takes the `Broker` from an `Extension`.
- `ui/sdk/src/messaging.ts`: `Chat({base, events, tenant, client, member})` with `on`, `on_event`, `send`, `send_event`, `close`. It follows the pair's topic with `history_limit=50`; the browser's `EventSource` reconnects with `Last-Event-ID`; a send that cannot reach the server is queued in memory and sent in order on `online`.
- Tests: `tests/messaging_surface_test.rs` (31), `tests/messaging_broker_test.rs` (11), `ui/sdk/test/messaging.test.ts` (5).
- Checked against `@a2ui/web_core` 0.15.0: every canned surface, a typed review card with labels, a `ClientData` form and a raw-schema form, each sent through `Messaging` on Postgres with avatar, timestamp and read mark, then a read. All 29 messages pass its bundled 1.0 JSON schemas and load in its v1.0 `MessageProcessor` with the v1 basic catalog. The read turns the mark to `doneAll`, and pressing the form's Send, resolved by web_core, gives an action whose context holds the filled values. A sample conversation of every canned surface, product cards with images, a nested intake and log in, and a receipt in rows, built by the builder and sent through Postgres, renders with `@a2ui/lit` 0.15.0 on web_core's v1.0 core in a browser; presses come back with the filled values. Not drawn by Flutter.
- Run live (scratch server, real Postgres, HTTP and SSE): the agent's card, the agent's typing, the client's answer, typing and read all reached the other side; the stored mark turned to `doneAll`; after a server restart a reconnect with the last id received the missed message from the database. Not run in a real browser.

## Decided, not built

- Attachments, reply-to and the attachment size cap (PRD 11.2).
- Guests: a `kind` field on `Client` (`guest`, `client`, `prospect`, or another name), a guest being a client with placeholder names, converted to a client at booking (11.8).
- The web UI (11.11).

## Open

- How a client signs in to get the token the routes read (`sub` = the client).
- `on` subscribes in a spawned task, so a message sent in the same instant as `on` is called may not be heard live (it is in history).
- The browser's offline queue is in memory: a page reload while offline loses it.
