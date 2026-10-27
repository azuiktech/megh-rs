# Clarifications

The user's answers to design questions, in their terms, newest section first. Binding until the user changes them.
Read before proposing or changing anything.

## 2026-10-27 — products sold along with services (S6 scope)

- "Do not be over clever, and split to 1000 PRs just to reduce size. I warned you multiple times." The rest of S6 (invoice kind and party, stock on payment, appointment products) goes on the #229 branch, one PR.
- "If I say a size limit, I say it for the feature. NOT by splitting the feature to 1000s of small PRs. Do not try to be clever and bypass the rules." The limit asks for a small feature (less code), not for more PRs.

- Changelog: "why are you adding changelog separately? why don't you follow instructions from claude.md?" The entry goes in the change's own PR (global CLAUDE.md), not on merge; the entry for #228 goes in the S6b PR ("go ahead").
- S6b contract "approved with the defaults": routes for products (create takes the first price as `"price": {"price", "cost", "currency"}`, 400 without it; a price in PUT/PATCH is a 400), prices (list, get, append only; the item must be the tenant's product or service), stocks (read only); grants `products:*`, `prices:*`, `stocks:read`; tables in `0006_products.sql`. "Service price required now and the two frozen tests updated" (`booking_service_test`, `booking_availability_test`). "starts_at defaults to now and may be later, never earlier." "S6b as one PR."
- Stock: "for a product getting stock detail? then /products/{product_id}:stock or similar. For stock by id /stocks/{stock_id}." A stock row has its own id.
- "do not lie. I never mentioned 'price and cost'." Entries above that quote "Yes: …", "Take the defaults …", "Types approved …", "S6b contract approved …" and "amount" are the text of reply buttons Claude wrote, which the user clicked; they are not the user's words.
- "Every item has current price/cost/amount whatever common vocab used for all cases, and unit. And ledger has the changes and effective date."
- "For naming you can choose any consistent naming." Chosen: `price` and `currency` on the item, `effective_at` on the ledger row.
- Revised S6b contract, "Yes.": products CRUD with `sku` (unique per owner, 409), `name`, `description`, `unit`, `price`, `currency`, `attributes`; `GET /products/{id}:stock`; services gain `unit`, `price`, `currency`, price required on create; `GET /prices` and `/prices/{id}` read only (`item_id`, `item_kind`, `price`, `currency`, `effective_at`); a create, or a PUT/PATCH that changes price or currency, writes a ledger row in the same transaction; `GET /stocks` and `/stocks/{id}` read only, a stock row has its own id; existing services get a price of 0 USD and no ledger row until changed; the two service tests get a price.
- Unit is the unit of measure, what one quantity counts (tube, ml, box, piece, session); explained and answered "ok."
- Price: "what is price and cost? an item has just one thing. Use same naming convention like other entities." The one value is named `amount` (as on a payment); there is no cost. A price row is `{tenant_id, item_id, item_kind, amount, currency, starts_at}`.

- "I am not interested in a full inventory management." Products are bought and sold alongside the services booking already has: a hair colour appointment bills the colour used, or similar consumables; a dental RCT adds a tooth as a product.
- "Not sure want to create a StockMove. Likely an invoice itself can handle this." Xero (tracked items, stock changed by documents) was the reference checked.
- 1. A product is needed; like a service, a product can have hierarchy/grouping.
- 2. The same product (same SKU) may be available in multiple tenants, but not the same inventory (unlike a service, which has no inventory).
- 3. A product purchased by the salon can be invoiced, so revenue can be tracked; that increases the item count and debits the cost.
- 4. When an appointment gets an additional product, its availability is checked, like provider and resource; the invoice has it as a line item, counting to the total appointment cost, and it is deducted from stock.
- "A product is a product with its name, sku, cost etc. How much of that product in a location is a different thing, those are invoiced/purchased/sold." The product holds no stock count; the stock of a product at a location is separate, and invoices (purchased, sold) change it.
- "There is a product, and stock of it."
- "A tenant holds a stock. Like a location/salon etc will have the inventory."
- "When tenant purchase it is purchase, when tenant sale, it is sale."
- "Cost is usually associated with product. And product cost can be changed over time."
- "Once a product is sold its count goes down. This is whether via appointment or direct sell."
- Purchase is option A: "supplier is outside the scope, not a tenant. Tenant purchase." The tenant records its own purchase; opening it adds to the tenant's stock.
- "An invoice has two parties with variety, a tenant and customer." First, the tenant invoices a customer: an appointment booked ("who is coming to the salon"), a product purchase, a subscription for a SaaS. Second, "the tenant itself gets some product/service ... I as a business purchase certain items and gets invoiced" (as agentivity does, which works on this).
- "There are 4 things I want: PO, Invoice, Bill, Receipt." "Here xero model is way better." "Both direction is perfectly possible with hubspot, xero, quickbook etc." This brings back the purchase side and the order cut on 2026-10-23.
- Invoice and Bill: "They are same with a type payable/receivable."
- "Receipt is auto generated. Again likely no separate entity is needed. Once an invoice gets paid, it just gets status update/stamped."
- "So as a code only a PO & invoice are two separate entity in db."
- "Keep payment as it is now." A payment is the receipt, tied to its invoice by `invoice_id`. "For now keep it. Usually nobody pays in part as my scope of work. So it is always 1:1. Hubspot also has same model."
- The other party of an invoice "can not be a tenant ... it is client. Every appointment has a client/paying customer in the payment. There are three entity tenant, member, customer, depending on buyer/seller. If we forget member, and just use tenant & customer, then one tenant can be another customer. May be a way to represent this (some system use a universal user to represent both side), or it can be id + which kind." Chosen: "Use id + kind (client or tenant) for the other party."
- A supplier: "no separate entity. I want it to hold a booking with a supplier also. Just like a provider having a role, have client also a role."
- Approved: "Client gets role: Option<Name>, the same as Provider; existing clients have none."
- "Booked appointment doesn't hold the stocks. It is deducted only when completed. That is when payment is received. The stock is tied up with just the payment."
- "A succeeded payment of a bill adds its products to stock, the mirror of a sale."
- "When a succeeded payment is refunded in full, fails or is voided, its stock change is undone."
- PO: "need some check. May be even PO can be dropped, and it stored as same account payable invoice with additional information, and generated on the fly than another entity." Chosen: "The PO is a payable invoice that is still a draft, with the PO fields on it; no PO entity."
- Product cost "edited over time" is not correct: "when Product changes price, it should keep the history of the price as ledger." "Invoice is always on the current price: I book appointment today, my invoice is today's product/service price, doesn't matter when the appointment is completed. History is just to keep the ledger. Same can be done for service also."
- Approved: "one Price ledger for products and services (item id + kind), price and cost, appended on every change; Product and Service hold no price."
- "Take the defaults": a new `0006` migration; a tenant may stock another tenant's product; a sale payment with too little stock is refused (409); a `reference` field for the PO number; the receivable/payable field is named `kind`. "A price is required when a product or service is created."
- "Types approved. Existing services stay without a price until one is added; booking one with no price is a 409." Five PRs in order: client role; product, stock and price; invoice kind, party and PO fields; stock changes on payment; appointment products and the availability check.
- Product and stock "can live in booking module. Can rename it to better later."

## 2026-10-26 — fixing the code review findings

- "Take the issues as a group. First all the serious bugs like 18, 12, 9 etc." The first group is 9, 12, 18 and the data loss and corruption findings: 4, 5, 14, 16 (numbers of `docs/code-review.md`).
- One GitHub issue for the group, then a branch and a PR with the fixes, through the usual workflow (types and interfaces approved before tests and code). Issue #211.
- #4: "crud takes Acquire". `crud::create`/`replace`/`patch`/`delete` take `impl sqlx::Acquire` instead of `&PgPool`; the billing handlers open a transaction in which `ensure_draft` locks the invoice `FOR UPDATE` and the line checks lock it `FOR SHARE`, so a write and `:open` wait for each other.
- #9: "As megh-go". The refresh reads the account, asks the provider outside any transaction, then saves the new tokens; no row lock, so two callers at once may both refresh.
- #9 is "a SERIOUS BUG. it holds on the connection till the token is refreshed." No in-process lock: "either all need to be in sync, or if multiple request update, last update is stored in db." So: last update wins, no mutex.
- Spec change for #9: "Change concurrent_requests_refresh_once to accept 1 or 2 refreshes; both requests still get the fresh token. Then fix #9 as last update wins."
- #12: "Start at the clock". The event counter starts at the current time in microseconds instead of 0; ids stay plain increasing numbers.
- #18: "One-off migration". A method on `ConnectedAccountRepo` the app calls once after turning encryption on: every token that does not decrypt is encrypted and saved. Reads stay strict.
- #5: "400 Bad Request". A purge `before` later than now is refused; nothing is deleted.
- #14: "Yes, change spec". `totals::invoice` returns `Result<Amounts, TotalsError>`, `TooLarge` on overflow; the two calls in `tests/billing_totals_test.rs` take `.unwrap()`.
- #16: "Approve". `subscribe` refuses seats below 1 with `InvalidSeats`, as `change_seats` does, before the included seats apply.
- "Signatures approved. One PR for all seven fixes." The new method of #18 is `ConnectedAccountRepo::encrypt_stored(&self) -> Result<u64, sqlx::Error>`, the number of rows changed.
- `TableEntity::soft_delete` also takes `impl Acquire` (approved), so an invoice `DELETE` locks the invoice and deletes in one transaction.
- "Make crud::get take Acquire too, so delete calls get instead of copying its query." That does not compile (sqlx's documented "Acquire is not general enough" limit). Replaced, "go ahead": `get`, `create` and `replace` run one query and take sqlx's `PgExecutor`; `patch` and `delete` open a transaction and take `Acquire`.
- The CHANGELOG entry for PR #212: "Skip the changelog entry for now."
- "Next group: the security and tenant-isolation findings 2, 3, 6, 17. Open one issue for them, then propose the design before any code." Issue #213.
- "ALWAYS to use v7 uuid everywhere." The remaining v4 uses are a separate issue, #214.
- #213 design approved "as proposed, including the two frozen-test changes and the revoke API change" (only one frozen line changes: `broker.register` in `tests/event_ids_test.rs`):
  - `jwt_session` puts the decoded `AccessToken` into the request besides its grants.
  - #6 (RFC 9068 §4): `authorizer` refuses (403) a route whose `{tenant_id}` is not the token's `aud`, or not the `Member`'s tenant; bare grants without a token are checked as before.
  - #2 (Mercure: replayed events are authorized like live ones): replay and history use only the topics the authorizer allowed; none allowed replays nothing.
  - #3: the events authorizer gets the caller's token (`Fn(Option<&AccessToken>, &Topic) -> bool`); a connection records who opened it; `POST /sub` finds only a connection the caller opened.
  - No library has an access-token claims type (`jsonwebtoken` has none; `oauth2`'s introspection response needs `active`). "Move only the AccessToken struct into its own file compiled with axum; keep its constructor in token.rs."
  - Revoke test: sending the revocation is the `oauth2` library's job; the test checks only megh's decision of which token is revoked. "do NOT write test just to satisfy your ego. if a test needed to check complilation etc do so in scratch, not a regular test. Tests should be real & meaningful & catch logics and bugs."
  - #17 (RFC 7009): `POST /auth/{provider}/revoke` takes `{"account_id"}` and revokes the stored token of the signed-in user's own account (404 otherwise), as `disconnect` finds it; the SDK's `revokeOAuth` takes the account id.
- "v7 uuid need to be merged. it should be simple change" (#214): every `new_v4` becomes `now_v7`, every `gen_random_uuid()` becomes `uuidv7()` (PostgreSQL 18+), the `uuid` crate's `v4` feature is dropped, and tables that already exist get the v7 default by `ALTER ... SET DEFAULT`.
- "now come to events related bugs first." The events group is 1, 11 and 13 of the review (issue #217). Design "approved as proposed":
  - #1: `publish` first drops the clients whose channel is closed (tokio's `Sender::is_closed`), then sends to the rest.
  - #11: history backfill fetches every allowed topic, drops duplicates by event id and orders by time.
  - #13: the bare pattern `*` matches exactly one segment (as RabbitMQ's `*`); only `**` matches everything.
- #10 (availability): "can be just a pagination, always return a max page based on cursor as a separate pr." Issue #219.
- "fix for 8,15, 19,20. also add change log for all in the same pr": one PR for findings 8, 15, 19 and 20, with the CHANGELOG entries for every PR merged without one (#212, #215, #216, #218, #220) and for this one.
  - "#8 is ok. sqlite needs COLLATE NOCASE. we need to support both postgress and sqlite". "#15 looks ok." "#20. how this works on sqlite likely json array? this is essentially is_array vs is_object kind of thing."
  - #20 (#222): sqlx has no annotation for an array or an object column (only `json`). "for Vec<T> mark is_array. postgres store native array, and sqlite text as json. for json both stores json."
    - "Add the per-database has-filter SQL now (PostgreSQL and SQLite), with sqlx's sqlite feature for tests, and test the SQLite SQL on an in-memory SQLite database."
    - Frozen test change approved: "add is_array to rest_filter_test's ColumnMeta, true for staff_ids".
  - "make 20 separate pr for both sqlite and postgres. and rest separate": #20 is issue #222, its own PR working on both databases; #221 keeps 8, 15, 19 and the changelog.
  - Frozen tests changed for #19 and #8, approved: `a_line_belongs_to_a_draft_invoice_of_the_tenant` (a `PUT` to another invoice is a 400, the line stays on `first`, which is the one opened) and `users_columns_match_megh_go` ("expect email as citext and the other three as text").
  - "19 is ok. that is bug. report correctly": a `PUT` that changes a line's `invoice_id` is a bug (a line never changes invoice) and is refused with 400, as `PATCH` refuses it.
- "It was explicitly asked to have all list apis with pagination." Every list API pages (AIP-158 `page_size`/`page_token`); `appointments:available` is the one that does not yet (#219).
- "it should be made a list api and not an action. check possibility to move it as a list api (but can not be made GET easily using the request shape). such as POST /tenants/{tenant_id}/available_slots"
  - A standard List must be GET with no body (AIP-132), and POST on a collection is create (AIP-133). Approved: "POST /tenants/{tenant_id}/slots:search returning Page {items, next_page_token}, with the design and the frozen-test changes as proposed." The body takes `page_size` (default 50, max 1000) and `page_token`; a page searches at most 7 days and never past `to`; the cursor is a `paginator-utils` cursor on `starts_at`; `next_available_day` and its 180-day look-ahead go; the grant is `slots:search`.

## 2026-10-26 — tenant and org

- "Rename the org as tenant, which is a better name to represent. so that it can represent an generic partition."
- "Use tenant for the partition; org stays the physical body that owns tenants and hierarchies." An org has one or more tenants and hierarchies.
- Database: "Rename tables too." The tables and columns become `tenants`/`tenant_id`, which breaks the schema shared with megh-go until megh-go follows.
- API: "Rename to tenant." Routes become `/tenants/{tenant_id}/...` and the token carries a tenant; breaking for existing callers.
- The current `Org` (the `organizations` row) "becomes Tenant" as is; a physical Org that owns tenants is designed later, separately.
- The contact and address work (PR #208) "is already merged"; the rename goes in its own PR from `main`.
- `megh::migrate` reruns every migration file on each start, so a one-off rename would be undone. Option A (a rename step first) was chosen, then replaced: "you do not need to bother too much about migrations. can compact and create a single migration also for each module."
- Old databases: "just rename existing db. do not need to have migrations available. have a single migration for each module as if it is already applied." No rename step is kept; the history of migrations is not kept.
- Five files by module: tenant (tenants, members, invites, configs), auth (users, connected accounts, sessions), audit, billing (plans to subscriptions, taxes, invoices, lines, payments, counters), booking (resources to reservations).
- Compacting and the rename go in one PR (#209).
- Open: where the hierarchy lives.

## 2026-10-24 — authentication of an API server called by browsers, servers and LLM/MCP clients

- megh is an API server; "I am least bothered if a frontend hosts on a entirely different domain, or another server calls." Nothing may disallow a call from another server.
- App scopes (grants) cannot be stored in the identity provider (Google); they are megh's own, carried in megh's JWT `scope`. `connected_accounts` keeps the provider's tokens and scopes (a separate thing) so megh can renew them and fetch the provider's resources (calendar, mail).
- Bearer or cookie: "perfectly fine to use bearer or cookie."
- Browser user: "oauth via google via authorization_code -> a short lived access token."
- Server to server: "own oauth -> access token via client credentials."
- No token endpoint on the existing provider routes: "/token, /revoke, /login endpoints" already exist there (identity provider flow).
- CSRF: "you can replace csrf with Sec-Fetch-Site check. I am not really bothered about old browsers." The stateless check must not read the DB per request.
- Browser callers from a subdomain are allowed: "xyz.com can call apis api.xyz.com". The login routes' CSRF check must let a configured site through.
- Client credentials for machines: "do hand roll this case. likely safer bet." The token endpoint is written in megh (no `oxide-auth`), with the `oauth2` crate's response types.
- A machine is another user: `account_id` is the client id, and "email can also be made client_id so pk remains same". The secret is stored on the user (`password_hash`).
- "No need to store generated jwt in connected_accounts. it is not calling api via this token, it is generating the token."
- The request is `grant_type=client_credentials&scope=...`; nothing prevents the `scope` parameter.
- "Always A. one token for one org." A token carries the grants of one org only (decided after the cross-org problem was explained: grants of one org must never count in another).
- Machine creation is just a function for now ("will have admin api later").
- "Can have an admin api" to create machines.
- "I do NOT want Claude to invent any of its own imaginary solution": propose only what is backed by a reference.

## 2026-10-23 — payments keep the invoice balance (S4, approved)

- Processing and succeeded payments count toward `amount_paid` (the sum of `amount` less `refunded`); a failed or void payment does not. `balance_due` is the total less `amount_paid`.
- "No overpaying with 409": a payment that would pay more than is owed is refused.
- "Refund via PATCH `refunded`": the refund is the refunded part of the payment; no new verb.
- "Number set when recorded and never changed": the payment's number comes from the app's payment `Ids` when it is recorded; a body that sets `number` is refused.
- "The payment tests updated for the number and Formats."

## 2026-10-24 — S5 discarded; one convention across the project

- PR #194 (S5: `parent_id` on `organizations`, a trigger that gives every client an org, the customer check) was discarded: "Discard it. Totally bogus." Reason given: "It was not discussed, and you implemented without informing me. Do not try to hide things. Be very explicit with decisions." The branch and the PR were closed and deleted; `main` never had it. How a customer is identified, and who may be invoiced, is open again and must be discussed before any design or code.
- "No need to do any changes in megh-go. Want to have an implementation for megh-rs only. Will come to megh-go later."
- "Every part of the project should use similar convention." Billing follows booking: an entity's type and its routes in one file (like `client.rs`), the router and OpenAPI in the module's `mod.rs`, the same naming. "Do not do things on your own": a choice of structure, naming or layout is asked, never decided; the file layout is shown with the types for approval. Every decision is stated explicitly, including what a change does to existing data.

## 2026-10-23 — open and void an invoice (S3, approved)

- Totals: "per-line half-up rounding" (each line's base, tax and total are rounded to the currency's minor unit, halves up; the invoice is the sum of its lines). `:open` and `:void` as described: `:open` needs a line, takes the number from the app's `Ids`, fixes the totals and locks the invoice; `:void` only for an open invoice with nothing paid.
- "`bill_to` kept as drafted until Org has an address": `:open` does not copy the customer's address yet.
- "`Formats` as an Extension": the app gives the billing router its invoice and payment `Ids` with an axum `Extension`.

## 2026-10-23 — billing is simple invoicing and payment

- "It should be simple: an inventory and an invoicing." Billing already has subscriptions (the SaaS plans of an org); the invoicing goes in the same module. "I want to make it absolutely simple."
- HubSpot is the reference for invoicing (https://developers.hubspot.com/docs/api-reference/latest/overview), checked repeatedly. Product and inventory are not from HubSpot: "those need to be taken from Odoo / Zoho."
- Discount and tax are a percentage only for now. "Note down that a fixed slab is to be done later (usually a fixed discount are vouchers which can be applied to a payment)."
- "The vendor is nothing but an org." No separate vendor entity. The customer also always points to an org: "if an org be auto generated to a client, so it always points to an org."
- "ExchangeRate likely not useful. Where is it to be used? All are on a fixed base unit." Removed. A payment has no PaymentKind, no location, no exchange rate, no reference ("too many unnecessary fields").
- Cut approved ("mostly ok"): Vendor, PaymentKind, the purchase side, ExchangeRate, extra payment fields, credit notes, Application, PaymentTerm, Order and OrderLine, Document, Party. A payment pays one invoice and keeps its refunded part (as HubSpot does); an invoice stores its amounts (subtotal, taxes, total, amount paid, balance due) as HubSpot does.
- The id of an invoice and of a payment (receipt) is natural and customizable: "a prefix, separator, optional middle part (usually date) and sequential." "Its constructor takes a function to generate the id. It is stored the same way (Name etc)." So: an `Ids` function supplied by the app, the result stored as a `Name`, a counter row per org, kind and scope. Prefix can be generic (INV) or the company abbreviation.
- "What is Document? And why hardcoding the table names? All entities have a Table annotation to give the name and can be configured." Helpers use the entity's `TABLE_NAME`, not a hardcoded name.
- Interim for who may be a customer: a nullable `parent_id` on `organizations`; a customer org must have the seller as its parent (approved, S5).
- Plain words: "too much complex ... with lots of unnecessary jargons". Explain in plain words.

## 2026-10-22 — address, org, entities (earlier in the same work)

- A generic address type, used for the org, the invoice and so on (exists: `booking::value::Address`). "Any shared value can go to a common module, e.g. address and contact can be moved to org and used." Reorganizing modules is for later: "entities are public, so should work without moving."
- Org: industry, language and primary "don't make sense that much". Keep name and description, add email and home, work and mobile phone. "Org can be implemented as entity. That is perfectly ok."
- Product is its own entity, not Service. Org denotes both organization and location through the flexible hierarchy, the same as booking. Quantity is a decimal (`rust_decimal`). A `completed` status works for both product and service. The same grouping serves the product catalogue. A SKU can be at multiple locations; like a room it has a single owner, and like capacity it has a stock count.
- Money has a currency with a minor unit and a factor; rounding is by the library (`rusty_money`).
