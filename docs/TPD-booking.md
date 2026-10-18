# TPD — Generic Booking

Status: design resolved (issue #78). **Section 20 is the latest revision and supersedes earlier sections where they conflict** (nomenclature, blockouts, time, search, API). Delivered as small separate PRs, all merged (section 18). Written 2026-10-18 so a separate session can pick this up with nothing lost.

**Relocation (2026-10-18).** The design was first drafted in the unrelated `trika-rs` project by mistake. It belongs here, in `megh-rs`. Everything below applies to megh-rs; trika-rs issue #352 and its draft crate are superseded by #78 and `src/booking`.

**Layout.** A `booking` module in the `megh` crate (like `org` and `billing`), not a new crate. It follows the repo's conventions: persistent rows are `Entity<ID, T>` (id, `created_at`, `updated_at`, soft `deleted_at`), tables use `#[derive(Table)]` and numbered migrations, prices are micros plus currency (`money.rs`), and an aggregate is a struct holding the injected `PgPool`. The earlier "use a new crate" was said in the trika-rs context; the module was proposed for megh-rs and accepted with "go ahead".

**What megh-rs already provided, so the draft reuses it:** `Entity` and soft delete (`TableEntity::soft_delete` cascades along `ON DELETE CASCADE` keys; resolves the old open item about the soft-delete source), `Org` (a location, brand or franchise is an `Org` row), `Money` conversion (`money::from_micros`, `to_micros`), `Table` derive. Not provided and therefore new: a grouping graph over orgs and other entities. `org::Member` is a user's membership in an org, so the grouping edge is named **Grouping** here, not Membership, to avoid the clash.

Everything below is a resolution the user (Abir) gave in conversation, in their words where possible. Where something is my own reading it is marked **(my reading)**. Items not yet answered are in "Open items".

---

## 1. Purpose

A generic booking domain that works across dentistry, salons, gyms, events, restaurants, classes, grooming, massage, float pods and more. Dental (NexHealth) is only one reference; the model must not be dental-specific.

Entities in concern: a **service** to be booked, a **room/resource** that has capacity (and a wait list), and the **providers** the service may need. A **client** (patient) can book one or more services. A service can have **steps** (procedure codes in dental); each step may or may not need staff. Rooms can be engaged while the client is not (cleanup).

An appointment has three kinds of participant: clients, rooms/resources, providers. During the total time any of them may be engaged or free.

## 2. Standing rules that bind the implementation (from the user's global instructions)

- Design from the caller inward: caller code, then interface, then implementation.
- Issue first. Then **types and interfaces only**, presented for approval (checkpoint) before any tests or implementation.
- Tests are written against approved interfaces, derived from this specification, then **frozen**.
- No change to any file without being asked. No commit, push or PR unless asked; a commit always comes with a PR; report line counts before commit and PR.
- No suffixes `Manager`, `Handler`, `Service`, `Store`, `Repository`, `Helper`, `Util`, `Data`, `Info`, `Object`, `Entity`, `Item`, `Base`, `Impl` and similar. Name the thing, not the pattern.
- `Result<T>` style composition rather than `(T, error)` plumbing, no raw loops over collections in business logic (iterator adaptors), functions under 30 lines, no boolean flags in public APIs, no `if` nested more than 2 deep, locality of change, library functions over hand-rolled ones.
- No `any`-style escape hatches; for Rust this means typed interfaces. Stateful behaviour is encapsulated, not scattered.
- Memory notes in this project that apply: never decide design myself (ask with my reading); never skip approval checkpoints; no worktrees; no commit before review; never inspect inside a `serde_json::Value` except as explicitly allowed in 10 below.
- Soft delete: provided by megh-rs `Entity` and `TableEntity::soft_delete` (see `docs/TPD-soft-delete.md`). Every booking table needs `created_at`, `updated_at` and `deleted_at`, and cascades through `ON DELETE CASCADE` foreign keys.
- **Existing libraries, never hand-written code** (the user, 2026-10-18: "use existing library for all cases, instead of AI generated code, including for aip, rrule or any other case where a library is available"). Before writing any non-trivial helper, search crates.io and use the maintained crate; hand-written code only where no library exists, and then say so. See R9.
- megh-rs `AGENTS.md`: dependencies always at the latest release in their own `chore(deps)` PR; stage explicit paths only, never untracked user files.

## 3. Research done

The user supplied two references and was (rightly) unhappy that the first fetches were shallow. They were then read properly.

### 3.1 Mindbody Public API v6 (developers.mindbodyonline.com)

All 337 pages in the portal sidebar were fetched: model structures (320), enumerations (34 minus overlap), and the guide pages. The portal exposes no pages for the API methods themselves, only the models. Scheduling-relevant pages were studied; sales, payroll, gift cards and similar were fetched but not studied.

What was learned and where it landed in the design:

- `SessionType`: `DefaultTimeLength` and `StaffTimeLength` ("a specific Staff (when applicable)"), `NumDeducted` (sessions deducted from the pricing option), `ResourceRequired`, `AvailableForAddOn`, category and subcategory ids. Confirms **duration varies by provider** and that a service hierarchy (category/sub-category) exists. Shaped the Variant (section 8).
- `StaffSessionType`: `StaffId`, `TimeLength`, `PrepTime`, `FinishTime`, `PayRateType`, `PayRateAmount`. Confirms per-provider prep/finish and duration. Pay rate is out of scope.
- `Availability`: `StartDateTime`, `EndDateTime`, `BookableEndDateTime` (last start time), `PrepTime`, `FinishTime`, `ResourceAvailabilities`, `IsMasked`, `ShowPublic`.
- `Appointment` / `ItineraryEvent`: status enum (None, Requested, Booked, Completed, Confirmed, Arrived, NoShow, Cancelled, LateCancelled), `StaffRequested`, `ClientServiceId` (the pass paying), `IsWaitlist` / `WaitlistEntryId`, `Resources[]`, `AddOns[]`, `PreparationTime`, `FinishTime`, `GenderPreference` on the add request. An appointment can have several itinerary events (multi-service) and several resources.
- `Staff`: `AlwaysAllowDoubleBooking`, `AppointmentInstructor`, bio, contact. Double booking maps to capacity (decision 3).
- `Class`: `MaxCapacity`, `WebCapacity`, `TotalBooked`, `TotalSignedIn`, `TotalBookedWaitlist`, `IsCanceled`, `Substitute`, `Resource`, `IsWaitlistAvailable`. `ClassSchedule` uses per-day booleans plus enrollment flags; `Semester`, `Course`.
- `Program`: schedule type is `All | Class | Enrollment | Appointment | Resource | Arrival`. Became the service **Mode** set (section 9).
- `ClientService` (pricing option / pass): `Remaining`, `ExpirationDate`, `Program`, `Name`. Became PaymentMethod (section 12).
- `Spot` / pick-a-spot class: a specific spot in a class. Resolved as "just a resource" (decision 10).
- `WaitlistEntry`: client, `RequestDateTime`, `Web`, class schedule or enrollment date forward.

### 3.2 NexHealth Synchronizer API (docs.nexhealth.com)

Read: Basics, Practice overview, Scheduling, Scheduling Intent (beta), clinical-records index and v3.1.0 beta index; and the pages for appointments, appointment types, appointment descriptors, available slots, working hours, operatories, providers, locations, patients, procedures, intent search and intent book. Financial, clinical-record, auth/sync/webhook sections were only indexed.

- **Appointment type**: `minutes`, `bookable_online`, `parent_type`/`parent_id` (institution or location owned), `position`, `category`. Appointment types exist only in NexHealth. Maps to Service. **Appointment descriptors** are EMR records linked to a type, `descriptor_type` such as "Procedure Code", with `code`, `name`, `data`. Maps to Step attributes.
- **Appointment**: needs patient, provider, start; operatory required when the location maps by operatory; end derived from the type's minutes; `confirmed`, `cancelled`, `checkin_at`, `unavailable` (schedule blocks are stored as appointments), `foreign_id`. Creation is asynchronous with a write-back.
- **Operatory**: "a chair, column, or room where providers see patients"; lists appointment types it supports; **an operatory cannot hold more than one appointment at a time, but a provider can be booked in more than one operatory at the same time** (the hair-colour case).
- **Working hours**: per provider and operatory, `begin_time`, `end_time`, `days` or `specific_date`, `custom_recurrence {ref, unit, num}`, `tz_offset`, `appointment_types` allowed in the slot. Provider with no working hours has no availability. Available slots = working hours minus booked minus unavailable time.
- **Available slots**: `slot_length`, `slot_interval`, `appointment_type_id` sets length; response groups by location and provider, each slot carries `operatory_id` and `provider_id`.
- **Scheduling Intent**: search returns ranked slots with an opaque `slot_token` that **carries a hold window** (`slot_expired`, `slot_taken` errors); book takes `slot_token` and a required **`idempotency_key`** (replay returns the original booking, never a double booking); `booking_status` is separate from `ehr_writeback`.
- **Patient**: `guarantor_id`, `bio`, `preferred_language`, `billing_type`. Only documented link to another person is the guarantor. Maps to the payer on a Booking.
- **Procedure**: `status` planned/scheduled/completed/referred/inactive, `body_site`, `fee`. Per-treatment data; attribute-level in our model.
- **Provider**: `npi`, `bio`, per-location `provider_requestables` (where patients may book them online), `availabilities`. Providers are location-scoped.

## 4. The scenarios the model must cover

Quoted from the user, with the mapping in section 14.

a) Hair colour: a provider does colouring, moves to another client while the colour dries, comes back to finish.
b) Dentist + hygienist on one patient: hygienist sets up, dentist does one or more procedures, hygienist does the cleaning.
c) Gym: client checks in, works out, checks out; until capacity is full; no provider.
d) One-time TED talk with capacity n; up to n bookings, possibly a wait list.
e) Hotel/restaurant: n customers book a table of n, optionally a waiter with no engagement time defined (comes on demand).
f) Recurring music class every Thursday and Sunday 6–7 pm, capacity n; either fixed pre-booked schedule, or a learner buys a 30-visit pass and may attend those times on any days during the pass validity.
g) Pet grooming appointment.
h) Couples massage: one room, two people simultaneously, one or more providers.
i) Float pod: a resource/room, likely no provider.

Further needs stated: duration and price can depend on provider or client (master stylist, new-patient emergency); a booking can be held temporarily while the client decides (bus/airline style) and not offered to anyone else; ready-made templates for the common shapes so a simple service needs no elaborate setup.

## 5. Principles the user stated (binding on the design)

1. Schedule is always **open time plus exceptions** (recurring exception like lunch; holiday list). They can be defined at org, location, room, service, provider and so on. Schedule and exceptions are a function of the participation axes.
2. **No hardcoded online flag.** Everything is tags, filterable.
3. **Double booking is a special case of capacity.** Capacity 2 means two bookings at the same time in the same room.
4. **Eligibility is nothing but a schedule.** Service S can have working hours for provider P and room R on certain recurring days. A schedule may specify all axes or only some (gym hours given alone mean all rooms open the same days and times); or be very specific (tooth cleaning, Dr Albert, Wed/Thu 3–6 pm).
5. **No hardcoded constraints.** One may pass any set of tags to find open slots: specific provider, any female provider, only master stylist, a doctor with 10+ years experience, a sea-facing room, a table with a kids high chair, a disabled-friendly seat.
6. **Add-ons are just a set of services.** One can book one or more services (back to back: haircut + hair colour) or in a group (two people in parallel, couples massage). Add-ons are incorporated like the service hierarchy (a menu/catalogue; Mindbody calls it category/sub-category with program).
7. **No status enum.** A booking is booked or cancelled (soft delete). Whether a cancel was late, or whether the client attended, is additional metadata, not part of booking.
8. **Payment is generic**: who pays / the primary, and how: pass, voucher, coupon, insurance, cash, card. It boils down to a prepaid option that debits, or on-spot payment. A generic type with a name, and eligible for that appointment or not.
9. **No staff-substitution logic.** It is configuration; the business changes the staff for a schedule and notes it in metadata.
10. **Pick a spot / table / seat is not special.** It is "just a booking": specify a room/resource.

Additional principles from the dialogue:

- Buffers before/after are part of the Step. A step has a duration and participants (client, room, staff). No separate buffer concept.
- Resource and provider are similar as far as appointments are concerned; they differ in how to communicate with them and what is stored about them. For scheduling both are constraints.
- Every entity can have assorted metadata. Living and non-living entities carry additional information. For a group booking: the payee, primary person, insurance holder and similar.
- Participants are grouped by **tags, not rigid grouping** (hair dresser, master stylist; dentist, hygienist). Resources can have a hierarchy (lobby 1, room 1).
- Recurrence uses **rrule with an appropriate library** (the `rrule` crate was proposed; not yet confirmed as the pick).

## 6. Resolved decisions, in order

| # | Decision | Source |
|---|---|---|
| D1 | Buffers are Steps, not a separate concept. Cleanup = trailing Step, client absent, room engaged. | "buffer_before and buffer_after can be part of the step" |
| D2 | Provider and resource merge for scheduling into one type named **Bookable**. Profile differences (contact, licence, equipment) live beside it, not in the scheduling core. | "resource & provider are similar ... both are constraints"; name approved "1. looks ok" |
| D3 | Duration and price variants are a **delta on the whole service**, not per step. | "adjustments are usually full service. e.g. original one + some delta" |
| D4 | Recurrence via an rrule library. | user |
| D5 | **Hold**: temporary block while the client decides; not offered to others. | user |
| D6 | Templates / ready-made constructors for common shapes. | user |
| D7 | Scheduler rejects any conflict: schedule unavailable, staff unavailable, room unavailable, and client unavailable if the client's calendar is visible. | "yes. any conflict in constraints will be rejected" |
| D8 | Availability = open rules and closed rules over axes, with inheritance (section 10). | principles 1, 4 |
| D9 | Only **Organization**; no separate Location. A location is an org; brand and franchise are orgs too. Grouping is a **graph**, not a tree, for every entity. | "location is just another org ... not necessarily a tree" |
| D10 | Tags and metadata are **one key/value map**; values are `serde_json::Value`. | "Tags/metadata is key value pair"; "just serde json value" |
| D11 | Conditions over the map are **predicates**, to be scriptable later and storable. | "just a predicate defined (may be a little scripting language ... which can be stored)" |
| D12 | An omitted axis in a search means **any eligible** member per the schedule. | tooth-extraction example |
| D13 | Hold is not a booking state. It is a semi-formed booking: may have no client or payment. Start with **two types**; collapse into one later if it works well. | "hold is not a booking state" / "can be done, if this works good, can be collapsed" |
| D14 | Rule inheritance goes inner to outer; nearest definition wins (section 10). | user |
| D15 | Longest path decides "nearest" for nodes with several parents; equal distance does not occur in practice. Keep a note. | user |
| D16 | The code lives in a **`booking` module of the `megh` crate** in megh-rs (not trika-rs). First drafted as a new crate in trika-rs by mistake. | "this is not for trika-rs. it should be done on megh-rs"; "go ahead" to the module proposal |

## 7. Entity model

Every persisted entity is `Entity<ID, T>` (id, `created_at`, `updated_at`, soft `deleted_at`) and carries the one key/value map (D10) in its `attributes`. Prices are micros plus ISO currency, as in `money.rs`.

- **Organization**: tenant and place; this is the existing megh `Org`. Brand, franchise, clinic, floor are all `Org` rows. Grouping forms a graph.
- **Grouping**: an edge from a member to a group (named Grouping because `org::Member` already means a user in an org). Applies to every entity kind: organizations, bookables, services, clients. Cycles are rejected. Used for lobby 1 → room 1, master stylist as a loose group, category → sub-category → service, brand → location.
- **Client**: the one served. Person, pet or other; the kind is metadata. A pet's guardian is a payer/party on the booking, not special structure. Whether Client→guardian is also a relation was asked and not answered (Open items).
- **Bookable**: provider or room. Attributes in the map; **`capacity`**; belongs to an organization; groups by Grouping. Capacity 2 = double booking (D3).
- **Service**: a catalogue node. Either a group (category, sub-category, menu; children via Grouping) or a leaf with an ordered list of **Steps**. Add-ons are Services in the same catalogue.
- **Step**: `duration`; whether the client is `Present` or `Absent`; list of **Needs**; attributes (procedure codes, descriptors).
- **Need**: a predicate/selector over Bookables, a count, an **Engagement**.
- **Engagement**: `Engaged` (blocks the participant during the step), `Free` (participant is released but the booking still holds the client and room; the colour-drying case), `OnDemand` (assigned, no blocked time; the waiter).
- **Variant**: `{ when: predicate over provider/client/context, adjust: delta on whole Service (duration, price) }`. Resolved at booking time once provider and client are known; the result is stored on the Booking.
- **Availability rule**: axes (organization, service, bookable; any may be omitted), a recurrence, and a kind `Open` or `Closed`. See section 10.
- **Occurrence**: one dated instance of a Service in `Class`/`Arrival` style modes with shared capacity.
- **Booking**: the reservation. Lines (Client, Service, seats), a payer (primary), payments, metadata. Booked or soft-deleted (cancelled). No status enum (principle 7).
- **Allocation**: who is engaged during which interval and how. Generated from a Booking's Steps. **Only Allocations block time.**
- **Hold**: temporary Allocations with `expires_at` and an idempotency key. A semi-formed Booking (D13). Converts to a Booking.
- **PaymentMethod**: name, key/value map, eligibility for Services (predicate), and kind `Prepaid` (balance/uses and validity) or `OnSpot`. Pass, voucher, coupon, insurance, cash, card are all instances. A Service may declare how many uses a booking consumes (Mindbody `NumDeducted`).
- **Payment**: a PaymentMethod and an amount/uses on a Booking.
- **WaitlistEntry**: a request pointing at an Occurrence or a requested window, with position.
- **Enrollment**: a client's fixed series that books Occurrences of a recurring class (scenario f, fixed schedule).

## 8. Variants (price and duration)

- A Variant is a condition plus an adjustment: duration (set or add) and price (set or add), applied to the whole Service (D3).
- Examples: master stylist lengthens a colour; new-patient emergency adds time.
- Conditions are predicates (D11) over the resolved provider and client maps.
- Mindbody evidence: `StaffTimeLength` overriding `DefaultTimeLength`.

## 9. Service Mode

Mindbody's Program types are the model for Modes:

| Mode | Behaviour | Scenarios |
|---|---|---|
| Appointment | The system finds a time and generates Allocations on Bookables | a, b, g, h, i |
| Class | Fixed Occurrences, shared capacity, waitlist | d, f |
| Enrollment | A fixed series booked as a whole | f (pre-booked) |
| Resource | Room/capacity with an optional on-demand provider | e |
| Arrival | Capacity over an opening window with check-in/out | c |

(My reading: the user did not confirm the exact names; they follow Mindbody's.)

## 10. Availability, exceptions and inheritance

Rules, as the user specified:

1. A schedule is **open time**; exceptions subtract from it. **Available time = Open − Blocked.**
2. Open rules and Closed (blocked) rules may each be defined on any axis: organization, service, bookable (provider, room), and combinations. Axes not named on a rule mean "any".
3. A gym's hours given on the service/organization alone open every room the same days and times. "Tooth cleaning, Dr Albert, Wed/Thu 3–6 pm" fixes service, provider and times.
4. **Eligibility is the schedule.** There is no separate eligibility entity; if no Open rule matches the combination, it cannot be booked.
5. **Inheritance goes inward to outward**, nearest definition wins, applied to open hours and to blocked hours independently:
   - If the room defines open hours, use them; else the location (organization); else the brand; and so on outward.
   - Blocked hours: if defined on the room (e.g. instrument breakdown), that is used; else traverse up (e.g. a clinic closure or holiday).
   - The result is Open − Blocked.
6. **Longest path decides nearest** when a node has several parents: L → Brand and L → Franchise → Brand: take the nearest definition along the longest path, so if both brand and franchise define, the franchise wins. **Keep a note** (this section is that note). Equal-distance ties do not occur in practice, so no tie rule is needed now.
7. Group rules apply to all members **transitively** (a rule on a brand covers all its locations). Cycles are rejected.
8. Recurrence uses rrule (D4). Rules also support specific-date entries (holidays), like NexHealth `specific_date`.

(My reading, to be confirmed when implementing: the nearest-wins override is per kind, open and blocked each resolved separately; overriding means the nearer level replaces the farther level's rules of the same kind for that axis.)

NexHealth evidence: working hours per provider+operatory with `days`/`specific_date`/`custom_recurrence`, available slots = working hours minus booked minus unavailable.

## 11. Search

- Input: a set of conditions (predicates over the key/value maps), a window, and a layout.
- Layout: `Sequential` (haircut then colour, back to back) or `Parallel` (couples massage, two people in one room). Layout is a search parameter; the result is Allocations.
- An omitted axis (provider, room) means any eligible per the schedule (D12).
- Predicates cover: a specific provider, any female provider, only master stylist, 10+ years experience, sea-facing room, table with kids high chair, disabled-friendly seat. All are conditions on the maps; nothing is hardcoded.
- Output: candidate slots, each with the Allocations that satisfy every Need. Every conflict is rejected (D7).
- Hold-then-book flow modelled on NexHealth: a search result can be held (D5, D13), then confirmed into a Booking with an idempotency key so retries never double-book.

## 12. Hold, Booking and Allocation

- A Hold blocks the schedule without being a Booking. It can lack a client and payment (D13).
- On confirmation it becomes a Booking; at that point the client, payer and payments are attached.
- Idempotency: confirm takes a caller-chosen key; replay returns the original Booking.
- Cancellation = soft delete of the Booking (principle 7); Allocations stop blocking time.
- Additional facts (late cancel, attendance, notes, who changed the staff) are metadata (principles 7 and 9).
- Later option: collapse Hold and Booking into one type (a Booking with optional client/payments and `hold_until`).

## 13. Payment

- PaymentMethod and Payment as in section 7. Generic: name, map, eligibility predicate, prepaid or on-spot.
- The primary/payer of a booking is a reference on the Booking. For a group booking the payee, primary person and insurance holder are roles stored as metadata/parties on the Booking.
- Eligibility for a given booking is evaluated by the PaymentMethod's predicate, not by hardcoded types.

## 14. Scenario mapping

| Scenario | Model |
|---|---|
| a) Hair colour | Service with three Steps: stylist Engaged (prep), stylist Free while client and chair remain held (processing), stylist Engaged (finish). During the Free step the stylist can take another booking. |
| b) Dentist + hygienist | Steps: hygienist Engaged, dentist Engaged (one or more procedures as step attributes), hygienist Engaged. Room cleanup is a trailing room-only Step. |
| c) Gym | Arrival mode; Bookable floor with capacity n; no provider Need; check-in/out. |
| d) TED talk | One Occurrence, capacity n, waitlist. |
| e) Hotel table | Resource mode; Bookable table, capacity n; waiter Need with `OnDemand`. |
| f) Music class | Class mode; recurring Open rule Thu & Sun 18:00–19:00 via rrule, capacity n; either an Enrollment (fixed schedule) or a prepaid PaymentMethod (30-visit pass with validity) used to book any Occurrence. |
| g) Pet grooming | Appointment; Client is the pet; payer is the owner (a party on the Booking). |
| h) Couples massage | One Booking, 2 seats in one room (Bookable capacity ≥ 2), one or more provider Needs; layout `Parallel`. |
| i) Float pod | Appointment/Resource; Bookable pod; no provider Need. |

## 15. Templates (ready-made constructors)

All return ordinary Services built from ordinary Steps; there are no special template types. Caller-side sketch, **illustrative and not approved** (the interfaces checkpoint comes first):

```rust
Service::simple("Consultation", 60.min())                      // one step, one provider, one room
Service::simple("Therapy", 50.min()).then_cleanup(10.min())    // + 10 min room-only trailing step
Service::processed("Hair colour", prep: 10.min(), wait: 30.min(), finish: 20.min())
Service::handoff("Dental procedure", [(hygienist, 15.min()), (dentist, 30.min()), (hygienist, 20.min())])
Service::room_only("Float", 60.min())                           // no provider
Service::session("Music class", 60.min(), capacity: 20)         // fixed Occurrence
```

Each takes tag predicates for provider and room with sensible defaults, and can be adjusted with `.variant(...)` and `.with_price(...)`.

## 16. Explicitly removed or rejected (do not reintroduce)

- Separate buffer fields (D1).
- A separate Provider vs Resource distinction in the scheduling core (D2).
- A separate Location entity (D9).
- A separate Eligibility entity (principle 4).
- Online-bookable flag (principle 2).
- Overlap / double-booking flag (principle 3).
- Booking status enum, late-cancel/attendance as booking fields (principle 7).
- Staff-substitute logic (principle 9).
- Pick-a-spot as a special feature (principle 10).
- Pass as its own entity (it is a PaymentMethod).
- Pay rate / commission (out of scope).

## 17. Open items (not answered; do not decide silently)

1. ~~Soft-delete source~~ resolved: megh-rs `Entity` / `TableEntity::soft_delete`.
2. ~~Crate name~~ resolved: a `booking` module of the `megh` crate (D16).
3. **PRD**: megh-rs has only `docs/TPD-*.md`; no PRD entry is planned unless asked.
3a. **`Org` as a place**: `Org` has billing fields (`sub_status`, `plan_id`, `trial_ends_at`) and no parent. A location or franchise as an `Org` row carries them; whether that is acceptable, and where the grouping of orgs is stored, needs a decision before migrations.
4. **Predicate representation**: typed predicates now, scripted later (D11). The first concrete form (a Rust enum/trait of comparators over `Value`, versus an embedded expression language) is unresolved. Comparing a number inside a `Value` (e.g. `experience_years >= 10`) reads inside the `Value`; the user's answer was that conditions are predicates, so the predicate owns that, but the exact API needs the interface checkpoint.
5. **Client calendar visibility**: D7 says client conflicts reject "in case client calendar is also visible". How visibility is expressed is undecided (a map entry on the client is a natural fit).
6. **Client → guardian**: a relation on Client, or only a party role on each Booking. Asked once, not answered.
7. **Step ordering**: strictly sequential, or may Steps overlap (graph)? Asked once, not answered; the current sketch is an ordered list.
8. **rrule crate choice**: user said "an appropriate library"; the `rrule` crate was proposed, not confirmed.
9. **Hold/Booking collapse**: start separate (D13), revisit.
10. **Mode names**: follow Mindbody (section 9); not explicitly confirmed.
11. ~~Persistence~~ drafted in section 19, awaiting approval.

## 18. Delivery order

Each row is its own issue, branch and PR, merged before the next starts (the user: "each feature, make separate PR and merge; do not make a giant blob which cannot be reviewed"). The tests were written from this document and frozen before the implementation; a PR carries the tests of its feature.

| # | PR | Status |
|---|---|---|
| 0 | Design (this document), issue #78 | `[x]` |
| 1 | Core types, migration `0009_booking.sql`, `Table` mapping test (PR #82, issue #81) | `[x]` |
| 2 | Service templates (PR #84, issue #83) | `[x]` |
| 3 | Schedules, blockouts and availability (`Calendar`) (PR #86, issue #85) | `[x]` |
| 4 | Availability search with parties, service gap and party offset (PR #88, issue #87) | `[x]` |
| 5 | Appointments: hold, confirm, cancel, payment (PR #90, issue #89) | `[x]` |
| 6 | Waitlist (PR #92, issue #91) | `[x]` |
| 7 | AIP-160 filter predicate (PR #94, issue #93) | `[x]` |
| 8 | REST: schedules and blockouts (PR #96, issue #95) | `[x]` |
| 9 | REST: `availability:search` (PR #98, issue #97) | `[x]` |
| 10 | REST: appointments (PR #100, issue #99) | `[x]` |

## 19. Persistence (migration `0009_booking.sql`, shipped in PR 1)

Every table has `id UUID PRIMARY KEY`, `created_at`, `updated_at`, `deleted_at` (an `Entity<ID, T>` row). `ON DELETE CASCADE` keys are the soft-delete cascade. JSON columns are `JSONB`. The key/value map is the `attributes` column.

| Table | Columns beyond the stamps | Keys |
|---|---|---|
| `bookables` | `org_id`, `name`, `capacity > 0`, `attributes` | `org_id` → `organizations` cascade |
| `clients` | `name`, `attributes` | |
| `services` | `name`, `mode`, `price`, `steps`, `variants`, `attributes` | `mode` checked against the five Modes |
| `groupings` | `kind` (`org`, `bookable`, `service`, `client`), `member_id`, `group_id` | unique `(kind, member_id, group_id)` while live; no foreign keys (polymorphic) |
| `schedules` | `org_id`, `service_id`, `bookable_id` (NULL = any), `kind` (`open`/`blockout`), `recurrence`, `span`, `reason`, `notes` | each axis cascades |
| `occurrences` | `service_id`, `starts_at`, `capacity`, `attributes` | `service_id` cascade |
| `enrollments` | `client_id`, `service_id`, `occurrences`, `attributes` | both cascade |
| `payment_methods` | `name`, `owner_id` (NULL = shared), `eligible`, `settlement`, `attributes` | `owner_id` cascade |
| `holds` | `org_id`, `tz`, `parties`, `expires_at`, `attributes` | |
| `appointments` | `org_id`, `tz`, `lines`, `payer_id`, `payments`, `idempotency_key` (unique), `fingerprint`, `attributes` | `payer_id` → `clients` |
| `occupancies` | `hold_id` xor `appointment_id`, `bookable_id`, `starts_at`, `ends_at > starts_at`, `engagement`, `seats > 0` | owners and `bookable_id` cascade |
| `waitlist_entries` | `client_id`, `target` (JSON), `requested_at`, `attributes` | `client_id` cascade; GIN index on `target` |

Decisions that follow from persistence (my reading, to confirm):

- **Occupancy is the persisted Allocation** and the only thing that blocks time. Cancelling an appointment soft-deletes it and the cascade removes its occupancies from blocking; a hold blocks until `expires_at`. `Hold` and `Appointment` therefore no longer carry an `allocations` list; they are read from `occupancies`. The `Allocation` struct stays as the search result (`Slot`).
- **Idempotent confirm**: `bookings.idempotency_key` is unique and `fingerprint` identifies the request; the same key with another fingerprint is `Error::KeyReused`.
- **Grouping is not generic** any more: one table for all kinds with a `kind` column, the ends are plain `Uuid`.
- **Conflict checks** read `occupancies` for the bookable and the interval, counting only `engaged` rows against capacity (a `free` provider is released), ignoring expired holds and deleted rows.
- `capacity` and `seats` are `u32` in Rust and `INT` in SQL (`try_from = "i32"`); durations, prices and steps are JSON.

## 20. Revision of 2026-10-18 (latest; supersedes earlier sections where they conflict)

**R1. Nomenclature** (user: "better name Schedule and Blockout ... consistent nomenclature").

| Term | Meaning | Rust |
|---|---|---|
| Schedule | times open for work | `Schedule` with `Kind::Open` (was `Availability`) |
| Blockout | closed time with a reason and notes: holiday, lunch, breakdown, time off | `Schedule` with `Kind::Blockout` (was `Kind::Closed`); one table `schedules`, columns `reason`, `notes` |
| Appointment | a held or booked reservation, which also blocks time | `Appointment` (was `Booking`), `Hold` stays; one REST resource with `state` `held`, `booked` or `cancelled` |
| Availability | computed: Schedule − Blockout − Appointment | trait `Availability` (was trait `Schedule`), implemented by `Calendar` |

**R2. Blockouts do not compete** (user: "a closed holiday not to be compared with closed lunch break"). Supersedes D14 for blockouts: every blockout whose axes match the combination is subtracted, whatever its level. Schedules keep nearest-wins (inner to outer, longest path). A room under a clinic blockout cannot be re-opened by its own schedule.

**R3. Time.** Every operation is in the org's local time: requests and responses carry local date-times (RFC 3339, AIP-142), an offset is accepted but not relied on (daylight saving). Storage: a `timestamptz` instant plus an IANA `tz` column on `occurrences`, `holds` and `appointments` (a snapshot of the org zone at creation); schedule and blockout recurrences carry `TZID` in the recurrence text. Postgres `timestamptz` keeps only the UTC instant, so the zone is a separate column; the engine works in UTC instants and the REST layer converts. (The user: "storage can be whatever db supports"; this choice keeps overlap and capacity queries correct across daylight-saving changes.)

**R4. Availability search** (user: "c1 => s1,s2; C2 => s1,s3 ... two control parameters, default to 0"). The request is a list of **parties**, each a client with an ordered list of services (each with `wants` filters); two controls, both default `0s` meaning the exact same time:
- `party_offset`: the largest difference between the parties' start times (0: all start together; 30 minutes: 9:00 and 9:30).
- `service_gap`: the largest gap between one party's consecutive services (0: back to back; 30 minutes: 9:00-9:30 then 10:00-10:30).
Starts and gaps are tried on a `slot_interval` grid up to the maximum. The response lists the parties with, per service, the start, end and allocations, and `next_available_day` (the first local date with any slot, searched forward up to `horizon_days`, default 180 as NexHealth) when the window has none. This replaces `Layout` and `Line`: one party with several services is the old sequential layout, several parties the old parallel layout.

**R5. Filters.** Conditions (needs, variants, wants, payment eligibility, appointment search) are **AIP-160 filter expressions** over the key/value map, for example `role = "stylist" AND experience_years >= 10`. The built-in predicate stores the expression string (scriptable and storable, as D11 asked); REST payloads carry the plain string. Crates compared when the filter was built: `bomboni_request` (needs `protoc` to build, and a system dependency is not acceptable), `aip-160` 0.1.5 (strict, but no `.` traversal, no implicit AND, and AND binds tighter than OR, the reverse of the spec) and `aip-filtering` 0.1.0 (follows the spec grammar, but accepts garbage such as `a =`, reads `10.5` as a traversal and panics on an empty filter). **`aip-filtering` is used**, with guards in `Filter::parse`: an empty text is no filter, a panic is caught, and what the parser would mis-read is refused as `Invalid` (functions, a comparator without spaces, a missing right side, decimal numbers). Evaluation against the key/value attributes is ours, since no crate evaluates a filter over JSON. Revisit when either crate matures.

**R6. API conventions: Google AIP, never an invented convention** (user: "ALWAYS keep that in mind"). Resources `/api/v1/orgs/{org_id}/{collection}` and `.../{collection}/{id}` (the user first wrote `/api/v1/org/...`; changed to the plural `orgs` of AIP-122 on 2026-10-18), standard methods Get, List, Create, Update (PATCH with `update_mask`), Delete (AIP-131 to 135), custom methods `POST ...:action` (AIP-136), `page_size` and `page_token` (AIP-158), `filter` and `order_by` (AIP-132, AIP-160), `request_id` for idempotency (AIP-155), durations as `"1800s"` and timestamps as RFC 3339 (AIP-142), errors as `{"error": {"code", "message", "status"}}` (AIP-193), soft delete with `show_deleted` (AIP-164). Grants come from `request_grants`: one `resource:action[:id]` per resource level.

| Route | Does |
|---|---|
| `.../schedules`, `.../blockouts` | CRUD |
| `POST .../availability:search` | open slots and `next_available_day` (POST because the payload is rich) |
| `POST .../appointments` | create with `state` `held` or `booked`; `GET` list and get with filters (id, window, org subtree, state, bookable, service, client) |
| `POST .../appointments/{id}:confirm` | confirm a hold; the appointment keeps the hold's id; `request_id` makes it retry-safe |
| `DELETE .../appointments/{id}` | cancel (soft delete) |
| waitlist and payment methods | small CRUD |

Bookables, services and clients: whether they get REST CRUD or are created by the host app in Rust is **undecided** (asked, not answered).

**R7. Tenancy.** `clients`, `services`, `payment_methods`, `occurrences`, `holds` and `appointments` carry a nullable `org_id`; REST always sets it from the path. Rules owned by an org only match that org and its members, so the existing engine is tenant-safe.

**R8. Review process.** Interfaces, then tests, were approved and frozen before the implementation. One frozen test was fixed with the user's permission (`a_provider_without_an_open_rule_is_never_offered`: its service carried a service-wide Open rule, which by principle 4 opens every bookable). The frozen tests were then renamed mechanically for R1 with the user's approval.

**R9. Libraries** (user, 2026-10-18, binding for every PR). Use the existing crate for: recurrence (`rrule`), AIP-160 filters (`aip-160`), interval sets (`rangemap` instead of the hand-written merge and subtract), combinations and cartesian products (`itertools`), the grouping graph, cycle detection and longest path (`petgraph`), AIP-142 durations (`humantime`), time zones (`chrono-tz`), JSON merge patch for PATCH (`json-patch`, already a dependency), serialization of conditions (`typetag`). A draft of the first PRs was written with hand-written interval, combination and graph code; it is replaced by these crates in the PR that introduces each feature. Before adding any other helper, search crates.io first.

**R10. Conventions of the REST layer as built** (PR 8). `booking_router(pool)` and `booking_router_with_csrf(pool, csrf)` return an axum `Router` with the full paths `/api/v1/orgs/{org_id}/...`; the application authenticates, puts a `Member` in the request extensions and applies `authorizer`. Writes are CSRF-protected like `events_router` (the stateless `tower-http` check). Per AIP: resource `name` (`orgs/{org_id}/schedules/{id}`) and `create_time`, `update_time`, `delete_time` (AIP-148), times in the org's zone with the offset, durations as `"3600s"` (AIP-142, parsed with `humantime`), `Get` of a deleted resource returns it with `delete_time` and `List` hides it unless `show_deleted=true` (AIP-164), `update_mask` on `PATCH` (AIP-134; without a mask the fields in the body), `filter` (AIP-160, the `Filter` predicate over the resource's JSON fields), `page_size` (default 50, at most 1000) and an opaque `page_token` (AIP-158), lists named after the collection with `next_page_token`, errors `{"error": {"code", "message", "status"}}` (AIP-193). A schedule or blockout belongs to the org of the path or to an org grouped under it, and its org, bookable and service must too. The time of a schedule is `recurrence` (RFC 5545 with `TZID`) and `span`, or the shorthand `days` or `date` with `start_time` and `end_time` in the org's time zone. Lists read the rows of the org's subtree and filter and page in memory.

**R11. `availability:search` as built** (PR 9). `POST /api/v1/orgs/{org_id}/availability:search` takes `parties` (each with `client_id`, `seats` and `services`, each service with `service_id` and `wants`, a list of AIP-160 filters), `start_time` and `end_time` (a time with an offset, or local time read in the org's zone, at most 31 days apart), `party_offset`, `service_gap` and `slot_interval` (AIP-142 durations, defaults `0s`, `0s`, `900s`), `page_size` (default 20, at most 100; no page token, ask again from a later `start_time`) and `horizon_days` (default 180, at most 366). It answers `slots`, each with `start_time`, `end_time`, `price` as `google.type.Money`, `parties` (the start, end and price of each service) and `allocations`, and `next_available_day` (a local date, only when there are no slots). Services and clients must belong to the org's subtree or to no org, else `NOT_FOUND`. The grants are `orgs:read:{org_id}` and `availability:search`.

**R12. Appointments over REST as built** (PR 10). One resource, `/api/v1/orgs/{org_id}/appointments`, with `state` `held`, `booked` or `cancelled`; the two Rust types (hold and appointment) stay behind it and a confirmed hold keeps its id.
- `POST` with `state: "booked"` (needs `request_id` and `payer_id`; the same `request_id` returns the same appointment) or `state: "held"` (until `expire_time`, default 10 minutes, at most 24 hours; no payer, clients or payments yet). The body carries the `slot` exactly as `availability:search` returned it. **The server does not trust it**: it searches again for those parties and times and holds the slot it finds, so a crafted slot (another org's bookable, a shorter service, a time that is not open) is refused with `INVALID_ARGUMENT`.
- `POST .../appointments/{id}:confirm` with `request_id`, `payer_id`, optional `clients`, `payments` (`{"method", "amount": {"uses": n}}` or `{"money": google.type.Money}`) and `attributes`. axum does not match a suffix after a path parameter, so the route is `{id}` and the handler reads the `:verb`; the authorizer reads it from the real path (`appointments:confirm:{id}`).
- `GET` one (a released hold shows as `cancelled`; a hold that ran out is gone) or a list, which holds booked and held appointments, earliest first. Filter fields (AIP-160): `state`, `start_time`, `end_time` (RFC 3339 with the org's offset), `payer_id`, and the repeated `client_ids`, `service_ids`, `bookable_ids` (use `:`); `show_deleted=true` adds the cancelled ones. An org lists its own appointments and those of the orgs grouped under it.
- `DELETE` cancels (soft delete) a booked appointment or releases a hold and returns the resource with `delete_time`.
- Not done: `order_by`; a payment method's `eligible` condition is not checked at confirm; replaying a booked `POST` with the same `request_id` and a different body returns the original instead of an error; `updated` appointments (rescheduling) and `:undelete`.

**R13. Plural `orgs`** (user, 2026-10-18). Every REST path is `/api/v1/orgs/{org_id}/...`, resource names are `orgs/{org_id}/...`, and the first grant of every route is `orgs:read:{org_id}`. Collection ids of several words are lowerCamelCase (AIP-122), so payment methods will be `paymentMethods`.
