# TPD — Booking: the requirements as given

Only what the user (Abir) said, from 2026-10-18 to 2026-10-20, in their words where possible (quotes) and
grouped by topic, then what is built (section 13) and how it is authorized (section 14). Where the user answered
"looks ok" or "yes" to a proposal, the proposal itself is not repeated here, because it was not the user's
requirement.

This document uses **tenant**, as the code does: `tenant_id` and `/api/v1/tenants/{tenant_id}/...`. Words inside
quotation marks are the user's own and keep the older `org` they used. A hierarchy of tenants or locations is
not built: see section 4.4 and "Recorded, not built".

Marks: `✅` shipped · `[~]` partly shipped, with what is missing named · `[ ]` not started (as in `PRD.md`).

---

## 1. Purpose and scope

- "a generic booking system which can work across other domains also." Dental (NexHealth) is one reference,
  not the target.
- Lives in **megh-rs** ("all to be done in megh-rs. Trika is an unrelated agents project").
- Entities to model: "appointment, provider, resource, schedule, client, and few like org/location etc just
  flexible system."

## 2. References to follow (binding)

- NexHealth: https://docs.nexhealth.com/ (and https://docs.nexhealth.com/reference/introduction)
- Mindbody Public API v6:
  https://developers.mindbodyonline.com/ui/documentation/public-api#/http/mindbody-public-api-v6-0/introduction/getting-started
  (e.g. `staff-session-type`, `get-schedule-items`)
- cal.diy, for what is stored instead of computed: https://github.com/calcom/cal.diy
- Google AIP: https://google.aip.dev/ (filters: https://google.aip.dev/160)
- Row-level security with RBAC for org scoping: https://github.com/gabrielroddev-art/authorization-with-abac
- "fetch each and every page from the link provided", "read everything. do not come with anything which is
  not in this doc", "do not build things on your stupid brain. check references that I give", "do what is
  suggested from each api documentation."

## 3. Scenarios the model must cover

- a) Hair colour: the provider colours, moves to another client while it dries, comes back to finish.
- b) Dentist + hygienist on one patient: the hygienist sets up, the dentist does one or more procedures, the
  hygienist cleans.
- c) Gym: a client checks in, works out alone, checks out, until capacity is full (no provider).
- d) One-time TED talk of capacity n: book up to n, possibly with a waitlist.
- e) Restaurant: n customers book a table of n, optionally assigned a waiter with no engagement time defined
  (comes for orders on demand).
- f) Recurring music class every Thursday and Sunday 6:00–7:00 pm, capacity n; the user comes at that time —
  either a fixed (pre-booked) schedule, or a learner buys a 30-visit pass and can come at those times on any
  day during the validity of the pass.
- g) Pet grooming appointment.
- h) Couples massage: one room, two people book simultaneously, one or more providers.
- i) Float pod: a person books a pod (a room/resource), likely no provider.
- "There may be some more scenarios, but these should give the essence."

## 4. Entities

### 4.1 Service

- A service "can have a set of steps" (procedure codes in dental). Each step has a duration and participants
  (client, room, staff); "each step can require a staff or not."
- Buffer time (prep, cleanup: room engaged, client not) is part of a step: "buffer_before and buffer_after =>
  can be part of the step ... Likely no separate thing needed."
- In an appointment the client, the room/resource and the providers each may be free or engaged during the
  total time.
- Duration and price may vary by provider/staff or by client (master stylist; new-patient emergency).
  "adjustments are usually full service. e.g. original one + some delta" (Mindbody `staff-session-type`).
- Add-ons "are nothing but a set of service"; one books one or more services back to back (haircut + colour)
  or in a group (two people in parallel). Add-ons fit a service hierarchy like a menu/catalog (Mindbody
  category / sub-category / program).
- Ready-made templates "instead of elaborate setup", e.g. a 60 min one-step one-provider service (most
  common); a 50 min service with 10 min room cleaning; a 60 min service with 10 min prep and 20 min finish
  (hair colour: in the 30 min between, the provider can attend another client).

### 4.2 Providers and rooms/resources

- "resource & provider are similar as far as appointments are concerned, but differs on how to communicate
  to client, information to be stored etc. otherwise both are constraints for appointment."
- A room has a capacity and a waitlist: "a room has a capacity and waitlist two integers."
- Capacity defaults to 1, also for providers; waitlist defaults to 0.
- Double booking is capacity: "capacity 2 means double booking can be done at the same time in same room."
- Rooms/resources can have a hierarchy (lobby 1, room 1, ...).
- Pick a spot / table / seat "is nothing but specify a room/resource."

### 4.3 Client

- "add first name, last name, email, home_phone, work_phone, mobile_phone", plus attributes for assorted
  fields; a `name` is fine.
- Clients can be living or non-living (e.g. a pet); a group booking has a payee / primary person / insurance
  holder.

### 4.4 Tenant, and the org / location hierarchy

- Built: every entity has a mandatory `tenant_id`, part of its key, and a tenant is mandatory on every route.
  Tenant scope is a `WHERE tenant_id = ...` clause. ✅
- Requirement, **not built** `[ ]` (no hierarchy of tenants or locations exists in the code): "location is just
  another org. org can be hierarchy in any possible way. not necessarily a tree." A brand with 100 locations,
  and a franchise holding 14 of them: each location is an org; brand and franchise are loose groupings.
- "org_id is NEVER option for ANY entity." (now `tenant_id`) Org/location is mandatory on appointment queries.
- Org scope, **not built** `[ ]`: "for org it is again just a where clause. each org has its parents which are
  authorized. it is simple a rls with rbac" (see the RLS reference in section 2).

### 4.5 Tags, grouping and attributes

- Every entity can carry assorted metadata.
- Participants are grouped by tags, "not a rigid grouping" (salon: hair dresser, master stylist; dental:
  hygienist, dentist). "Tags/metadata is key value pair."
- Loose grouping applies to all entities (provider, service, etc.), like brand and franchise for orgs.
- Attributes are a JSON value: "pub type Attributes = serde_json::Value; is sufficient" — not a map.
- "no need any hardcoded online flag. it is all based on tags, can be filtered out."

### 4.6 Schedule and Blockout

- "Schedule is what are times open for work. Blockouts/Blocked Schedules are blocks." Names: **Schedule**
  and **Blockout**; a Blockout "usually can have an reason/notes associated."
- "schedule is always a open time, and an exception, like lunch time - recurring exception, holiday list
  (specific days are blocked). all these can be at org level, location, room, service, provider etc.
  schedule & exceptions are function of these appointment participation axis."
- Eligibility "is nothing but schedule defined" (NexHealth working hours): a service can have working hours
  for provider p and room r on recurring days; a schedule can have all axes or only some (a gym's hours open
  every room) or be very specific (tooth cleaning for Dr Albert on Wed/Thu 3–6 pm).
- Inheritance: "inward to outward - if room has hours defined then take, else take from location else from
  brand move outwards based on where it is defined." With several parents, "take the nearest from longest
  path" (L → Brand and L → Franchise → Brand: franchise wins); "Just keep a note."
- "room defines open hours, holiday is blocked hour (exception). so it subtracts open - blocked." A blockout
  on the room is taken (instrument breakdown), else traverse up (clinic closure).
- "a closed holiday not to be compared with closed lunch break."
- Business may change the staff of a schedule: configuration, noted in metadata.
- Recurrence: "use rrule with an appropriate library"; "for RRule also, do NOT have a string. have the
  RRuleObject which need to be saved in rrule format rfc."
- When asked about the schedule body: the Schedule entity as is.

### 4.7 Appointment

- "an appointment is one client, one service, start & end time, one room, zero or more provider. appointment
  time is the start and end time for the client." The service decides how long room or staff are needed
  beyond the client's time.
- Fields: "appointment { org_id, service_id, vec<provider_id>, starts_at, ends_at, room_id, status, and a
  created_at, updated_at etc fields, some of them already part of entity, payment, attributes}".
- "an appointment has one payment." Use proper types; no `Option` where the field is not optional.
- Statuses: "waitlist, booked, canceled, confirmed." Transitions: "an appointment waitlist can be booked or
  cancelled. an booked appointment can be canceled/confirmed. a confirmed appointment can be canceled. only
  those are possible."
- Anything else (late cancellation, attendance) is metadata: "They are not directly related to booking."
- No automatic waitlist promotion: "explicit api can update an waitlist appointment to booked. same like
  cancel etc other status update."

### 4.8 Slot

- "a slot can book one or more appointment for one or more client (group appointment like a couples
  massage, or two back to back appointment for a single client)." "A slot books N appointments." "4 people
  at a table is 4 appointments."

### 4.9 Reservation

Called "hold" until 2026-10-21, when the user renamed hold/held/held_id to reserve/reservation/reservation_id.

- "hold is not a booking state. a hold is a semi formed booking, it may not have client, or payment etc. it
  just a temporary block on the schedule." Like bus or airline booking: while held, it is not offered to
  another client.
- "a held slot is not an appointment. it just a slot temporarily removed from available slots."
- If a hold is implemented as an appointment, "then it need to be auto removed ... likely then an
  expires_at or similar thing needed so query always filter those out and a periodic job can run to clean
  up those entries."
- "do not use held for booking. use two different endpoints. not every booking use a held. some directly
  proactively tries to book and can fail at realtime."

### 4.10 Payment

- "booking has payment, who is paying/primary for booking. and how paying - pass/voucher/coupon/insurance/
  cash/card etc. essentially boils down to some kind of pre-paid option which debits, or just on spot
  payment. again a generic type with name etc, and eligible for that appointment or not."
- The payer is part of the payment ("appointment for my kid, insurance is a group insurance for me").
- Eligibility: "just have a predicate before booking."
- "for pass the debit is only when booked. not when waitlist."

## 5. Rules of availability and capacity

- "Availability = Schedule - Blockout - Appointment", computed dynamically.
- "any conflict in constraints will be rejected (schedule is not available, staff not available, room not
  available). in case client calendar is also visible, then client not available."
- Client calendar: "if client id is available, booked slots will be excluded. this is just the booking the
  client is holding in the system." If c1 with p1, p2 and r1 is booked on the 19th 9–10, another slots call
  for c1 must not offer 9–10.
- "a missing/not specified axis means any from the eligible set" (tooth extraction on Monday afternoon with
  no provider named: any provider who can do it then, by schedule).
- Capacity: "available capacity = total room capacity - booked + confirmed appointments. available waitlist
  = total waitlist - appointments in waitlist." Booked/confirmed reduce capacity; cancel increases it;
  waitlist reduces waitlist capacity; a waitlisted appointment that is cancelled or booked frees a waitlist
  place. "waitlist do not expire ... if a waitlist cancels it goes to waitlist free count, and NOT capacity
  free count."
- Constraints are predicates: "this is just a predicate defined (may be a little scripting language to make
  it dynamic later which can be stored)."

## 6. Availability search

- POST, "as get can't do a good payload for this request." If nothing is open, it can return
  `next_available_day`.
- Several services and parties: C1 => s1, s2; C2 => s1, s3. Parties are served in parallel or with an offset
  (both at 9; or 9–10 and 9:30–10:30); back-to-back services likewise (s1 9–9:30 then s2 9:30–10, or s2 at
  10–10:30). "two control parameter, default to 0 (i.e only give snap in time - exact same time for both
  cases)."
- Search by any tags, nothing hardcoded: a specific provider (Dr Albert), any female provider, only master
  stylists, a doctor with 10+ years of experience, a sea-facing room, a table with a kid's high chair, a
  disabled-friendly seat. Tags are an expression: Google AIP-160.
- "search to return slots that will be waitlisted. can be filtered just like anything else. default
  including all slots including waitlist."

## 7. Time and durations

- "all operations are based on local time in the org timezone. storage can be whatever db supports. both
  local time, time with timezone possible. with utc need a converter. offset is not much useful due to day
  light savings."
- "all duration to be stored ISO 8601 format. ALWAYS. check crate."

## 8. Performance

- Store what is needed instead of recomputing: "to get the open slots, it should NOT take all recurrences,
  all appointments etc. rather have those counters have up/down while booking" (cal.diy). "for now use
  indexed range."
- An occupancy must include the client too, so that a client already booked is not offered the same time.
- Lists: "for pagination, filter directly translate to limit, offset and where clause." "pagination is a
  simple implementation, and sqlx/axum has enough library to support pagination with 5 lines of code."
- Tenant scope is a where clause (section 4.4).

## 9. REST API

- Google AIP throughout, "including the path based permission ... do not invent own convention. actions will
  be with :action, resources will be /resources or /resources/{id} etc."
- Versioned paths, built as `/api/v1/tenants/{tenant_id}/...` (the user wrote `orgs/{org_id}`).
- Schedules CRUD, Blockouts, availability (open slots), appointments.
- Appointments: search by id, between times, for an org and its hierarchy, filtered by status (cancelled,
  held, waitlist, confirmed, booked), staff/provider, service, room, client; org/location mandatory. Built:
  list with an AIP-160 filter on any field, scoped to the tenant; there is no `held` status (holds are
  reservations, section 4.9) and no hierarchy.
- Book: a POST request; open slots: a POST (like NexHealth `appointment_slots`).
- Entity-direct: "it should work on entity directly. for request like post take body of the entity (like
  ServiceBody which is without id), and request like put etc take whole entity like Service. should return
  entity like Vec<Bookable>, or better a Json<Paginated<Bookable>> which gives current page cursor, and the
  data both." "entities will be simply annotated and extracted from request." A view, if needed, is separate;
  "for now return the entity."
- "first have single entity implement all crud endpoints with put/patch being the body of the entity" —
  then "This need to be done for all entities."
- Cancel is DELETE; PATCH can move an appointment; confirm is `:confirm`. "Use one POST route on
  /appointments/{id} that reads the :verb, with a single typed input; payment is the only extra field."
- No update mask ("looks you confuse what is grpc vs rest implementation").
- Handler shape: "ALL route should have a single arg input and single output." "http request is single
  input, NOT the state. state whenever needed can still come State(state): State<AppContext>." The state is
  named for what it is: "the context/config for the app having db, redis etc external dependency" (not
  `BookingState`).
- Later, replacing the `Json<...>` above: no `WithRejection`/`Reply` wrappers. Output format is not the route's concern: "it is middleware to do
  that. response should be simple Result<Paginated<BookableRow>, ApiError> or Result<BookableRow,
  ApiError>" — "simple have an ApiResult<BookableRow, ApiError> or ApiResult<Paginated<BookableRow>,
  ApiError>."
- Errors: "It can always be converted from one error to another error."
- The shared REST infrastructure goes in `auth` "so other packages can use the same infra" — "only the
  generic types, not the booking etc entities."
- Enums the extractor cannot read from objects: "just use a tag type."
- "no signature of handler should change without my permission."
- OpenAPI: the whole API mounted in a server, with an OpenAPI UI including request, response and
  authentication, checkable from the browser (like https://docs.nexhealth.com/reference/getinstitutions).

## 10. Libraries

- "use existing library for all cases, instead of ai generated code, including for aip, rrule or any other
  case where library is available." "for every case, come up with set of libraries which can implement the
  feature." Read their examples and follow them.
- Named by the user: `validator` ("use validate crate"); `axum-params`, `oasgen` (to check, with an example
  first); `aip-160` (not the abandoned `aip-filtering`); paginator-rs / sqlx / sqlx-paginated for paging;
  `rrule`; an ISO 8601 duration crate.
- Nothing out of scope (no diesel, protobuf).

## 11. Process

- First a TPD "in detail each and every resolution that is taken, so that NOTHING IS LOST."
- Issue first; types and interfaces for approval before tests or implementation; tests written from the TPD
  and frozen; schema shown before seeded tests.
- "each feature, make separate pr and merge. do not make a giant blob which can not be reviewed."
- Follow everything in CLAUDE.md and AGENTS.md.
- Work live in megh-rs ("ALWAYS do live editing, so human can receive feedback"); no scratch copies; no
  worktrees.
- "for any case where clarification needed, ask. do NOT lie." Ask only about real issues; always give
  context.
- Do not add what was not asked ("do not complicate things which was not asked").

## 12. Resolutions, 2026-10-20 (second round)

Given in answer to a list of questions; the user's words where quoted.

- **Process.** "do not jump to conclusion directly. have the spec defined first." Clarifications are asked
  first; "for every design decision (entity/public api, trait) get approval first." "Absolute
  non-negotiable is a correct implementation." "Take a small bite at a time."
- **Scope.** "full thing in scope - one pr at a time. goal is to have full workable implementation of a
  booking system supporting all common profiles listed" (section 3).
- **Entities.** "all cases need to implement via Entity." Every entity has a generic `Attributes` with a
  JSON value for assorted metadata. "There is no separate tags. those are attributes. a json object which
  is a key value."
- **Values.** "no unnecessary nullable." "a value can NOT be ill-constructed. not everything is a
  object/string."
- **Grouping.** "grouping/hierarchy is same for all cases. any set of grouping can be made with a
  tag/name+ description (like lobby 1), and as usual an attributes."
- **Org hierarchy** (**not built** `[ ]`). Loosely coupled, not a fixed tree; an org may be part of another or isolated. "no
  cycles. this need to be stored to optimize queries. e.g. given a parent location/org get all
  appointments across all sub locations." Follow standard practice from real projects ("do not innovate
  solution"), e.g. https://github.com/gabrielroddev-art/authorization-with-abac.
- **Shared person and contact fields.** "for all cases use same fields/reuse/share home/work/mobile/email
  etc, address { zip, state, country, ...}, and some human specific things where entity is a person/living
  like gender, age, first name last name." Address and the like are not entities: a type used by many
  entities, possibly flattened into the table.
- **Provider and resource.** They "can share same common trait and use the same for open slot
  computation, but they are entirely different entity as far as db tables or types are concerned."
  Resource: id, name, description, capacity, waitlist. Provider: the same plus human fields (first_name,
  last_name, name, work/home/mobile phone, email, description).
- **Service steps.** "each step may need more than one constraints like need a hyginist + dentist + client
  + room. a room cleaning step after appointment is done is just room, rest including client is free.
  appointment starts_at, ends_at is just clients time. it doesn't mean room is free etc." Per-provider
  delta on the steps: later, "when rest done."
- **Schedule.** As NexHealth working hours: "a schedule can be for a service, for a set of providers and
  rooms. those are OR conditions. if for service S1, P1,P2, R1, R2 are on a schedule 9-5, then P1,R1,
  P1,R2, P2,R1, P2,R2 all combinations are possible for that S1." A Blockout is a schedule as a negative
  list with a reason (holiday, staff day off). All schedules recur by rrule plus a time of day (e.g. every
  Mon and Tue of alternate weeks, 9:00–17:00).
- **Appointment.** `{ id, client_id, vec[provider], service, room, starts_at, ends_at, note, payment,
  status }`; `note` is a typed field. Also "cancelled_at, booked_at, confirmed_at etc which are not part of
  lifecycle entity timestamp." Statuses the user first listed: held or blocked, waitlist, booked, confirmed, done "or similar".
  Superseded: the code has `waitlist`, `booked`, `confirmed`, `completed`, `cancelled` (serialized in snake_case,
  spelled `Cancelled` in Rust) and no `held` status; a hold is a row of `reservations`, not an appointment
  (section 4.9).
- **Payment.** As in section 4.10: "a minimal small trait/type."
- **CRUD.** Follows megh's path-based RBAC, e.g. `/api/v1/tenants/{tenant_id}/providers/{provider_id}`. "optimized
  means the structure in db should be optimized for query => the minimum number of query /join can do the
  job. representation should be done based on algorithm (query)."
- **PATCH.** JSON merge (RFC 7396): "update the fields specified, default the fields explicitly set to
  null." To be reminded later: a field set to null that has no default (e.g. a required name).
- **Libraries.** "for all cases use libraries. when needed mention the library to be used."

Third round, same day:

- **Attributes.** `serde_json::Map<String, Value>` ("minor thing").
- **Statuses.** Also "booked-> completed, confirmed-> completed". Held "is not really appointment. so this
  can share the type, or held appointments are stored in a separate place. check real booking systems like
  LibreBooking, easyappointments etc, how they do. ALWAYS check real software written by human and use that
  as reference."
- **Person.** Store the date of birth, not the age.
- **Org hierarchy** (**not built** `[ ]`). "look at real implementations and propose solution. it can be a postgres column array
  to store all child, or all parent. come up with possible options based on checking in internet for real
  projects. requirement is to move up the hierarchy to get appropriate data, e.g. a location defines
  holiday list, take that, else go through its parents."
- **Schema changes.** "you can add modify columns as required to improve. always mention & get approval
  when implementing that => transparency is key."

Fourth round, same day:

- **Reservation** (then called hold). Approved: a separate `reservations` table with `expires_at`, as cal.diy's `SelectedSlots` (`releaseAt`);
  expired reservations are filtered by the query and removed by a periodic job. Appointment transitions: waitlist →
  booked or cancelled; booked → confirmed, cancelled or completed; confirmed → cancelled or completed.
- **Org hierarchy** (**not built** `[ ]`). Deferred: "Not satisfied with any of them. It can be done in a limited scope far simple
  way." The user will come back to it. Until then every entity carries a mandatory `tenant_id` (then `org_id`).
- "based on what already decided. start execution plans." (section 13).

---

## 13. Delivery

One PR per row, in order. Each PR: GitHub issue, then its types and interfaces for approval, then tests from
this TPD (frozen), then the implementation, then a live check against a running server. Every decision not
yet approved is listed in the row and asked before that PR starts.

| # | Feature | Pending decisions | Status |
|---|---|---|---|
| B1 | Shared value types: `Attributes`, names, `Email`, `Phone`, `Address`, `Contact`, `Person` (no table). Types approved 2026-10-20 as proposed (`Attributes = Map<String, Value>`; `Name` via `nutype`; `Address { line_1, line_2?, city, region, postal_code, country: celes::Country }`; `Contact { email?, home_phone?, work_phone?, mobile_phone? }`; `Person { first_name, last_name, gender, birth_date }`; flattened columns). Built as plain types: `Name` and `Phone` (E.164) are `nutype` newtypes validated by annotation; `celes::Country` and `email_address::EmailAddress` are used directly; `Gender` is `male`/`female`/`other` (NexHealth's set). Reading these from table columns is decided with the first table (B3) | — | ✅ PR #142 |
| B2 | Shared REST layer in `megh::auth`: `AppContext`, `ApiError`/`ApiResult`, `Input<T>` (axum-params merges path, query and body, read as JSON; path values always win), `ListRequest { tenant_id, filter: Option<aip_160::Filter>, page }`, `PageRequest`/`Page<T>` with an opaque paginator-utils cursor, `push_filter` (AIP-160 to a SQL WHERE condition; values written into the SQL text; non-column fields compare in `attributes` as jsonb of the same JSON type). Decided after several rounds: no paging crate (sqlx-paginated needs sqlx 0.9), no sea-query (adds nothing over `QueryBuilder` for this), no ORM | — | ✅ PR #144 |
| B3 | Resource: `ResourceData { tenant_id, name, description, capacity (>=1), waitlist (>=0), attributes }`, table `resources` keyed by `(tenant_id, id)`, CRUD under `/api/v1/tenants/{tenant_id}/resources` (UUID v7 ids; list ordered by id with cursor paging; PATCH is a JSON merge read and written in one transaction); `megh::booking::{openapi, router}`; `examples/booking.rs` with Scalar docs at `/docs` (cookie session plus CSRF header). Changes to shared code: `TableEntity::update` takes any executor (pool or transaction); saving an `Entity` also writes `updated_at`; `Entity` and `Page` answer a request as JSON | — | ✅ PR #146 |
| B4 | Provider: `ProviderData { tenant_id, first_name, last_name, name, description, contact, capacity, waitlist, attributes }`, table `providers` keyed by `(tenant_id, id)` with `email`, `home_phone`, `work_phone`, `mobile_phone` as nullable columns; routes like Resource. `Email` and `Phone` read from text columns through nutype's `derive_unchecked(sqlx::Type)` with `#[sqlx(transparent)]`; `Contact.email` is `Email` (checked by `email_address`). `#[derive(Table)]` now understands `#[sqlx(flatten)]` (the nested type's columns joined with `constcat`). The six SQL bodies moved to `megh::auth::crud`, used by Resource and Provider. `Capacity`, `Waitlist` moved to `value`. The trait shared with Resource for slot search comes with B10 | — | ✅ PR #148 |
| B5 | Client: `ClientData { tenant_id, first_name, last_name, name, contact, gender, birth_date: Option, address: Option<Address>, attributes }`, table `clients` keyed by `(tenant_id, id)`; `gender` text with a CHECK, `male | female | other | unknown`, default `unknown` (`Gender::Unknown` added, 2026-10-20); `address` one nullable jsonb column (a flattened `Option<Address>` would silently drop a partial address; decided 2026-10-20), complete when given; routes like Provider. `#[derive(Table)]` accepts `#[sqlx(json(nullable))]` | — | ✅ PR #150 |
| B6 | Service: `ServiceData { tenant_id, name: Name, description, setup: iso8601::Duration, cleanup: iso8601::Duration, steps: Vec<Step>, attributes }`; setup and cleanup take only the room, each `Step { duration, provider1: Option<String>, provider2: Option<String> }` takes the client and the provider groups it names (dental: hygienist; hygienist + dentist; hygienist. Hair colour: stylist; none; stylist). Decided 2026-10-20: "setup duration 0 is no setup", no `Option` where a default exists; no wrapper types: the contract is checked on construction by `validator` 0.21 annotations (name 1–255 chars, at least one step, provider names not empty), in `Input` and again on the patched result; durations ISO 8601 through `iso8601` 0.6, written in its canonical form (zero is `P0D`). Table `services` (setup, cleanup, steps jsonb) | how a group name matches a provider (for B10) | ✅ PR #152 |
| B7 | Schedule and Blockout, like NexHealth working hours: `ScheduleData { tenant_id, name: Name, provider_id: Option<Uuid>, resource_id: Option<Uuid>, service_ids: Vec<Uuid>, recurrence: rrule::RRuleSet, starts_at: NaiveTime, ends_at: NaiveTime, attributes }`; `BlockoutData` adds `reason`. No provider, no resource or no services means any. Decided 2026-10-20: DTSTART is the rule start and UNTIL/COUNT the rule end; `starts_at`/`ends_at` are the window of each day, so it is known without expanding the rule; a window ends the same day (overnight is a known limitation, checked by `validator`); the time in DTSTART is not used; the recurrence is a JSONB string (rrule 0.14 writes the RFC text, adding BYHOUR/BYMINUTE/BYSECOND); provider and resource are foreign keys, `service_ids` a `UUID[]`. Service `name` back to `Name` | — | ✅ PR #155 |
| B8 | Appointment: `AppointmentData { tenant_id, client_id, service_id, resource_id, provider_ids: Vec<Uuid>, starts_at, ends_at, status: Status, note, payment: Payment, products: Vec<Used>, booked_at, attributes }`, `Payment { payer_id, method: Name, attributes }` in one jsonb column, `Status { waitlist, booked, confirmed, completed, cancelled }`. Decided 2026-10-21: `booked_at` is set by the server at insert and never empty; no other status timestamps (status and `updated_at` say it); status and `booked_at` sent on PATCH are ignored. Routes: list, get, `POST /appointments` (see B9, B10), PATCH, DELETE cancels (grant `appointments:delete`; there is no `:cancel` verb), `POST /appointments/{id}:book`, `:confirm`, `:complete` (AIP-136; optional `payment`), each one guarded UPDATE (`src/booking/appointment.rs`): waitlist → booked or cancelled; booked → confirmed, cancelled or completed; confirmed → cancelled or completed. `megh::booking::rewrite_aip_136` routes `{id}:verb` as `{id}/:verb` and keeps the original URI for the authorizer. | — | ✅ PR #157; create, move and products added in B9, B10 |
| B9 | Availability search and booking. Search: `POST /tenants/{tenant_id}/slots:search` (grant `slots:search`) with `from`, `to`, `parties` (each `{client: {id} or {tag}, service_id, providers: [{field: value}], resources: [{field: value}]}`), `service_gap` (longest gap between one client's appointments) and `client_gap` (longest gap between different clients' start times), both default 0, and `page_size` / `page_token`; returns a page `{items, next_page_token}` of slots, each the parties filled in with `providers`, `resources`, `starts_at`, `ends_at`, `waitlist`, `capacity` left. A page searches at most 7 days. Decided 2026-10-21: a slot is not an appointment until it is booked; providers and resources get a `role` field and a step's provider group matches it; `providers`/`resources` match fields only, not attributes yet (a field that is not a column is a 400; **later: match attributes through a GIN index on `attributes`**); taken time is stored in `reservations` (B10; there is no `occupancies` table). Start times are each schedule window's start plus setup, one slot per start time (as NexHealth returns a time once), one room time apart (setup, client time and cleanup), so that consecutive slots can share a room; gaps are tried on a 5-minute grid; a free place is taken before a waitlist place, and a waitlisted party needs a waitlist place on its room and every provider. A collection custom method is one literal route segment (`slots:search`), not rewritten. Booking: `POST /tenants/{tenant_id}/appointments` takes the appointment itself with an optional `reservation_id` (`status` booked or waitlist, otherwise 400; no waitlist flag, no parties); in one transaction under a per-tenant advisory lock it checks that the products are on hand, runs the search's check for exactly that client, service, room, providers and time, answers 409 when the time is not open for that status, and writes the appointment with `booked_at` and its reservations. `:book` (waitlist → booked) runs the same check under the same lock, leaving out the appointment's own time (issue #161). PATCH moves an appointment (issue #163): `starts_at`, `ends_at`, `resource_id` or `provider_ids` (MOVES) move a waitlisted, booked or confirmed appointment only when the booking check passes for its status at the new time, room and providers (leaving out its own time), under the same lock; its reservations are written again; a cancelled or completed appointment does not move (400). PATCH field rules: CHANGEABLE (`tenant_id`, `id`, `note`, `payment`, `products`, `attributes`) change in place; MOVES as above; SERVER_SET (`status`, `booked_at`) are ignored; any other field is a 400 (so `client_id` and `service_id` cannot change). Not built: `next_available_day` when nothing is open (only the next page token). | — | ✅ PRs #159, #160, #162, #164 |
| B10 | Reservations (decided 2026-10-21): a reservation is not an appointment (no client, no payment). One table `reservations (tenant_id, id, owner_id, participant_id, during tstzrange, waitlist, expires_at)` (`migrations/0004_booking.sql`), one row per place taken from a room, provider or client; `owner_id` is the appointment or the hold, so one id points to all its blocks; GiST on `(tenant_id, participant_id, during)` (btree_gist). Capacity left is capacity minus the most rows overlapping at once (room of capacity 2: one booking leaves 1, two leave 0 and it is no longer offered, a cancel gives one back); `waitlist` rows count against the waitlist the same way. Booking writes rows, cancel deletes them, `:book` sets `waitlist = false`, a move rewrites them; a hold's rows count until `expires_at` (default 10 minutes). Search reads, all scoped by `tenant_id`: the services asked for, providers by role (all hygienists), rooms with the parties' fields (`to_jsonb(r) @> wanted`), schedules and blockouts that apply to those ids and services, and reservations of those ids in the range; open time per room-with-crew and provider-in-room is merged once and checked by binary search; each slot party returns `capacity` left. Recurrence (measured 2026-10-21): expanding from the rule's start costs 13–363 µs per rule per search, so rules are expanded as stored (COUNT, EXDATE and INTERVAL work as is); a floating reference date only if that grows. Done in #165 (the search reads `reservations`, not a separate occupancy table). Holds (#167): `POST /tenants/{tenant_id}/reservations` takes a slot as the search returns it, checks it like a booking under the same lock and holds its places for 10 minutes (`{id, expires_at}`); `DELETE /tenants/{tenant_id}/reservations/{id}` releases; `POST /tenants/{tenant_id}/appointments` with `reservation_id` next to the appointment books from the hold, taking one held place per place it needs (409 if the hold does not hold them or expired), so a hold of several parties is booked one appointment at a time; `POST /tenants/{tenant_id}/reservations:purge` (grant `reservations:purge`) with `before` deletes expired holds and places that ended before it; a `before` later than now is a 400 and nothing is deleted | — | ✅ PRs #166, #168 |
| B11 | Server example mounting the whole API with a browsable OpenAPI (request, response, authentication) | done with B3: `examples/booking.rs`, unchanged as entities are added | ✅ PR #146 |
| B12 | Products, prices, stocks (`migrations/0006_products.sql`; sold or used with a service): `products` CRUD (SKU unique per owner tenant, unit, price), the `prices` ledger (read only: a row each time a product or service is created or its price changes), `stocks` (read only: how much of a product a tenant holds) and `GET /tenants/{tenant_id}/products/{id}:stock`; an appointment's `products` must be on hand when it is booked or they change (409), nothing is held. Services gain `unit`, `price`, `currency`. | — | ✅ |
| — | Org / location hierarchy (DAG, inheritance upward): not built, no code or table | deferred by the user; open until the user returns to it | [ ] |


Reminders to raise later: a PATCH that sets a field with no default (e.g. `name`) to null (section 12); the
tie between two unrelated parents at the same depth (org hierarchy, not built).

## 14. Authorization and routes

Every route needs a grant derived from its path (`src/auth/authorizer.rs`, `request_grants`; `src/auth/grant.rs`).

- Each resource level of the path needs a grant. Every level above the leaf needs `read`; the leaf takes the
  `:verb` of the last segment, else the action of the HTTP method (POST create, PUT and PATCH update, DELETE
  delete, otherwise read).
- The grants are `tenants:read:{tenant_id}`, then `<resource>:<action>` or `<resource>:<action>:{id}`; e.g. the
  cancel of an appointment needs `tenants:read:{tenant_id}`, `appointments:delete:{id}`.
- The route's `{tenant_id}` must be the caller's tenant (the member's, or the token's audience).
- Custom grants: `appointments:book` (and `:confirm`, `:complete`), `slots:search`, `reservations:purge`,
  `products:stock`.
- `megh::booking::rewrite_aip_136` routes an item's `{id}:verb` as `{id}/:verb` and keeps the original URI for the
  authorizer; a collection method (`slots:search`, `reservations:purge`) is one literal segment and is not rewritten.

Routes, all under `/api/v1/tenants/{tenant_id}` (`src/booking/mod.rs`, `src/booking/openapi.md`):

| Resource | Routes |
|---|---|
| resources, providers, clients, services, schedules, blockouts | list, create, get, PUT, PATCH, DELETE (soft delete) |
| products | list, create, get, PUT, PATCH, DELETE; `GET /products/{id}:stock` |
| prices | `GET /prices`, `GET /prices/{id}` (read only) |
| stocks | `GET /stocks`, `GET /stocks/{id}` (read only) |
| appointments | list, `POST /appointments`, get, PATCH, DELETE (cancel); `POST /appointments/{id}:book`, `:confirm`, `:complete` |
| slots | `POST /slots:search` |
| reservations | `POST /reservations` (hold), `DELETE /reservations/{id}` (release), `POST /reservations:purge` |

## Recorded, not built

Requirements above that the code does not implement; each is `[ ]` not started.

- [ ] Payment eligibility by pass, voucher, coupon or insurance, and the debit of a pass when booked (section 4.10):
  `Payment` is `{payer_id, method, attributes}` with no predicate before booking and no balance.
- [ ] Recurring class with a 30-visit pass (scenario f): no pass or visit count exists.
- [ ] Org / location hierarchy and scope by parents (sections 4.4, 12): only the flat `tenant_id` exists.
- [ ] Waitlist auto-promotion: none, by decision (section 4.7); a waitlisted appointment is booked only by `:book`.
- [ ] `next_available_day` in the availability search (section 6).
- [ ] Constraints as stored predicates or a scripting language (section 5).

## Decided (from the clarifications)

- Availability is `POST /tenants/{tenant_id}/slots:search`, returning a page `{items, next_page_token}`. The body takes `page_size` (default 50, max 1000) and `page_token`; a page searches at most 7 days and never past `to`; the cursor is on `starts_at`. The grant is `slots:search`. A standard List is GET with no body, so a search with a body is a custom verb, not a list.
- Every list API pages (AIP-158 `page_size` / `page_token`).
- Shared REST layer: `crud` `get`, `create` and `replace` run one query and take `PgExecutor`; `patch` and `delete` open a transaction and take `Acquire`, and `TableEntity::soft_delete` takes `Acquire`.
- Filters: `is_array` marks a `Vec<T>` column (native array on PostgreSQL, JSON text on SQLite); the has-filter SQL exists for both databases and is tested on in-memory SQLite.
- Every id is a UUID v7: `now_v7` in code, `uuidv7()` in SQL (PostgreSQL 18 or later).
- A reservations purge whose `before` is later than now is a 400 and nothing is deleted (`src/booking/reservation.rs`).
- Decided, not built (for messaging, see [TPD-messaging](TPD-messaging.md)): a `Client` has a `kind` field, with defined names `guest`, `client` and `prospect` and other names allowed. A guest is created at first contact with placeholder names and becomes a client when booking fills in the details.
