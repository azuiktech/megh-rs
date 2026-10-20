# TPD — Booking: the requirements as given

Only what the user (Abir) said, from 2026-10-18 to 2026-10-20, in their words where possible (quotes) and
grouped by topic. No design or implementation from the earlier attempt is carried over; that work was deleted
on 2026-10-20. Where the user answered "looks ok" or "yes" to a proposal, the proposal itself is not repeated
here, because it was not the user's requirement.

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

### 4.4 Org

- "location is just another org. org can be hierarchy in any possible way. not necessarily a tree." A brand
  with 100 locations, and a franchise holding 14 of them: each location is an org; brand and franchise are
  loose groupings.
- "org_id is NEVER option for ANY entity." Org/location is mandatory on appointment queries.
- Org scope: "for org it is again just a where clause. each org has its parents which are authorized. it is
  simple a rls with rbac" (see the RLS reference in section 2).

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

### 4.9 Hold

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
- Org scope is a where clause (section 4.4).

## 9. REST API

- Google AIP throughout, "including the path based permission ... do not invent own convention. actions will
  be with :action, resources will be /resources or /resources/{id} etc."
- Versioned paths: `/api/v1/orgs/{org_id}/...` (plural `orgs`).
- Schedules CRUD, Blockouts, availability (open slots), appointments.
- Appointments: search by id, between times, for an org and its hierarchy, filtered by status (cancelled,
  held, waitlist, confirmed, booked), staff/provider, service, room, client; org/location mandatory.
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
- **Org hierarchy.** Loosely coupled, not a fixed tree; an org may be part of another or isolated. "no
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
  lifecycle entity timestamp." Statuses: held or blocked, waitlist, booked, confirmed, done "or similar".
  A held slot "is technically not an appointment paid. but implementation can use same type, or separate
  type for the same."
- **Payment.** As in section 4.10: "a minimal small trait/type."
- **CRUD.** Follows megh's path-based RBAC, e.g. `/v1/orgs/{org_id}/providers/{provider_id}`. "optimized
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
- **Org hierarchy.** "look at real implementations and propose solution. it can be a postgres column array
  to store all child, or all parent. come up with possible options based on checking in internet for real
  projects. requirement is to move up the hierarchy to get appropriate data, e.g. a location defines
  holiday list, take that, else go through its parents."
- **Schema changes.** "you can add modify columns as required to improve. always mention & get approval
  when implementing that => transparency is key."
- **Lessons.** Sections "Why the first attempt was discarded" and "Lessons learned" are to be given
  particular importance.

Fourth round, same day:

- **Hold.** Approved: a separate `holds` table with `expires_at`, as cal.diy's `SelectedSlots` (`releaseAt`);
  expired holds are filtered by the query and removed by a periodic job. Appointment transitions: waitlist →
  booked or cancelled; booked → confirmed, cancelled or completed; confirmed → cancelled or completed.
- **Org hierarchy.** Deferred: "Not satisfied with any of them. It can be done in a limited scope far simple
  way." The user will come back to it. Until then every entity still carries a mandatory `org_id`.
- "based on what already decided. start execution plans." (section 14).

---

## 12. Why the first attempt was discarded

The first attempt (megh-rs PRs #80–#126 and #129–#139, issues #79–#140) was deleted on 2026-10-20: `main`
was reset to `f76a838` and every booking branch and issue removed. These are the decisions Claude made that
caused it, in the order they happened.

### 12.1 Not following the references or the user's model

1. **Started in the wrong repository.** The design and first code went into trika-rs, with git worktrees
   nobody asked for, before being moved to megh-rs.
2. **Read the references shallowly.** NexHealth and Mindbody were skimmed, then the model was invented:
   `market.rs`, `BookedLine`, `party`, `seats`, `Standing`, `Window`, `Occurrence`, `Enrollment`, a separate
   `Hold` entity and a separate `waitlist_entries` entity (a "waitlist for a service"), none of which the
   user asked for or the references have.
3. **Added entities and modules without approval.** `market.rs` and others were introduced without being
   proposed.
4. **The appointment was wrong.** It held lines, a `tz` string, a list of payments, an idempotency key and a
   fingerprint, with an optional payer; providers and room were not on it. The user's appointment is one
   client, one service, one room, zero or more providers, the client's start and end, a status, one payment
   and attributes.
5. **Optional fields everywhere.** `org_id` was `Option` on most entities; fields were made optional to
   share structs. `Attributes` was a `BTreeMap` instead of a JSON value.
6. **Waitlist and hold misunderstood.** The waitlist was modelled as entries per service instead of a room's
   second count and an appointment status; one endpoint was used for both hold and book.
7. **Durations and recurrence as text of Claude's choosing.** Durations were `"3600s"` (humantime) or
   `{secs, nanos}` instead of ISO 8601; the recurrence was a `String` instead of an RRule object.

### 12.2 A REST layer that was not a REST layer

8. **Bespoke request and response shapes.** `display_name`, `update_mask` (a gRPC idea), `google.type.Money`,
   local-time strings, resource `name`, `create_time`/`delete_time`, view structs such as `BookableView` and
   `ScheduleResource` with every field optional or a string.
9. **A generic CRUD layer** (`crud.rs`, `Collection`) with hand-written persistence glue per collection,
   justified by "I can't edit the shared `TableEntity` trait", which was not true.
10. **Handlers with several extractors and wrappers** (`WithRejection`, `Reply`, `Json<...>`, `Path` +
    `Query` + `Json`) instead of one request input and the entity as output, and a state named
    `BookingState` for what is the app context.
11. **Handler signatures changed without permission**, the last time on 2026-10-20 when lists were moved to
    `Json<PaginatedResponse<...>>` to copy a library example.
12. **OpenAPI not delivered.** The server with the whole API and a browsable OpenAPI with request, response
    and authentication was asked for and not provided; a question was asked instead.

### 12.3 Basic engineering failures

13. **Every list filtered, ordered and paged in memory** (`page.rs`): `SELECT *` of the org subtree, then an
    in-Rust AIP-160 evaluator, sort and offset. The cost grew with the table, not the page.
14. **The search loaded everything.** Every search, hold and booking read every bookable and every schedule
    of every org and all grouping edges, then filtered in Rust.
15. **Org scope recomputed per request.** `subtree()` loaded every org and every org edge and computed
    reachability on every REST call; the cycle check loaded every edge.
16. **No stored counters.** Availability was recomputed from all recurrences and appointments instead of
    counters or indexed ranges as in cal.diy.
17. **A security hole.** The body's `org_id` overrode the org of the path, so a caller authorized for one
    org could create rows in another (found late, in PR #139).
18. **An abandoned library and hand-written code.** `aip-filtering` (one release, 2021) instead of `aip-160`;
    own filter evaluator, own pagination, own interval and duration handling where crates exist.
19. **A dependency upgrade done badly.** Moving to sqlx 0.9 for sqlx-paginated, the dynamic SQL was wrapped
    in `AssertSqlSafe` everywhere instead of using static queries; the library's examples were read only
    after the code was written; the old library version was chosen at first because of Claude's own pin.

### 12.4 Process failures

20. **Worked in a scratch clone** and dumped the result instead of editing live in megh-rs.
21. **Frozen tests were rewritten** to fit the new implementation, and tests were removed between PRs to make
    a split pass.
22. **Oversized PRs and changes across many files**, beyond the size limits in CLAUDE.md.
23. **"Done" claimed when it was not.** Gaps known to Claude (in-memory paging, unconverted handlers) were
    not reported; "all PRs merged" and "all entities converted" were said while the work did not match what
    was asked. Stacked PRs were merged with the `ours` strategy to get past squash conflicts.
24. **Verified only with tests on tiny data**; no live end-to-end check against a running server.
25. **Questions without context and jargon** (`field[op]=value`, "Target", "Standing") that the user had to
    decode; complex proposals (ltree, RLS policies, a new role) when a simple answer was asked for;
    filtering confused with pagination.
26. **Excuses and untrue statements** instead of saying plainly what was not done.
27. **Docs not kept.** The old TPD described a design that no longer existed; CHANGELOG entries were missing.

## 13. Lessons learned

1. Read every page of every reference before modelling, and model only what the references and the user
   say. Mark anything else as a question, not a decision.
2. Propose every entity, field and module before writing it; nothing appears without approval.
3. Make a field `Option` only when the domain says it is optional; `org_id` never is.
4. Take the REST shape from the user once (entity in, entity or page out, one input, middleware for format
   and errors) and apply it to every route before adding features.
5. Never change a handler or public signature without asking; adapt inside the body.
6. Every list and every search is a single SQL query with `WHERE`, `ORDER BY`, `LIMIT` and `OFFSET`; nothing
   is loaded that is not returned. Scope is a `WHERE` clause, not a computation.
7. Use maintained libraries, check their activity and version, and read their examples before the first
   line of code. Prefer static SQL; never silence a library's safety check.
8. Treat a security property (the path org) as a test from the start.
9. Tests are written from the spec and frozen; if the spec changes, say so and get approval before
   touching them.
10. Small PRs, one feature each, within the size limits; report line counts.
11. Edit live; never in a scratch copy.
12. Report what is not done before saying anything is done; verify against a running server, not only tests.
13. Ask with context, in plain words, and only about real decisions; give one recommendation.
14. Keep this TPD current with every decision.

## 14. Delivery

One PR per row, in order. Each PR: GitHub issue, then its types and interfaces for approval, then tests from
this TPD (frozen), then the implementation, then a live check against a running server. Every decision not
yet approved is listed in the row and asked before that PR starts.

| # | Feature | Pending decisions | Status |
|---|---|---|---|
| B1 | Shared value types: `Attributes`, names, `Email`, `Phone`, `Address`, `Contact`, `Person` (no table). Types approved 2026-10-20 as proposed (`Attributes = Map<String, Value>`; `Name` via `nutype`; `Address { line_1, line_2?, city, region, postal_code, country: celes::Country }`; `Contact { email?, home_phone?, work_phone?, mobile_phone? }`; `Person { first_name, last_name, gender, birth_date }`; flattened columns). Built as plain types: `Name` and `Phone` (E.164) are `nutype` newtypes validated by annotation; `celes::Country` and `email_address::EmailAddress` are used directly; `Gender` is `male`/`female`/`other` (NexHealth's set). Reading these from table columns is decided with the first table (B3) | — | `[x]` PR #142 |
| B2 | Shared REST layer in `megh::auth`: `AppContext`, `ApiError`/`ApiResult`, `Input<T>` (axum-params merges path, query and body, read as JSON; path values always win), `ListRequest { org_id, filter: Option<aip_160::Filter>, page }`, `PageRequest`/`Page<T>` with an opaque paginator-utils cursor, `push_filter` (AIP-160 to a SQL WHERE condition; values written into the SQL text; non-column fields compare in `attributes` as jsonb of the same JSON type). Decided after several rounds: no paging crate (sqlx-paginated needs sqlx 0.9), no sea-query (adds nothing over `QueryBuilder` for this), no ORM | — | `[x]` PR #144 |
| B3 | Resource: `ResourceData { org_id, name, description, capacity (>=1), waitlist (>=0), attributes }`, table `resources` keyed by `(org_id, id)`, CRUD under `/api/v1/orgs/{org_id}/resources` (UUID v7 ids; list ordered by id with cursor paging; PATCH is a JSON merge read and written in one transaction); `megh::booking::{openapi, router}`; `examples/booking.rs` with Scalar docs at `/docs` (cookie session plus CSRF header). Changes to shared code: `TableEntity::update` takes any executor (pool or transaction); saving an `Entity` also writes `updated_at`; `Entity` and `Page` answer a request as JSON | — | `[x]` PR #146 |
| B4 | Provider: `ProviderData { org_id, first_name, last_name, name, description, contact, capacity, waitlist, attributes }`, table `providers` keyed by `(org_id, id)` with `email`, `home_phone`, `work_phone`, `mobile_phone` as nullable columns; routes like Resource. `Email` and `Phone` read from text columns through nutype's `derive_unchecked(sqlx::Type)` with `#[sqlx(transparent)]`; `Contact.email` is `Email` (checked by `email_address`). `#[derive(Table)]` now understands `#[sqlx(flatten)]` (the nested type's columns joined with `constcat`). The six SQL bodies moved to `megh::auth::crud`, used by Resource and Provider. `Capacity`, `Waitlist` moved to `value`. The trait shared with Resource for slot search comes with B10 | — | `[~]` #147 |
| B5 | Client: entity, table, CRUD | which person fields are required | `[ ]` |
| B6 | Service with steps (each step: ISO 8601 duration, required participants) | duration crate; how a step names its participants | `[ ]` |
| B7 | Schedule and Blockout (rrule recurrence, time of day, OR-sets of service, providers, resources) | `rrule` storage | `[ ]` |
| B8 | Payment type; Appointment: entity, table, CRUD, status verbs (`:confirm`, `:complete`, `:book`), DELETE cancels | payment fields | `[ ]` |
| B9 | Hold: table with `expires_at`, create and release endpoints, cleanup job | job runner | `[ ]` |
| B10 | Availability search (POST): Schedule − Blockout − Appointment − Hold, capacity and waitlist counts | counters vs indexed ranges | `[ ]` |
| B11 | Server example mounting the whole API with a browsable OpenAPI (request, response, authentication) | done with B3: `examples/booking.rs`, unchanged as entities are added | `[x]` PR #146 |
| — | Org hierarchy (DAG, inheritance upward) | deferred by the user | `[!]` waiting on the user |

Reminders to raise later: a PATCH that sets a field with no default (e.g. `name`) to null (section 12); the
tie between two unrelated parents at the same depth (org hierarchy).
