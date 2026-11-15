# TPD — Messaging

PRD feature 11. In-app messaging between a user and the business's agent (usually an AI agent) for support, booking, purchase and the like. **Not a user-to-user chat room.** Free text and attachments, plus a2ui 1.0 structured surfaces the user fills in. Modelled on the Facebook Messenger, Twilio Conversations and WhatsApp connectors.

## Scope

- Threads, their participants, messages with attachments, labels on threads and on messages, typing and read indicators. A **thread** is the container; the conversation is what its participants say in it.
- a2ui 1.0 surfaces sent by the agent, answers sent by the user; ready-made surfaces and Messenger-style canned templates.
- Guests: a prospect may talk before being a client; the agent shows a login surface when it needs a member.
- A web UI package; host apps style it.
- Out of scope here: AI logic (an agent is a machine member calling the same interface), external connectors (Facebook, Twilio, WhatsApp — PRD 11.12, later), payments.

## Built

The backend (PRD 11.1–11.10); `ui/chat` (11.11) and the connectors (11.12) are not built.

- `migrations/0009_messaging.sql`: `clients.kind` (default `client`), `threads`, `participants`, `messages` (with `answer`), `attachments`, `labels`, `thread_labels`, `message_labels`.
- `src/booking/client.rs`: `ClientKind { Guest, Client, Prospect, Other(Name) }` as text, `ClientData.kind`.
- `src/auth/token.rs`: `JwtSession::guest(tenant_id, client_id, thread_id)`.
- `src/messaging/`: `Messaging` (`open`, `join`, `speaker`, `send`, `messages`, `seen`, `typing`, `unread`, `label`, `unlabel`, `attach`, `attachment`, plus `reply`, `respond`, `party`, `arrive`), `Party`, `Labelled`, `Content`, `Upload`, `MessagingError`, `History`, `authorizer()`, `router()`, `guest_router()`, `openapi()`; one file per entity (`thread`, `participant`, `message`, `receipt`, `attachment`, `label`, `guest`).
- `src/messaging/surface/`: `Surface::generic` (validated with `jsonschema` against the schemas vendored in `src/messaging/spec/`), the ready-made surfaces and templates, and typed answers.
- `examples/messaging.rs`: the wiring, runnable.
- Tests: `tests/messaging_*_test.rs` (thread, receipts, attachment, label, surface, guest, history) and `tests/booking_client_kind_test.rs`.
- Run live against a real server and database: a guest arrives, follows its own thread over SSE (message and typing events arrive), sends a message, uploads and downloads an attachment; another thread of the tenant, the thread list and a missing token are refused (403/403/401), and the guest's SSE on another thread receives nothing.

Differences from the approved text, to be approved: `receipt(&Invoice, &Payment)` shows five fixed lines (an invoice holds no lines); `selection` is single-choice; `login`/`logout` open `/auth/login` and `/auth/logout`; the feedback `.comment()` and `.attachment()` options are not built (`Rated` reads a `comment` and an `attachment` from the context); `Selected`, `Picked` and `Intake` have the shapes in `src/messaging/surface/answer.rs`; `:seen` and `:respond` are routed as `…/{id}/{verb}` as appointments are; guest names use the first four hex digits of a v7 id, which are the same for guests created within minutes (a defect, see Open).

Reused as they are:

- Entities and routes: `Entity<Uuid, T>`, `#[derive(Table)]`, `auth::crud`, `AppContext`, `Input`, `ListRequest`/`Page` (`src/entity.rs`, `src/auth/crud.rs`, `src/auth/rest.rs`).
- Authorization: route-derived grants (`src/auth/authorizer.rs`); `AccessToken` with `sub`, `aud` (tenant), `scope` (grants).
- Participants: `Member` (`src/tenant/member.rs`; a provider, staff, or a machine from `UserRepo::machine`) and booking `Client` (`src/booking/client.rs`).
- Realtime: `Broker`, `events_router`, `HistoryProvider` (`src/event/`, see [TPD-events](TPD-events.md)).
- Bytes in a row: `#[serde(with = "megh::bytea")]` (`src/bytea.rs`).
- What surfaces draw on: `Service`, `Product`, `Provider`, `Invoice`, `Payment`, the `slots:search` result (`src/booking`, `src/billing`).

## Decided, not built

All types and interfaces below are approved (2026-11-15); the Open questions at the end are not.

### Decisions already made

- a2ui **1.0** (candidate-stable; `version: "1.0"` in the released schemas vendored in `src/messaging/spec/`; messages `createSurface`, `updateComponents`, `updateDataModel`, `deleteSurface`; the user's action comes back as a renderer-to-agent `action` with its `context`). Basic catalog components available: Text, Image, Icon, Row, Column, List, Card, Tabs, Modal, Divider, Button, TextField, CheckBox, ChoicePicker, Slider, DateTimeInput.
- A **thread** holds a conversation between **participants**. A participant is a **member** (a provider, staff, or a machine such as an AI agent) or a **client**.
- A client has a `kind`: the defined names are `guest`, `client` and `prospect`, and others are allowed. A guest (someone who wants to book) is a `Client` row created at first contact with placeholder names; during booking the same row is updated with the real details and its kind changes, which is the conversion. There is no separate guest entity.
- Labels can be put on a thread and on a message.
- Guests may chat; the agent surfaces the login form when it needs a member.
- Attachments are `bytea` rows through sqlx with a configurable size cap.
- Delivery rides the existing broker (durable events are decided in TPD-events); a reconnecting client receives everything pending.
- A user-to-agent flow, not a chat room.

### Sample usage (caller first)

Agent side, in the app (an agent is a machine member):

```rust
let messaging = Messaging::new(pool, broker).with_attachment_limit(5 * 1024 * 1024);

let thread = messaging.open(tenant_id, Party::Client(guest_id), Some(name("Haircut question"))).await?;
let agent = messaging.join(tenant_id, thread.id, Party::Member(agent_member_id)).await?;
messaging.send(tenant_id, thread.id, agent.id, Content::text("Which service?")).await?;

let services = Surface::selection(Prompt::new("Pick a service"), choices_of(&services));
messaging.send(tenant_id, thread.id, agent.id, Content::surface(services)).await?;

// the user's reply arrives as a message of kind Answer
let picked: Selected = answer(&reply)?;        // Vec<(Uuid, Option<Decimal>)>
messaging.label(tenant_id, Labelled::Message(reply.id), label_id).await?;   // or Labelled::Thread(thread.id)
```

Browser side (`@megh/chat`):

```ts
const chat = await Chat.open({ tenantId, threadId, events });   // EventSource + history catch-up
chat.send({ text: "Hi", attachments: [file] });
chat.typing();
chat.markSeen(lastMessageId);
chat.answer(surfaceId, { event: "submit", context });          // a2ui action → POST :respond
```

An agent over HTTP uses the same API as any client, with its machine token: `POST /tenants/{t}/threads/{id}/messages`.

### Data (migration `0009_messaging.sql`; every table `tenant_id`-first, `deleted_at`, v7 ids)

| Table | Columns |
|---|---|
| `threads` | `tenant_id, id, subject, status` (`open`/`closed`), `appointment_id?`, `invoice_id?`, `last_message_at` |
| `participants` | `tenant_id, id, thread_id, member_id?, client_id?` (exactly one is set), `last_read_message_id?` |
| `messages` | `tenant_id, id, thread_id, sender_id (participant), kind` (`text`/`surface`/`answer`), `text?, surface? (a2ui v1.0 messages, jsonb), surface_id?, in_reply_to?` |
| `attachments` | `tenant_id, id, thread_id, message_id?, filename, content_type, size, content (bytea)` |
| `labels` | `tenant_id, id, name, colour?` |
| `thread_labels` | `tenant_id, thread_id, label_id` |
| `message_labels` | `tenant_id, message_id, label_id` |

A participant's kind follows from which id is set; a member's role (provider, staff, machine) comes from the member. Unread count of a participant = messages after `last_read_message_id` not sent by them. Typing is never stored.

### Interfaces (Rust, `megh::messaging`; every method returns `Result<_, MessagingError>`)

```rust
pub enum Party { Member(Uuid), Client(Uuid) }
pub enum Labelled { Thread(Uuid), Message(Uuid) }

pub struct Messaging { /* PgPool, Broker (clone), attachment limit */ }
impl Messaging {
    pub fn new(pool: PgPool, broker: Broker) -> Self;
    pub fn with_attachment_limit(self, bytes: usize) -> Self;

    pub async fn open(&self, tenant_id: Uuid, opener: Party, subject: Option<Name>) -> Result<Thread, MessagingError>;
    pub async fn join(&self, tenant_id: Uuid, thread_id: Uuid, party: Party) -> Result<Participant, MessagingError>;
    pub async fn send(&self, tenant_id: Uuid, thread_id: Uuid, sender: Uuid, content: Content) -> Result<Message, MessagingError>;
    pub async fn messages(&self, tenant_id: Uuid, thread_id: Uuid, page: PageRequest) -> Result<Page<Message>, MessagingError>;
    pub async fn seen(&self, tenant_id: Uuid, thread_id: Uuid, participant: Uuid, up_to: Uuid) -> Result<u32, MessagingError>; // unread left
    pub async fn typing(&self, tenant_id: Uuid, thread_id: Uuid, participant: Uuid) -> Result<(), MessagingError>;       // published, not stored
    pub async fn unread(&self, tenant_id: Uuid, participant: Uuid) -> Result<Vec<Unread>, MessagingError>;
    pub async fn label(&self, tenant_id: Uuid, target: Labelled, label_id: Uuid) -> Result<(), MessagingError>;
    pub async fn unlabel(&self, tenant_id: Uuid, target: Labelled, label_id: Uuid) -> Result<(), MessagingError>;
    pub async fn attach(&self, tenant_id: Uuid, thread_id: Uuid, sender: Uuid, upload: Upload) -> Result<Attachment, MessagingError>; // over the limit → TooLarge
    pub async fn attachment(&self, tenant_id: Uuid, id: Uuid) -> Result<Attachment, MessagingError>;
}

pub enum Content { Text { text: String, attachments: Vec<Uuid> }, Surface(Surface), Answer { surface_id: String, action: Action } }
pub struct History { /* HistoryProvider over messages, for reconnect catch-up */ }
pub enum MessagingError { NotFound, TooLarge { limit: usize }, InvalidSurface(String), Database(#[from] sqlx::Error) } // thiserror, non_exhaustive
```

`Thread` and `Label` CRUD beyond this use the generic `auth::crud` routes like other entities.

### Surfaces (`megh::messaging::surface`; builders return a validated `Surface { id, messages: Vec<Value> }`, a2ui 1.0)

```rust
Surface::generic(messages: Vec<Value>) -> Result<Surface, MessagingError>   // any a2ui 1.0, validated against agent_to_renderer.json

selection(prompt, choices: Vec<Choice>)            // pick products/services/providers
quantities(prompt, choices: Vec<Choice>)           // same, each with a count
login(prompt) / logout(prompt)                     // member sign-in; actions call /auth/*
open_times(prompt, slots: Vec<Slot>)               // from slots:search, per service
calendar_date(prompt) / calendar_range(prompt)     // date or date range
intake(prompt)                                     // fields of Client
feedback_rating(prompt, scale: Scale) / feedback_choice(prompt, labels: Vec<Name>)  // + .comment(), .attachment()
approval(prompt, fields: Vec<(Name, String)>)      // approve/reject with field name: value
quick_replies(Vec<Reply>)  buttons(text, Vec<Button>)  carousel(Vec<Card>)  receipt(&Invoice, &Payment)
```

Messenger limits enforced: quick replies ≤ 13 (title ≤ 20), buttons ≤ 3, carousel ≤ 10 cards (title ≤ 80, subtitle ≤ 80, ≤ 3 buttons each), receipt ≤ 100 lines. Typed answers: `answer::<Selected | Picked | Dates | Intake | Rated | Decision>(&Message) -> Result<T, MessagingError>`.

A calendar and a login form are built from basic-catalog components (`DateTimeInput`, `TextField`, `Button`); no custom catalog. The intake answer fills a `Client`; this is where a guest becomes a client (see Open).

### HTTP (grants derived from the route by the existing authorizer)

| Route | Grants |
|---|---|
| `POST/GET /tenants/{t}/threads`, `GET /threads/{id}` | `threads:create`, `threads:read` |
| `POST/GET /tenants/{t}/threads/{id}/participants` | `participants:create`, `participants:read` |
| `POST/GET /tenants/{t}/threads/{id}/messages` | `messages:create`, `messages:read` |
| `POST /tenants/{t}/threads/{id}/messages/{mid}:respond` | `messages:respond` (a2ui action) |
| `PUT/DELETE /tenants/{t}/threads/{id}/labels/{label_id}` and `…/messages/{mid}/labels/{label_id}`; `/labels` CRUD | `labels:*` |
| `POST /tenants/{t}/threads/{id}:seen` | `threads:seen` |
| `POST /tenants/{t}/threads/{id}/typing` | `typing:create` |
| `POST/GET /tenants/{t}/threads/{id}/attachments` (raw body), `GET …/attachments/{aid}` | `attachments:create`, `attachments:read` |
| `POST /tenants/{t}/guests` (public) | none; creates a guest client and mints its token (see Open) |

Events: topic `tenants/{t}/threads/{id}`, payloads `message`, `typing`, `seen`, `labels`. Thread-list changes for staff and agents: `tenants/{t}/threads`. Reconnect catch-up: `History` over `messages` (as kyrios-rs `ChatHistoryProvider`).

### Web UI (`ui/chat`, Svelte 5)

Thread list with unread badges, thread view, composer with attachments, typing and read indicators, thread and message labels, an a2ui 1.0 surface renderer (`@a2ui/lit`, bumped to 1.0), semantic HTML, styling only through CSS custom properties (`--chat-*`). Port of kyrios-rs `CourseChat.svelte` and `OnboardingSurface.svelte`. Needs the `ui/sdk` `Events` fix first (TPD-events).

### Delivery plan (one feature per PR, each through the full sequence)

1. `fix/events-sdk`. 2. Core: migration, `Messaging` open/join/send/messages, routes, events, `History`. 3. Attachments. 4. Seen and typing. 5. Labels. 6. a2ui: validation, `:respond`, answers, ready-made surfaces, templates. 7. Guests. 8. `ui/chat`. Sizes may shrink further at each interface gate.

### Wiring and wire contracts (added when the tests were written; additive to the approved interfaces)

- `Messaging` is `Clone`; `messaging::router() -> OpenApiRouter<AppContext>` takes it from an axum `Extension<Messaging>`, like billing takes `Formats`. Default attachment limit 10 MiB (`with_attachment_limit` overrides; over it is `413`).
- `messaging::authorizer() -> impl Fn(Option<&AccessToken>, &Topic) -> bool` for `Broker::with_authorizer`: `tenants/{t}/threads/{id}` is allowed when the token's grants imply `tenants:read:{t}` and `threads:read:{id}`; `tenants/{t}/threads` when they imply `tenants:read:{t}` and `threads:read`; nothing else, and no token allows nothing. `History::new(pool)` is the `HistoryProvider`, set with `Broker::with_history_provider`.
- `JwtSession::guest(tenant_id, client_id, thread_id) -> Result<IssuedToken, _>`: `sub` = the client id, `aud` = the tenant, `scope` = `tenants:read:{t} threads:read:{thread} threads:seen:{thread} messages:create messages:read messages:respond participants:read attachments:create attachments:read typing:create`.
- `POST /tenants/{t}/guests` (`messaging::guest_router()`, mounted outside the authorizer, takes `Extension<Messaging>` and `Extension<JwtSession>`) answers `{client, thread, participant, token, expires_in}`; the client has `kind = guest`, `first_name` `Guest`, `last_name` the last four hex digits of its id, `name` `Guest <those four>`.
- `Messaging::speaker(tenant_id, thread_id, subject) -> Participant`: the participant of `subject` (a token's `sub`) in the thread; a tenant member who is not one yet joins; anything else is `NotFound`. The message routes send as the speaker.
- `POST …/messages` body `{"text"?, "attachments"?: [uuid], "surface"?: [a2ui 1.0 messages], "in_reply_to"?: uuid}`: with `surface` the message is kind `surface`, otherwise `text`. A message JSON is the entity plus `attachments: [{id, filename, content_type, size}]`. `:respond` body is the a2ui action `{"name", "context", "userMessage"?}`; it stores a kind `answer` message (`surface_id` and `in_reply_to` of the surface message, `answer` = the action, `text` = `userMessage`). `messages` gets an `answer jsonb` column.
- Attachments: `POST …/attachments?filename=` with the raw body and its `Content-Type`, answers the metadata; `GET …/attachments/{aid}` answers the bytes with that content type.
- Events: topic `tenants/{t}/threads/{id}`, event id = the message id for a message; `data` is `{"kind":"message","message":{…}}`, `{"kind":"typing","participant_id"}`, `{"kind":"seen","participant_id","up_to"}` or `{"kind":"labels","target":{"thread"|"message":id},"labels":[ids]}`. A new thread is published on `tenants/{t}/threads` as `{"kind":"thread","thread":{…}}`.
- Answer contexts (the `context` of the `submit` action of each surface): `selection`/`quantities` `{"selected":[{"id","count"?}]}`; `open_times` `{"slot":{"starts_at","ends_at","service_id"}}`; `calendar_date` `{"from"}`, `calendar_range` `{"from","to"}` (dates `YYYY-MM-DD`); `intake` the flat fields of a `Client`; `feedback_rating` `{"rating":n,"comment"?,"attachment"?}`, `feedback_choice` `{"choice":"<label>","comment"?,"attachment"?}`; `approval` `{"decision":"approved"|"rejected"}`; `quick_replies` `{"reply":"<payload>"}`; `buttons` `{"button":"<payload>"}`.
- a2ui validation: the v1.0 `agent_to_renderer.json`, `common_types.json` and basic `catalog.json` are vendored in `src/messaging/spec/` (as trika-rs vendors them in `src/tool/a2ui/spec/`) and checked with the `jsonschema` crate.

## Open


- Decided since approval: the client `kind` (`ClientKind`, placeholder names `Guest` + short id), the guest token with instance grants, event authorization from the token's grants (no broker change), a2ui validation with `jsonschema` and vendored schemas, the attachment cap as an app-level builder (default 10 MiB). Not decided: how a guest differs from a prospect (no behavioural difference in the first version).

- **Messaging window** (Messenger: 24 hours; 7 days for a human agent) for external channels, later.
- Surface id uniqueness: a2ui requires a globally unique surface id for the renderer's lifetime; proposed one v7 id per surface.
